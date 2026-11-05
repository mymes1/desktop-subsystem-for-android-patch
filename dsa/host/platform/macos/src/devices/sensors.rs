//! The Mac's motion and light sensors, as the guest's sensor HAL asks for them.
//!
//! A Mac has no accelerometer, gyroscope or orientation sensor that macOS lets a program
//! read: Core Motion's device motion is not available on macOS, and the ambient light
//! sensor has no public interface. So the sensor port is served, and the guest is told the
//! truth: this computer lends it no sensors. Its apps see a device without them, as on a
//! desktop without a sensor hub, rather than a port that refuses the connection.

use std::time::Duration;

use dsa_host_core::sensors::{HostSensorInfo, HostSensors, ReadingSink, RunningSensor, SensorKind};

/// The Mac's sensors: none that macOS lets a program read.
#[derive(Debug, Default)]
pub struct MacSensors;

impl HostSensors for MacSensors {
    fn available(&self) -> Vec<HostSensorInfo> {
        Vec::new()
    }

    fn start(
        &self,
        kind: SensorKind,
        _interval: Duration,
        _sink: ReadingSink,
    ) -> Result<Box<dyn RunningSensor>, String> {
        Err(format!(
            "this Mac has no {kind:?} sensor that macOS lets a program read"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn fr26_a_mac_offers_the_guest_no_sensor_it_cannot_read() {
        let sensors = MacSensors;
        assert!(sensors.available().is_empty());
        let sink: ReadingSink = Arc::new(|_| {});
        assert!(sensors
            .start(SensorKind::Accelerometer, Duration::from_millis(20), sink)
            .is_err());
    }
}
