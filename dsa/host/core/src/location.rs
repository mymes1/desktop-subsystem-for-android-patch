//! The host's location, given to the guest as its GNSS location.
//!
//! The guest's GNSS HAL tells the host when the framework wants locations (`LocationRequest`)
//! and how often. The session keeps that word in a [`LocationHub`] and hands it to every
//! watcher. The watcher runs in the user's session, where the user's privacy settings apply,
//! and drives the host's location service through a [`LocationProvider`]: started only while
//! a location is wanted, stopped as soon as it is not. Each position comes back as a
//! [`LocationEvent::Fix`] and goes to the guest as a `LocationFix`; whether the host lets DSA
//! read its location goes as a `LocationStatus`. A denied or missing location service is a
//! status, never a crash: the guest's apps see no fix, as on a phone with location off.
//!
//! [`follow`] is that watcher's loop over the local API, and [`LocationBridge`] its rules,
//! apart from any transport so they are tested with a fake provider. Nothing here names a
//! host operating system API; the platform supplies the provider.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use dsa_protocol::control;
use dsa_protocol::host_api::types;
use dsa_transport::Connector;

use crate::host_api::HostApiClient;

/// How often the host gives a location when the guest does not say.
pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(1);
/// The shortest interval the host runs its location service at, whatever the guest asks:
/// no host location service gives new positions faster, and asking costs power.
pub const MIN_INTERVAL: Duration = Duration::from_millis(100);
/// The longest interval the host takes from the guest.
pub const MAX_INTERVAL: Duration = Duration::from_secs(3600);

/// Whether the guest wants locations now, and how often.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wanted {
    pub active: bool,
    pub interval: Duration,
}

impl Wanted {
    /// No location is wanted.
    pub const NONE: Wanted = Wanted {
        active: false,
        interval: DEFAULT_INTERVAL,
    };

    /// Locations every `interval`, kept between [`MIN_INTERVAL`] and [`MAX_INTERVAL`]; a zero
    /// interval is [`DEFAULT_INTERVAL`].
    pub fn every(interval: Duration) -> Wanted {
        let interval = if interval.is_zero() {
            DEFAULT_INTERVAL
        } else {
            interval.clamp(MIN_INTERVAL, MAX_INTERVAL)
        };
        Wanted {
            active: true,
            interval,
        }
    }

    fn from_parts(active: bool, interval_ms: u32) -> Wanted {
        if active {
            Wanted::every(Duration::from_millis(u64::from(interval_ms)))
        } else {
            Wanted::NONE
        }
    }

    /// What a `LocationRequest` from the guest asks for.
    pub fn from_control(request: &control::LocationRequest) -> Wanted {
        Wanted::from_parts(request.active, request.interval_ms)
    }

    pub fn from_host_api(wanted: &types::LocationWanted) -> Wanted {
        Wanted::from_parts(wanted.active, wanted.interval_ms)
    }

    pub fn to_host_api(self) -> types::LocationWanted {
        types::LocationWanted {
            active: self.active,
            interval_ms: if self.active {
                u32::try_from(self.interval.as_millis()).unwrap_or(u32::MAX)
            } else {
                0
            },
        }
    }
}

/// Whether the host lets DSA read its location.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Access {
    Allowed,
    /// Turned off for DSA, or for the whole computer, in the host's privacy settings.
    Denied,
    /// No location device, or the location service failed.
    Unavailable,
}

impl Access {
    fn to_control(self) -> control::LocationAccess {
        match self {
            Access::Allowed => control::LocationAccess::Allowed,
            Access::Denied => control::LocationAccess::Denied,
            Access::Unavailable => control::LocationAccess::Unavailable,
        }
    }

    fn to_host_api(self) -> types::LocationAccess {
        match self {
            Access::Allowed => types::LocationAccess::Allowed,
            Access::Denied => types::LocationAccess::Denied,
            Access::Unavailable => types::LocationAccess::Unavailable,
        }
    }

    fn from_host_api(value: i32) -> Result<Access, String> {
        match types::LocationAccess::try_from(value) {
            Ok(types::LocationAccess::Allowed) => Ok(Access::Allowed),
            Ok(types::LocationAccess::Denied) => Ok(Access::Denied),
            Ok(types::LocationAccess::Unavailable) => Ok(Access::Unavailable),
            _ => Err(format!("a location status of unknown access {value}")),
        }
    }

    /// In words, for the logs.
    pub fn describe(self) -> &'static str {
        match self {
            Access::Allowed => "allowed",
            Access::Denied => "turned off in the privacy settings",
            Access::Unavailable => "not available",
        }
    }
}

