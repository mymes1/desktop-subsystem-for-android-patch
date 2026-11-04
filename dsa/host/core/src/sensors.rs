//! The host's motion and light sensors, read by the guest's sensors HAL.
//!
//! The guest's sensors HAL (`android.hardware.sensors`) opens one connection to the sensor
//! port. The host answers its Hello (a Hello of [`Role::GuestSensors`], which needs no
//! secret) with the sensors this computer has ([`SensorList`]): its accelerometer,
//! gyroscope, orientation and light sensors, each only when it is there and the user lets
//! DSA read it. A sensor runs only while the HAL has it on (`SensorActivate`), which is
//! while an app in the guest listens to it, and every sensor stops when the connection ends.
//!
//! Readings are converted here to Android's units and axes, so the guest passes them on as
//! they come: gravity in g becomes m/s^2 and its sign turns (a host reports which way gravity
//! pulls, Android the acceleration that holds the device up), degrees per second become
//! radians per second, and the orientation is a unit quaternion whose real part is never
//! negative.
//!
//! Any guest process can reach the port, so every message is checked and nothing the peer
//! sends can take the host down: a bad message ends that connection only.
//!
//! Nothing here names a host operating system API. The platform reads its sensors through
//! [`HostSensors`].

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use dsa_protocol::control::envelope::Body;
use dsa_protocol::control::{
    Envelope, Role, SensorDescription, SensorList, SensorReading, SensorReadings,
};
use dsa_protocol::handshake::{host_accept_open, local_hello};
use dsa_protocol::{decode, encode};
use dsa_transport::{Connection, Listener, TransportError};

/// The vsock port the guest's sensors HAL connects to.
pub const SENSOR_PORT: u32 = 5580;

/// How long the host waits for the guest's Hello.
pub const OPEN_TIMEOUT: Duration = Duration::from_secs(5);

/// The most connections served at once. The guest has one sensors HAL; a second covers the
/// moment a restarted HAL connects before the old connection is seen to close.
pub const MAX_CONNECTIONS: usize = 2;

/// The shortest time between readings the host asks a sensor for. Android asks for 200 Hz at
/// most.
pub const MIN_INTERVAL: Duration = Duration::from_millis(5);

/// The longest time between readings the host asks a sensor for. A slower sensor is of no
/// use to an app.
pub const MAX_INTERVAL: Duration = Duration::from_secs(1);

/// How often the host sends the readings it has while a sensor runs.
pub const SEND_INTERVAL: Duration = Duration::from_millis(10);

/// The most readings kept waiting to be sent. A guest that does not read cannot make the
/// host keep more: later readings are dropped until it does.
pub const MAX_QUEUED: usize = 512;

/// Standard gravity, in m/s^2.
pub const STANDARD_GRAVITY: f64 = 9.80665;

/// A sensor the host shows to the guest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SensorKind {
    Accelerometer,
    Gyroscope,
    RotationVector,
    Light,
}

impl SensorKind {
    /// Every kind, in the order the guest lists them.
    pub const ALL: [SensorKind; 4] = [
        SensorKind::Accelerometer,
        SensorKind::Gyroscope,
        SensorKind::RotationVector,
        SensorKind::Light,
    ];

    /// The kind a control message names, if it is one.
    pub fn from_control(kind: i32) -> Option<SensorKind> {
        use dsa_protocol::control::SensorKind as Control;
        match Control::try_from(kind) {
            Ok(Control::Accelerometer) => Some(SensorKind::Accelerometer),
            Ok(Control::Gyroscope) => Some(SensorKind::Gyroscope),
            Ok(Control::RotationVector) => Some(SensorKind::RotationVector),
            Ok(Control::Light) => Some(SensorKind::Light),
            _ => None,
        }
    }

    pub fn to_control(self) -> dsa_protocol::control::SensorKind {
        use dsa_protocol::control::SensorKind as Control;
        match self {
            SensorKind::Accelerometer => Control::Accelerometer,
            SensorKind::Gyroscope => Control::Gyroscope,
            SensorKind::RotationVector => Control::RotationVector,
            SensorKind::Light => Control::Light,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            SensorKind::Accelerometer => "accelerometer",
            SensorKind::Gyroscope => "gyroscope",
            SensorKind::RotationVector => "orientation sensor",
            SensorKind::Light => "light sensor",
        }
    }

