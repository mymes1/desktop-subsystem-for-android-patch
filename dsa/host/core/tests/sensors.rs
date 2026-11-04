//! The sensor port: the handshake, the sensor list, sensors that run only while the guest
//! has them on, and readings in Android's units, with fake sensors standing in for the
//! platform's.

use std::f64::consts::{FRAC_PI_2, PI};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use dsa_host_core::sensors::{
    interval_for, serve_connection, HostReading, HostSensorInfo, HostSensors, ReadingSink,
    RunningSensor, SensorEnd, SensorKind, MAX_INTERVAL, MIN_INTERVAL, STANDARD_GRAVITY,
};
use dsa_protocol::control::envelope::Body;
use dsa_protocol::control::{
    Envelope, Role, SensorActivate, SensorDescription, SensorKind as ControlKind, SensorReading,
};
use dsa_protocol::handshake::local_hello;
use dsa_protocol::{decode, encode};
use dsa_transport::memory;
use dsa_transport::{Connection, Connector, Listener};

const WAIT: Duration = Duration::from_secs(5);

type Log = Arc<Mutex<Vec<String>>>;
type Sinks = Arc<Mutex<Vec<(SensorKind, ReadingSink)>>>;

/// A sensor that says when it changes rate and when it stops.
struct FakeRunning {
    kind: SensorKind,
    log: Log,
}

impl RunningSensor for FakeRunning {
    fn set_interval(&mut self, interval: Duration) -> Result<(), String> {
        let line = format!("rate {} {}ms", self.kind.name(), interval.as_millis());
        self.log.lock().unwrap().push(line);
        Ok(())
    }
}

impl Drop for FakeRunning {
    fn drop(&mut self) {
        let line = format!("stop {}", self.kind.name());
        self.log.lock().unwrap().push(line);
    }
}

/// The sensors of a computer that has `present`. Its light sensor fails to start when
/// `broken_light` is set, as one the user turned off after the list was made does.
struct FakeSensors {
    present: Vec<HostSensorInfo>,
    broken_light: bool,
    log: Log,
    sinks: Sinks,
}

impl HostSensors for FakeSensors {
    fn available(&self) -> Vec<HostSensorInfo> {
        self.present.clone()
    }

    fn start(
        &self,
        kind: SensorKind,
        interval: Duration,
        sink: ReadingSink,
    ) -> Result<Box<dyn RunningSensor>, String> {
        if kind == SensorKind::Light && self.broken_light {
            return Err("access denied".into());
        }
        let line = format!("start {} {}ms", kind.name(), interval.as_millis());
        self.log.lock().unwrap().push(line);
        self.sinks.lock().unwrap().push((kind, sink));
        Ok(Box::new(FakeRunning {
            kind,
            log: Arc::clone(&self.log),
        }))
    }
}

fn sensor(kind: SensorKind, min_interval_ms: u64) -> HostSensorInfo {
    HostSensorInfo {
        kind,
        name: format!("Test {}", kind.name()),
        vendor: "DSA".into(),
        min_interval: Duration::from_millis(min_interval_ms),
    }
}

fn every_sensor() -> Vec<HostSensorInfo> {
    vec![
        sensor(SensorKind::Accelerometer, 16),
        sensor(SensorKind::Gyroscope, 16),
        sensor(SensorKind::RotationVector, 16),
        sensor(SensorKind::Light, 100),
    ]
}

fn send(connection: &mut dyn Connection, body: Body) {
    connection
        .send(&encode(&Envelope { body: Some(body) }))
        .expect("send");
}

fn next(connection: &mut dyn Connection) -> Body {
    let frame = connection
        .recv(Some(WAIT))
        .expect("a message from the host");
    decode(&frame).expect("decodes").body.expect("has a body")
}

/// Turns the sensor of `kind` on, at a reading every `interval_us`, or off.
fn turn(guest: &mut dyn Connection, kind: ControlKind, enabled: bool, interval_us: u32) {
    let body = Body::SensorActivate(SensorActivate {
        kind: kind as i32,
        enabled,
        interval_us,
    });
    send(guest, body);
}