/// One position from the host's location service. A value that is not known is `None`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fix {
    pub latitude_degrees: f64,
    pub longitude_degrees: f64,
    /// The radius of 68% confidence, in meters.
    pub horizontal_accuracy_meters: Option<f32>,
    /// Above the WGS84 ellipsoid, in meters.
    pub altitude_meters: Option<f64>,
    pub vertical_accuracy_meters: Option<f32>,
    pub speed_meters_per_second: Option<f32>,
    /// Degrees east of true north, from 0 up to 360.
    pub bearing_degrees: Option<f32>,
    /// When it was taken, in milliseconds since the Unix epoch (UTC).
    pub unix_time_ms: u64,
}

/// A known, finite, positive value, or nothing.
fn positive(value: f32) -> Option<f32> {
    (value.is_finite() && value > 0.0).then_some(value)
}

impl Fix {
    /// Checks that the position is one on Earth, and drops the values that cannot be right
    /// (a negative speed, an accuracy that is not a number), so nothing the host's location
    /// service gets wrong reaches the guest's apps.
    pub fn checked(self) -> Result<Fix, String> {
        let (lat, lon) = (self.latitude_degrees, self.longitude_degrees);
        if !lat.is_finite() || !(-90.0..=90.0).contains(&lat) {
            return Err(format!("a latitude of {lat} degrees is not on Earth"));
        }
        if !lon.is_finite() || !(-180.0..=180.0).contains(&lon) {
            return Err(format!("a longitude of {lon} degrees is not on Earth"));
        }
        Ok(Fix {
            horizontal_accuracy_meters: self.horizontal_accuracy_meters.and_then(positive),
            altitude_meters: self.altitude_meters.filter(|a| a.is_finite()),
            vertical_accuracy_meters: self.vertical_accuracy_meters.and_then(positive),
            speed_meters_per_second: self
                .speed_meters_per_second
                .filter(|s| s.is_finite() && *s >= 0.0),
            bearing_degrees: self
                .bearing_degrees
                .filter(|b| b.is_finite())
                .map(|b| b.rem_euclid(360.0)),
            ..self
        })
    }

    pub fn to_control(&self) -> control::LocationFix {
        control::LocationFix {
            latitude_degrees: self.latitude_degrees,
            longitude_degrees: self.longitude_degrees,
            horizontal_accuracy_meters: self.horizontal_accuracy_meters.unwrap_or(0.0),
            has_altitude: self.altitude_meters.is_some(),
            altitude_meters: self.altitude_meters.unwrap_or(0.0),
            vertical_accuracy_meters: self.vertical_accuracy_meters.unwrap_or(0.0),
            has_speed: self.speed_meters_per_second.is_some(),
            speed_meters_per_second: self.speed_meters_per_second.unwrap_or(0.0),
            has_bearing: self.bearing_degrees.is_some(),
            bearing_degrees: self.bearing_degrees.unwrap_or(0.0),
            unix_time_ms: self.unix_time_ms,
        }
    }

    pub fn to_host_api(&self) -> types::LocationFix {
        let fix = self.to_control();
        types::LocationFix {
            latitude_degrees: fix.latitude_degrees,
            longitude_degrees: fix.longitude_degrees,
            horizontal_accuracy_meters: fix.horizontal_accuracy_meters,
            has_altitude: fix.has_altitude,
            altitude_meters: fix.altitude_meters,
            vertical_accuracy_meters: fix.vertical_accuracy_meters,
            has_speed: fix.has_speed,
            speed_meters_per_second: fix.speed_meters_per_second,
            has_bearing: fix.has_bearing,
            bearing_degrees: fix.bearing_degrees,
            unix_time_ms: fix.unix_time_ms,
        }
    }

    /// The fix a client sent, checked.
    pub fn from_host_api(fix: &types::LocationFix) -> Result<Fix, String> {
        Fix {
            latitude_degrees: fix.latitude_degrees,
            longitude_degrees: fix.longitude_degrees,
            horizontal_accuracy_meters: positive(fix.horizontal_accuracy_meters),
            altitude_meters: fix.has_altitude.then_some(fix.altitude_meters),
            vertical_accuracy_meters: positive(fix.vertical_accuracy_meters),
            speed_meters_per_second: fix.has_speed.then_some(fix.speed_meters_per_second),
            bearing_degrees: fix.has_bearing.then_some(fix.bearing_degrees),
            unix_time_ms: fix.unix_time_ms,
        }
        .checked()
    }
}