    /// The largest value the guest is told to expect, in Android's units: 4 g, 2000 degrees
    /// per second, a unit quaternion, and full sunlight. A host does not say its sensor's
    /// range, and these cover the sensors PCs carry.
    pub fn max_range(self) -> f32 {
        match self {
            SensorKind::Accelerometer => (4.0 * STANDARD_GRAVITY) as f32,
            SensorKind::Gyroscope => 2000f64.to_radians() as f32,
            SensorKind::RotationVector => 1.0,
            SensorKind::Light => 100_000.0,
        }
    }

    /// The smallest step the guest is told to expect, in Android's units.
    pub fn resolution(self) -> f32 {
        match self {
            SensorKind::Accelerometer => (0.001 * STANDARD_GRAVITY) as f32,
            SensorKind::Gyroscope => 0.1f64.to_radians() as f32,
            SensorKind::RotationVector => 0.0001,
            SensorKind::Light => 1.0,
        }
    }
}

/// One sensor this computer has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostSensorInfo {
    pub kind: SensorKind,
    /// What the guest's sensor list calls it.
    pub name: String,
    pub vendor: String,
    /// The shortest time between readings the sensor gives.
    pub min_interval: Duration,
}

/// One reading, in the host's own units and on its own axes: x to the right of the screen,
/// y up it, z out of it toward the user.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HostReading {
    /// Gravity and acceleration in g, as a host reports them: a device lying face up reads
    /// z = -1.
    Acceleration { x: f64, y: f64, z: f64 },
    /// Rate of rotation in degrees per second, counter-clockwise about each axis positive.
    AngularVelocity { x: f64, y: f64, z: f64 },
    /// Orientation as a quaternion.
    Orientation { w: f64, x: f64, y: f64, z: f64 },
    /// Illuminance in lux.
    Illuminance { lux: f64 },
}

impl HostReading {
    pub fn kind(&self) -> SensorKind {
        match self {
            HostReading::Acceleration { .. } => SensorKind::Accelerometer,
            HostReading::AngularVelocity { .. } => SensorKind::Gyroscope,
            HostReading::Orientation { .. } => SensorKind::RotationVector,
            HostReading::Illuminance { .. } => SensorKind::Light,
        }
    }

    /// The reading in Android's units and on its axes, in the order Android's sensor event
    /// gives them. Nothing for a reading that is not a number, which a sensor sometimes
    /// gives as it starts.
    pub fn to_android(self) -> Option<Vec<f32>> {
        let values = match self {
            // Android reports the acceleration that holds the device up: +9.81 on z when it
            // lies face up, where the host reports gravity's pull, -1 g.
            HostReading::Acceleration { x, y, z } => vec![
                (-x * STANDARD_GRAVITY) as f32,
                (-y * STANDARD_GRAVITY) as f32,
                (-z * STANDARD_GRAVITY) as f32,
            ],
            HostReading::AngularVelocity { x, y, z } => vec![
                x.to_radians() as f32,
                y.to_radians() as f32,
                z.to_radians() as f32,
            ],
            HostReading::Orientation { w, x, y, z } => {
                let norm = (w * w + x * x + y * y + z * z).sqrt();
                if !(norm.is_finite() && norm > 0.0) {
                    return None;
                }
                // q and -q are the same rotation; Android's has cos(angle / 2) >= 0.
                let sign = if w < 0.0 { -1.0 } else { 1.0 };
                let scale = sign / norm;
                vec![
                    (x * scale) as f32,
                    (y * scale) as f32,
                    (z * scale) as f32,
                    (w * scale) as f32,
                ]
            }
            HostReading::Illuminance { lux } => vec![lux.max(0.0) as f32],
        };
        values.iter().all(|v| v.is_finite()).then_some(values)
    }
}

/// Where a running sensor hands its readings. It may be called on any thread.
pub type ReadingSink = Arc<dyn Fn(HostReading) + Send + Sync>;

/// A sensor that runs on the host. Dropping it stops the sensor; no reading reaches the sink
/// after the drop returns.
pub trait RunningSensor: Send {
    /// Asks for a reading about every `interval`.
    fn set_interval(&mut self, interval: Duration) -> Result<(), String>;
}