/// Waits until the log holds `line`.
fn wait_for(log: &Log, line: &str) {
    let deadline = Instant::now() + WAIT;
    while Instant::now() < deadline {
        if log.lock().unwrap().iter().any(|l| l == line) {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    panic!("never logged {line:?}; logged {:?}", log.lock().unwrap());
}

/// Waits until a sensor of `kind` has started, and returns where it hands its readings.
fn sink_of(sinks: &Sinks, kind: SensorKind) -> ReadingSink {
    let deadline = Instant::now() + WAIT;
    while Instant::now() < deadline {
        let started = sinks.lock().unwrap();
        if let Some((_, sink)) = started.iter().rev().find(|(k, _)| *k == kind) {
            return Arc::clone(sink);
        }
        drop(started);
        thread::sleep(Duration::from_millis(5));
    }
    panic!("the {} never started", kind.name());
}

/// The next readings the host sends.
fn readings(guest: &mut dyn Connection) -> Vec<SensorReading> {
    match next(guest) {
        Body::SensorReadings(readings) => readings.readings,
        other => panic!("expected readings, got {other:?}"),
    }
}

struct Connected {
    guest: Box<dyn Connection>,
    host: thread::JoinHandle<SensorEnd>,
    log: Log,
    sinks: Sinks,
    host_log: Log,
}

fn connect_as(role: Role, present: Vec<HostSensorInfo>, broken_light: bool) -> Connected {
    let (mut listener, mut connector) = memory::pair();
    let log: Log = Arc::default();
    let sinks: Sinks = Arc::default();
    let host_log: Log = Arc::default();
    let sensors = FakeSensors {
        present,
        broken_light,
        log: Arc::clone(&log),
        sinks: Arc::clone(&sinks),
    };
    let lines = Arc::clone(&host_log);
    let host = thread::spawn(move || {
        let connection = listener.accept(WAIT).expect("accepts");
        let report = |line: &str| lines.lock().unwrap().push(line.to_string());
        serve_connection(connection, &sensors, "dsa-host test", &report)
    });
    let mut guest = connector.connect(WAIT).expect("connects");
    let hello = local_hello(role, "dsa-sensors test");
    send(guest.as_mut(), Body::Hello(hello));
    Connected {
        guest,
        host,
        log,
        sinks,
        host_log,
    }
}

/// Connects as the guest's sensors HAL, past the handshake, and returns the sensor list too.
fn connect_with(
    present: Vec<HostSensorInfo>,
    broken_light: bool,
) -> (Connected, Vec<SensorDescription>) {
    let mut connected = connect_as(Role::GuestSensors, present, broken_light);
    let accepted = next(connected.guest.as_mut());
    assert!(matches!(accepted, Body::HelloAccepted(_)), "{accepted:?}");
    let list = match next(connected.guest.as_mut()) {
        Body::SensorList(list) => list.sensors,
        other => panic!("expected the sensor list, got {other:?}"),
    };
    (connected, list)
}

fn connect(present: Vec<HostSensorInfo>) -> (Connected, Vec<SensorDescription>) {
    connect_with(present, false)
}

/// Closes the guest's end, and returns how the host's end ended and what the sensors did.
fn close(connected: Connected) -> (SensorEnd, Vec<String>) {
    let Connected {
        mut guest,
        host,
        log,
        ..
    } = connected;
    guest.close();
    let end = host.join().expect("the host's thread ends");
    let lines = log.lock().unwrap().clone();
    (end, lines)
}

#[test]
fn fr26_the_list_names_only_the_sensors_the_computer_has() {
    let present = vec![
        sensor(SensorKind::Accelerometer, 16),
        sensor(SensorKind::Light, 100),
    ];
    let (connected, list) = connect(present);
    let kinds: Vec<i32> = list.iter().map(|s| s.kind).collect();
    let expected = [ControlKind::Accelerometer, ControlKind::Light];
    assert_eq!(kinds, expected.map(|k| k as i32));
    let (end, log) = close(connected);
    assert_eq!(end, SensorEnd::Closed);
    // Listing a sensor does not start it.
    assert!(log.is_empty(), "{log:?}");
}

#[test]
fn fr26_a_computer_without_sensors_lists_none() {
    let (connected, list) = connect(Vec::new());
    assert!(list.is_empty());
    assert_eq!(close(connected).0, SensorEnd::Closed);
}

#[test]
fn fr26_the_list_carries_ranges_and_rates_in_android_units() {
    let (connected, list) = connect(every_sensor());
    assert_eq!(list.len(), 4);
    let accelerometer = &list[0];
    assert_eq!(accelerometer.name, "Test accelerometer");
    assert_eq!(accelerometer.vendor, "DSA");
    assert!((accelerometer.max_range - 39.2266).abs() < 0.001);
    assert_eq!(accelerometer.min_interval_us, 16_000);
    assert!((list[1].max_range - 34.9066).abs() < 0.001);
    assert_eq!(list[2].max_range, 1.0);
    assert_eq!(list[3].min_interval_us, 100_000);
    assert_eq!(close(connected).0, SensorEnd::Closed);
}

#[test]
fn fr26_a_sensor_runs_only_while_the_guest_has_it_on() {
    let (mut connected, _) = connect(every_sensor());
    let guest = connected.guest.as_mut();
    // Nothing runs before the guest asks.
    thread::sleep(Duration::from_millis(50));
    assert!(connected.log.lock().unwrap().is_empty());

    turn(guest, ControlKind::Accelerometer, true, 20_000);
    wait_for(&connected.log, "start accelerometer 20ms");
    turn(guest, ControlKind::Accelerometer, true, 50_000);
    wait_for(&connected.log, "rate accelerometer 50ms");
    turn(guest, ControlKind::Accelerometer, false, 0);
    wait_for(&connected.log, "stop accelerometer");

    let (end, log) = close(connected);
    assert_eq!(end, SensorEnd::Closed);
    let expected = [
        "start accelerometer 20ms",
        "rate accelerometer 50ms",
        "stop accelerometer",
    ];
    assert_eq!(log, expected);
}

#[test]
fn fr26_closing_the_connection_stops_every_sensor() {
    let (mut connected, _) = connect(every_sensor());
    let guest = connected.guest.as_mut();
    turn(guest, ControlKind::Gyroscope, true, 10_000);
    turn(guest, ControlKind::Light, true, 200_000);
    // The gyroscope gives no more than one reading every 16 ms.
    wait_for(&connected.log, "start gyroscope 16ms");
    wait_for(&connected.log, "start light sensor 200ms");
    let (end, log) = close(connected);
    assert_eq!(end, SensorEnd::Closed);
    assert!(log.iter().any(|line| line == "stop gyroscope"));
    assert!(log.iter().any(|line| line == "stop light sensor"));
}

#[test]
fn fr26_a_sensor_the_computer_lacks_never_starts() {
    let present = vec![sensor(SensorKind::Light, 100)];
    let (mut connected, _) = connect(present);
    let guest = connected.guest.as_mut();
    turn(guest, ControlKind::Gyroscope, true, 10_000);
    turn(guest, ControlKind::Light, true, 100_000);
    wait_for(&connected.log, "start light sensor 100ms");
    let (end, log) = close(connected);
    assert_eq!(end, SensorEnd::Closed);
    assert!(!log.iter().any(|line| line.contains("gyroscope")));
}

#[test]
fn fr26_readings_reach_the_guest_in_android_units() {
    let (mut connected, _) = connect(every_sensor());
    let guest = connected.guest.as_mut();

    turn(guest, ControlKind::Accelerometer, true, 20_000);
    let sink = sink_of(&connected.sinks, SensorKind::Accelerometer);
    // Lying face up and tilted: the host reports gravity's pull, -1 g on z when flat.
    sink(HostReading::Acceleration {
        x: 0.0,
        y: 0.5,
        z: -1.0,
    });
    let got = readings(guest);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].kind, ControlKind::Accelerometer as i32);
    let values: Vec<f64> = got[0].values.iter().map(|v| f64::from(*v)).collect();
    assert_eq!(values.len(), 3);
    assert!(values[0].abs() < 1e-6);
    assert!((values[1] + 0.5 * STANDARD_GRAVITY).abs() < 1e-4);
    assert!((values[2] - STANDARD_GRAVITY).abs() < 1e-4);

    turn(guest, ControlKind::Gyroscope, true, 20_000);
    let sink = sink_of(&connected.sinks, SensorKind::Gyroscope);
    sink(HostReading::AngularVelocity {
        x: 180.0,
        y: -90.0,
        z: 0.0,
    });
    let got = readings(guest);
    assert_eq!(got[0].kind, ControlKind::Gyroscope as i32);
    let values: Vec<f64> = got[0].values.iter().map(|v| f64::from(*v)).collect();
    assert!((values[0] - PI).abs() < 1e-5);
    assert!((values[1] + FRAC_PI_2).abs() < 1e-5);
    assert!(values[2].abs() < 1e-6);

    turn(guest, ControlKind::Light, true, 100_000);
    let sink = sink_of(&connected.sinks, SensorKind::Light);
    sink(HostReading::Illuminance { lux: 320.0 });
    let got = readings(guest);
    assert_eq!(got[0].kind, ControlKind::Light as i32);
    assert_eq!(got[0].values, [320.0]);
    assert_eq!(close(connected).0, SensorEnd::Closed);
}