/// What the host's location service reports.
#[derive(Debug, Clone, PartialEq)]
pub enum LocationEvent {
    Fix(Fix),
    /// Whether DSA may read the location, and why, in words for the logs.
    Status {
        access: Access,
        detail: String,
    },
}

impl LocationEvent {
    /// The control message that carries it to the guest.
    pub fn to_control(&self) -> control::envelope::Body {
        match self {
            LocationEvent::Fix(fix) => control::envelope::Body::LocationFix(fix.to_control()),
            LocationEvent::Status { access, detail } => {
                control::envelope::Body::LocationStatus(control::LocationStatus {
                    access: access.to_control() as i32,
                    detail: detail.clone(),
                })
            }
        }
    }

    /// The local API message that carries it to the service.
    pub fn to_host_api(&self) -> types::envelope::Body {
        match self {
            LocationEvent::Fix(fix) => types::envelope::Body::LocationFix(fix.to_host_api()),
            LocationEvent::Status { access, detail } => {
                types::envelope::Body::LocationStatus(types::LocationStatus {
                    access: access.to_host_api() as i32,
                    detail: detail.clone(),
                })
            }
        }
    }

    /// The event a client's status carries.
    pub fn from_host_api_status(status: &types::LocationStatus) -> Result<LocationEvent, String> {
        Ok(LocationEvent::Status {
            access: Access::from_host_api(status.access)?,
            detail: status.detail.clone(),
        })
    }
}

/// The host's location service, as the platform runs it.
pub trait LocationProvider {
    /// Starts giving a position about every `interval` to `events`, and a status each time
    /// access changes. An error says the service cannot run: DSA may not read the location,
    /// or there is none to read.
    fn start(
        &mut self,
        interval: Duration,
        events: Sender<LocationEvent>,
    ) -> Result<(), (Access, String)>;
    /// Stops the service. Nothing more is sent to the last `events`.
    fn stop(&mut self);
}

/// The rules between what the guest wants and the host's location service: the service runs
/// only while a location is wanted, is stopped when the guest stops wanting one or the bridge
/// goes away, and what it reports after it stopped is dropped.
pub struct LocationBridge<'a> {
    provider: &'a mut dyn LocationProvider,
    /// The interval the service runs at, while it runs.
    running: Option<Duration>,
    events: Option<Receiver<LocationEvent>>,
    /// What the guest last asked for.
    wanted: Wanted,
}