/// The host's sensors.
pub trait HostSensors: Send + Sync {
    /// The sensors this computer has and its user lets DSA read. Asked once for each
    /// connection.
    fn available(&self) -> Vec<HostSensorInfo>;
    /// Starts the sensor of `kind`, with a reading about every `interval`.
    fn start(
        &self,
        kind: SensorKind,
        interval: Duration,
        sink: ReadingSink,
    ) -> Result<Box<dyn RunningSensor>, String>;
}

/// Why a connection ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SensorEnd {
    /// The guest closed it.
    Closed,
    /// It ended on an error; the detail says what.
    Failed(String),
}

/// The sensors running for one connection.
type Running = BTreeMap<SensorKind, Box<dyn RunningSensor>>;

fn send(connection: &mut dyn Connection, body: Body) -> Result<(), TransportError> {
    connection.send(&encode(&Envelope { body: Some(body) }))
}

fn micros(duration: Duration) -> u32 {
    u32::try_from(duration.as_micros()).unwrap_or(u32::MAX)
}

/// The interval the host asks a sensor for when the guest asks for `interval_us`: never
/// shorter than the sensor gives, and within [`MIN_INTERVAL`] and [`MAX_INTERVAL`].
pub fn interval_for(interval_us: u32, sensor: &HostSensorInfo) -> Duration {
    let asked = Duration::from_micros(u64::from(interval_us));
    let shortest = sensor.min_interval;
    asked.max(shortest).clamp(MIN_INTERVAL, MAX_INTERVAL)
}

/// Readings on their way from the sensors' threads to the connection.
struct Queue {
    rx: Receiver<SensorReading>,
    sink: ReadingSink,
}

impl Queue {
    fn new() -> Queue {
        let (tx, rx) = mpsc::sync_channel::<SensorReading>(MAX_QUEUED);
        let started = Instant::now();
        let sink: ReadingSink = Arc::new(move |reading: HostReading| {
            let Some(values) = reading.to_android() else {
                return;
            };
            let elapsed = started.elapsed().as_micros();
            let timestamp_us = u64::try_from(elapsed).unwrap_or(u64::MAX);
            // Full: the guest is behind, and this reading is dropped.
            let _ = tx.try_send(SensorReading {
                kind: reading.kind().to_control() as i32,
                timestamp_us,
                values,
            });
        });
        Queue { rx, sink }
    }

    /// The readings waiting, of the sensors that still run.
    fn take(&self, running: &Running) -> Vec<SensorReading> {
        let mut readings = Vec::new();
        while let Ok(reading) = self.rx.try_recv() {
            let kind = SensorKind::from_control(reading.kind);
            if kind.is_some_and(|kind| running.contains_key(&kind)) {
                readings.push(reading);
            }
        }
        readings
    }

    /// Drops every reading waiting.
    fn clear(&self) {
        while self.rx.try_recv().is_ok() {}
    }
}