#[test]
fn fr26_readings_that_arrive_together_keep_their_order_and_times() {
    let (mut connected, _) = connect(every_sensor());
    let guest = connected.guest.as_mut();
    turn(guest, ControlKind::Light, true, 100_000);
    let sink = sink_of(&connected.sinks, SensorKind::Light);
    for lux in [1.0, 2.0, 3.0] {
        sink(HostReading::Illuminance { lux });
    }
    let mut got = Vec::new();
    while got.len() < 3 {
        got.extend(readings(guest));
    }
    let lux: Vec<f32> = got.iter().map(|r| r.values[0]).collect();
    assert_eq!(lux, [1.0, 2.0, 3.0]);
    let times: Vec<u64> = got.iter().map(|r| r.timestamp_us).collect();
    let mut sorted = times.clone();
    sorted.sort_unstable();
    assert_eq!(times, sorted);
    assert_eq!(close(connected).0, SensorEnd::Closed);
}

#[test]
fn fr26_a_sensor_that_will_not_start_is_logged_and_the_rest_go_on() {
    let (mut connected, _) = connect_with(every_sensor(), true);
    let guest = connected.guest.as_mut();
    turn(guest, ControlKind::Light, true, 100_000);
    turn(guest, ControlKind::Gyroscope, true, 20_000);
    wait_for(&connected.log, "start gyroscope 20ms");
    let line = "sensors: the light sensor did not start: access denied";
    wait_for(&connected.host_log, line);
    assert_eq!(close(connected).0, SensorEnd::Closed);
}

