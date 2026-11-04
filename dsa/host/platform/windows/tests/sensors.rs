//! The computer's sensors for the guest: the rate Windows is asked for, and, on Windows,
//! reading a computer that may have no sensors at all without failing.

use std::time::Duration;

use dsa_host_windows::sensors::report_interval_ms;

#[test]
fn fr26_the_report_interval_is_never_shorter_than_the_sensor_gives() {
    assert_eq!(report_interval_ms(Duration::from_millis(5), 16), 16);
    assert_eq!(report_interval_ms(Duration::from_millis(50), 16), 50);
    // 0 would ask Windows for the sensor's default rate, not the fastest.
    assert_eq!(report_interval_ms(Duration::ZERO, 0), 1);
    let forever = Duration::from_secs(u64::MAX);
    assert_eq!(report_interval_ms(forever, 0), u32::MAX);
}

#[cfg(windows)]
#[test]
fn fr26_listing_the_sensors_never_fails_and_names_each_kind_once() {
    use dsa_host_core::sensors::HostSensors;
    use dsa_host_windows::sensors::WindowsSensors;

    // A build machine usually has no sensors, and then the list is empty.
    let listed = WindowsSensors.available();
    for (i, sensor) in listed.iter().enumerate() {
        assert!(listed[..i].iter().all(|other| other.kind != sensor.kind));
    }
}

#[cfg(windows)]
#[test]
fn fr26_a_sensor_the_computer_lacks_does_not_start() {
    use std::sync::Arc;

    use dsa_host_core::sensors::{HostReading, HostSensors, ReadingSink, SensorKind};
    use dsa_host_windows::sensors::WindowsSensors;

    let listed = WindowsSensors.available();
    for kind in SensorKind::ALL {
        if listed.iter().any(|sensor| sensor.kind == kind) {
            continue;
        }
        let interval = Duration::from_millis(20);
        let sink: ReadingSink = Arc::new(|_: HostReading| {});
        let started = WindowsSensors.start(kind, interval, sink);
        assert!(started.is_err(), "the {} started", kind.name());
    }
}