/// Runs one connection on the sensor port to its end. `build` names this host in its Hello;
/// `log` takes a line for each sensor that would not start.
pub fn serve_connection(
    mut connection: Box<dyn Connection>,
    sensors: &dyn HostSensors,
    build: &str,
    log: &dyn Fn(&str),
) -> SensorEnd {
    let local = local_hello(Role::Host, build);
    let accepted = host_accept_open(
        connection.as_mut(),
        &local,
        Role::GuestSensors,
        OPEN_TIMEOUT,
    );
    if let Err(error) = accepted {
        return SensorEnd::Failed(format!("the handshake failed: {error}"));
    }

    // One sensor of each kind, the first the platform names.
    let mut available: Vec<HostSensorInfo> = Vec::new();
    for sensor in sensors.available() {
        if !available.iter().any(|s| s.kind == sensor.kind) {
            available.push(sensor);
        }
    }
    let list = SensorList {
        sensors: available
            .iter()
            .map(|sensor| SensorDescription {
                kind: sensor.kind.to_control() as i32,
                name: sensor.name.chars().take(128).collect(),
                vendor: sensor.vendor.chars().take(128).collect(),
                max_range: sensor.kind.max_range(),
                resolution: sensor.kind.resolution(),
                min_interval_us: micros(sensor.min_interval.clamp(MIN_INTERVAL, MAX_INTERVAL)),
            })
            .collect(),
    };
    if let Err(error) = send(connection.as_mut(), Body::SensorList(list)) {
        return SensorEnd::Failed(format!("the sensor list did not go out: {error}"));
    }

    let queue = Queue::new();
    // Dropped on every way out of the loop, which stops each sensor still running.
    let mut running: Running = BTreeMap::new();
    loop {
        let wait = if running.is_empty() {
            None
        } else {
            Some(SEND_INTERVAL)
        };
        match connection.recv(wait) {
            Ok(frame) => match decode(&frame).ok().and_then(|e| e.body) {
                Some(Body::SensorActivate(activate)) => {
                    let Some(kind) = SensorKind::from_control(activate.kind) else {
                        continue;
                    };
                    let name = kind.name();
                    if !activate.enabled {
                        running.remove(&kind);
                        if running.is_empty() {
                            // Nothing runs: what is left would be stale by the next start.
                            queue.clear();
                        }
                        continue;
                    }
                    let Some(sensor) = available.iter().find(|s| s.kind == kind) else {
                        // Not in the list this connection had, so the guest cannot have it.
                        continue;
                    };
                    let interval = interval_for(activate.interval_us, sensor);
                    if let Some(current) = running.get_mut(&kind) {
                        if let Err(why) = current.set_interval(interval) {
                            log(&format!("sensors: the {name} kept its rate: {why}"));
                        }
                        continue;
                    }
                    match sensors.start(kind, interval, Arc::clone(&queue.sink)) {
                        Ok(started) => {
                            running.insert(kind, started);
                        }
                        Err(why) => log(&format!("sensors: the {name} did not start: {why}")),
                    }
                }
                _ => {
                    let why = "the guest sent a message the sensor port does not take";
                    return SensorEnd::Failed(why.into());
                }
            },
            Err(TransportError::Timeout) => {}
            Err(TransportError::Closed) => return SensorEnd::Closed,
            Err(error) => return SensorEnd::Failed(format!("the connection failed: {error}")),
        }
        let readings = queue.take(&running);
        if !readings.is_empty() {
            let body = Body::SensorReadings(SensorReadings { readings });
            if let Err(error) = send(connection.as_mut(), body) {
                return SensorEnd::Failed(format!("readings did not go out: {error}"));
            }
        }
    }
}

/// Accepts connections on `listener` until `stop` is set, and runs each on a thread of its
/// own, at most [`MAX_CONNECTIONS`] at once. `log` takes a line for each connection that ends
/// on an error. A listener that fails (the guest's VM went away) is tried again a second
/// later.
pub fn serve(
    mut listener: Box<dyn Listener>,
    sensors: Arc<dyn HostSensors>,
    build: String,
    stop: Arc<AtomicBool>,
    log: Arc<dyn Fn(&str) + Send + Sync>,
) {
    let running = Arc::new(AtomicUsize::new(0));
    while !stop.load(Ordering::Relaxed) {
        let mut connection = match listener.accept(Duration::from_millis(250)) {
            Ok(connection) => connection,
            Err(TransportError::Timeout) => continue,
            Err(error) => {
                log(&format!(
                    "sensors: the sensor port took no connection: {error}"
                ));
                thread::sleep(Duration::from_secs(1));
                continue;
            }
        };
        if running.load(Ordering::SeqCst) >= MAX_CONNECTIONS {
            log(&format!(
                "sensors: refused a connection; {MAX_CONNECTIONS} are already open"
            ));
            connection.close();
            continue;
        }
        running.fetch_add(1, Ordering::SeqCst);
        let (sensors, build, counter, log) = (
            Arc::clone(&sensors),
            build.clone(),
            Arc::clone(&running),
            Arc::clone(&log),
        );
        let spawned = thread::Builder::new()
            .name("dsa-sensors".into())
            .spawn(move || {
                let report = |line: &str| log(line);
                let end = serve_connection(connection, sensors.as_ref(), &build, &report);
                if let SensorEnd::Failed(why) = end {
                    log(&format!("sensors: a connection ended: {why}"));
                }
                counter.fetch_sub(1, Ordering::SeqCst);
            });
        if spawned.is_err() {
            running.fetch_sub(1, Ordering::SeqCst);
        }
    }
}