#[test]
fn fr26_a_bad_message_ends_only_that_connection() {
    let (mut connected, _) = connect(every_sensor());
    let guest = connected.guest.as_mut();
    turn(guest, ControlKind::Accelerometer, true, 20_000);
    wait_for(&connected.log, "start accelerometer 20ms");
    guest.send(b"not an envelope").expect("send");
    let end = connected.host.join().expect("the host's thread ends");
    assert!(matches!(end, SensorEnd::Failed(_)), "{end:?}");
    // The sensor it had on stops with it.
    wait_for(&connected.log, "stop accelerometer");
}

#[test]
fn fr26_the_control_role_is_refused_on_the_sensor_port() {
    let mut connected = connect_as(Role::Guest, every_sensor(), false);
    let answer = next(connected.guest.as_mut());
    assert!(matches!(answer, Body::HelloRejected(_)), "{answer:?}");
    let end = connected.host.join().expect("the host's thread ends");
    assert!(matches!(end, SensorEnd::Failed(_)), "{end:?}");
    assert!(connected.log.lock().unwrap().is_empty());
}

#[test]
fn fr26_the_orientation_is_a_unit_quaternion_whose_real_part_is_not_negative() {
    let turned = HostReading::Orientation {
        w: -2.0,
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };
    assert_eq!(turned.to_android(), Some(vec![0.0, 0.0, 0.0, 1.0]));
    let scaled = HostReading::Orientation {
        w: 0.0,
        x: 0.0,
        y: 3.0,
        z: 4.0,
    };
    assert_eq!(scaled.to_android(), Some(vec![0.0, 0.6, 0.8, 0.0]));
    let empty = HostReading::Orientation {
        w: 0.0,
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };
    assert_eq!(empty.to_android(), None);
    let broken = HostReading::Acceleration {
        x: f64::NAN,
        y: 0.0,
        z: 0.0,
    };
    assert_eq!(broken.to_android(), None);
}

#[test]
fn fr26_the_rate_stays_within_what_the_sensor_gives() {
    let accelerometer = sensor(SensorKind::Accelerometer, 16);
    let sixteen = Duration::from_millis(16);
    let fifty = Duration::from_millis(50);
    assert_eq!(interval_for(1_000, &accelerometer), sixteen);
    assert_eq!(interval_for(50_000, &accelerometer), fifty);
    assert_eq!(interval_for(u32::MAX, &accelerometer), MAX_INTERVAL);
    let fast = sensor(SensorKind::Gyroscope, 0);
    assert_eq!(interval_for(0, &fast), MIN_INTERVAL);
}