impl<'a> LocationBridge<'a> {
    pub fn new(provider: &'a mut dyn LocationProvider) -> LocationBridge<'a> {
        LocationBridge {
            provider,
            running: None,
            events: None,
            wanted: Wanted::NONE,
        }
    }

    /// Tries again to start a service that could not start while a location is wanted, as
    /// when the user has since turned location on. Returns what [`LocationBridge::want`]
    /// would.
    pub fn retry(&mut self) -> Vec<LocationEvent> {
        if self.wanted.active && self.running.is_none() {
            self.want(self.wanted)
        } else {
            Vec::new()
        }
    }

    /// Whether the host's location service runs now.
    pub fn running(&self) -> bool {
        self.running.is_some()
    }

    /// Takes what the guest wants now, and starts, restarts or stops the service to match.
    /// Returns what the guest should be told at once: that access is allowed when the service
    /// starts, and why not when it cannot.
    pub fn want(&mut self, wanted: Wanted) -> Vec<LocationEvent> {
        self.wanted = wanted;
        if !wanted.active {
            self.halt();
            return Vec::new();
        }
        if self.running == Some(wanted.interval) {
            return Vec::new();
        }
        self.halt();
        let (tx, rx) = mpsc::channel();
        match self.provider.start(wanted.interval, tx) {
            Ok(()) => {
                self.running = Some(wanted.interval);
                self.events = Some(rx);
                vec![LocationEvent::Status {
                    access: Access::Allowed,
                    detail: String::new(),
                }]
            }
            Err((access, detail)) => vec![LocationEvent::Status { access, detail }],
        }
    }

    /// What the service reported since the last call, while it runs.
    pub fn take(&mut self) -> Vec<LocationEvent> {
        let mut taken = Vec::new();
        let Some(events) = &self.events else {
            return taken;
        };
        loop {
            match events.try_recv() {
                Ok(LocationEvent::Fix(fix)) => {
                    if let Ok(fix) = fix.checked() {
                        taken.push(LocationEvent::Fix(fix));
                    }
                }
                Ok(status) => taken.push(status),
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
        taken
    }

    fn halt(&mut self) {
        if self.running.take().is_some() {
            self.provider.stop();
        }
        self.events = None;
    }
}

impl Drop for LocationBridge<'_> {
    fn drop(&mut self) {
        self.halt();
    }
}

/// What the guest wants and who watches it, on the session's side. A new watcher is told
/// the current word first, so one that connects while a location is wanted starts at once.
#[derive(Debug)]
pub struct LocationHub {
    wanted: Wanted,
    watchers: Vec<Sender<Wanted>>,
}

impl Default for LocationHub {
    fn default() -> LocationHub {
        LocationHub {
            wanted: Wanted::NONE,
            watchers: Vec::new(),
        }
    }
}

impl LocationHub {
    pub fn wanted(&self) -> Wanted {
        self.wanted
    }

    /// A watch of what the guest wants, from the current word on.
    pub fn watch(&mut self) -> Receiver<Wanted> {
        let (tx, rx) = mpsc::channel();
        if tx.send(self.wanted).is_ok() {
            self.watchers.push(tx);
        }
        rx
    }

    /// The guest's new word; every watcher hears it if it changed.
    pub fn set(&mut self, wanted: Wanted) {
        if wanted == self.wanted {
            return;
        }
        self.wanted = wanted;
        self.watchers.retain(|watcher| watcher.send(wanted).is_ok());
    }
}

/// How long the watcher waits for the service each time it connects.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// How long the watcher waits for the guest's word before it passes on positions again.
const POLL: Duration = Duration::from_millis(100);
/// How long the watcher waits before it connects again.
const RETRY: Duration = Duration::from_secs(2);
/// How often the watcher tries again to start a location service that could not start while
/// Android wants a location, so turning location on in the privacy settings takes effect.
const REFUSED_RETRY: Duration = Duration::from_secs(5);

/// Gives the location of the host to the caller's Android through the local API until `stop`
/// is set: runs `provider` while the Android wants locations, and sends what it reports. A
/// lost connection stops the provider and is made again.
pub fn follow(
    connector: &mut dyn Connector,
    build: &str,
    provider: &mut dyn LocationProvider,
    stop: &AtomicBool,
    log: &dyn Fn(&str),
) {
    let mut warned = false;
    while !stop.load(Ordering::Relaxed) {
        let stream = HostApiClient::connect(connector, build, CONNECT_TIMEOUT)
            .and_then(|client| client.watch_location());
        let mut stream = match stream {
            Ok(stream) => stream,
            Err(why) => {
                if !warned {
                    let line = format!("location: cannot give Android the location yet: {why}");
                    log(&line);
                    warned = true;
                }
                wait(stop, RETRY);
                continue;
            }
        };
        warned = false;
        let mut bridge = LocationBridge::new(&mut *provider);
        let mut last_access = None;
        let mut last_retry = Instant::now();
        'connected: while !stop.load(Ordering::Relaxed) {
            let mut out = match stream.next(POLL) {
                Ok(None) => Vec::new(),
                Ok(Some(wanted)) => {
                    let was_running = bridge.running();
                    let told = bridge.want(wanted);
                    if !was_running && bridge.running() {
                        let every = wanted.interval.as_millis();
                        let line = format!("location: Android wants it every {every} ms; running");
                        log(&line);
                    } else if was_running && !bridge.running() {
                        log("location: Android wants none; stopped");
                    }
                    told
                }
                Err(why) => {
                    let line = format!("location: the service connection ended: {why}");
                    log(&line);
                    break 'connected;
                }
            };
            if last_retry.elapsed() >= REFUSED_RETRY {
                last_retry = Instant::now();
                out.extend(bridge.retry());
            }
            out.extend(bridge.take());
            for event in out {
                if let LocationEvent::Status { access, detail } = &event {
                    if last_access != Some(*access) {
                        last_access = Some(*access);
                        log(&status_line(*access, detail));
                    }
                }
                if let Err(why) = stream.send(&event) {
                    let line = format!("location: the service connection ended: {why}");
                    log(&line);
                    break 'connected;
                }
            }
        }
        drop(bridge);
        wait(stop, RETRY);
    }
}

/// How a change of access reads in the log.
fn status_line(access: Access, detail: &str) -> String {
    if detail.is_empty() {
        format!("location: the host's location is {}", access.describe())
    } else {
        format!(
            "location: the host's location is {} ({detail})",
            access.describe()
        )
    }
}

/// Sleeps `time`, or less once `stop` is set.
fn wait(stop: &AtomicBool, time: Duration) {
    let step = Duration::from_millis(50);
    let mut waited = Duration::ZERO;
    while waited < time && !stop.load(Ordering::Relaxed) {
        std::thread::sleep(step);
        waited += step;
    }
}
