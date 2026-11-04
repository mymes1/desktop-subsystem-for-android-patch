//! The host's motion and light sensors on Windows, read through `Windows.Devices.Sensors`.
//!
//! Each kind the guest can have is the computer's default sensor of that kind: its
//! accelerometer, its gyrometer, its orientation sensor (which Windows fuses from the
//! accelerometer, the gyrometer and the compass) and its light sensor. A kind the computer
//! does not have, or that Windows does not let DSA read, has no default sensor and is left
//! out of the guest's list.
//!
//! A sensor runs only while its reading handler is attached: it is attached when the guest
//! turns the sensor on and removed when it turns it off, so Windows can power the sensor down
//! the rest of the time. The readings go to the core ([`dsa_host_core::sensors`]) as Windows
//! gives them, in g, degrees per second, a quaternion and lux; the core converts them.
//!
//! [`report_interval_ms`] is the rate rule, and is tested on every platform; the rest is
//! Windows only.

use std::time::Duration;

/// The report interval, in milliseconds, Windows is asked for when a reading is wanted about
/// every `interval`: never shorter than the sensor's `minimum_ms`, and never 0, which would
/// ask for the sensor's default rate instead.
pub fn report_interval_ms(interval: Duration, minimum_ms: u32) -> u32 {
    let asked = u32::try_from(interval.as_millis()).unwrap_or(u32::MAX);
    asked.max(minimum_ms).max(1)
}

#[cfg(windows)]
pub use system::WindowsSensors;

#[cfg(windows)]
mod system {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use dsa_host_core::sensors::{
        HostReading, HostSensorInfo, HostSensors, ReadingSink, RunningSensor, SensorKind,
    };
    use windows::core::Result as WinResult;
    use windows::Devices::Sensors::{
        Accelerometer, AccelerometerReadingChangedEventArgs, Gyrometer,
        GyrometerReadingChangedEventArgs, LightSensor, LightSensorReadingChangedEventArgs,
        OrientationSensor, OrientationSensorReadingChangedEventArgs,
    };
    use windows::Foundation::TypedEventHandler;
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

    use super::report_interval_ms;

    /// Where a sensor's handler hands its readings, until the sensor stops. Emptied before
    /// the handler is removed, so a reading Windows delivers while it is being removed goes
    /// nowhere.
    type Shared = Arc<Mutex<Option<ReadingSink>>>;

    fn give(shared: &Shared, reading: HostReading) {
        let sink = shared.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(sink) = sink.as_ref() {
            sink(reading);
        }
    }

    /// What DSA needs of each kind of Windows sensor.
    trait WindowsSensor: Sized + 'static {
        fn open() -> WinResult<Self>;
        fn minimum_ms(&self) -> WinResult<u32>;
        fn set_report_ms(&self, ms: u32) -> WinResult<()>;
        fn attach(&self, shared: Shared) -> WinResult<i64>;
        fn detach(&self, token: i64) -> WinResult<()>;
    }

    impl WindowsSensor for Accelerometer {
        fn open() -> WinResult<Self> {
            Accelerometer::GetDefault()
        }

        fn minimum_ms(&self) -> WinResult<u32> {
            self.MinimumReportInterval()
        }

        fn set_report_ms(&self, ms: u32) -> WinResult<()> {
            self.SetReportInterval(ms)
        }

        fn attach(&self, shared: Shared) -> WinResult<i64> {
            type Handler = TypedEventHandler<Accelerometer, AccelerometerReadingChangedEventArgs>;
            let handler = Handler::new(move |_, args| {
                if let Some(args) = args.as_ref() {
                    let reading = args.Reading()?;
                    let acceleration = HostReading::Acceleration {
                        x: reading.AccelerationX()?,
                        y: reading.AccelerationY()?,
                        z: reading.AccelerationZ()?,
                    };
                    give(&shared, acceleration);
                }
                Ok(())
            });
            self.ReadingChanged(&handler)
        }

        fn detach(&self, token: i64) -> WinResult<()> {
            self.RemoveReadingChanged(token)
        }
    }

    impl WindowsSensor for Gyrometer {
        fn open() -> WinResult<Self> {
            Gyrometer::GetDefault()
        }

        fn minimum_ms(&self) -> WinResult<u32> {
            self.MinimumReportInterval()
        }

        fn set_report_ms(&self, ms: u32) -> WinResult<()> {
            self.SetReportInterval(ms)
        }

        fn attach(&self, shared: Shared) -> WinResult<i64> {
            type Handler = TypedEventHandler<Gyrometer, GyrometerReadingChangedEventArgs>;
            let handler = Handler::new(move |_, args| {
                if let Some(args) = args.as_ref() {
                    let reading = args.Reading()?;
                    let rotation = HostReading::AngularVelocity {
                        x: reading.AngularVelocityX()?,
                        y: reading.AngularVelocityY()?,
                        z: reading.AngularVelocityZ()?,
                    };
                    give(&shared, rotation);
                }
                Ok(())
            });
            self.ReadingChanged(&handler)
        }

        fn detach(&self, token: i64) -> WinResult<()> {
            self.RemoveReadingChanged(token)
        }
    }

    impl WindowsSensor for OrientationSensor {
        fn open() -> WinResult<Self> {
            OrientationSensor::GetDefault()
        }

        fn minimum_ms(&self) -> WinResult<u32> {
            self.MinimumReportInterval()
        }

        fn set_report_ms(&self, ms: u32) -> WinResult<()> {
            self.SetReportInterval(ms)
        }

        fn attach(&self, shared: Shared) -> WinResult<i64> {
            type Handler =
                TypedEventHandler<OrientationSensor, OrientationSensorReadingChangedEventArgs>;
            let handler = Handler::new(move |_, args| {
                if let Some(args) = args.as_ref() {
                    let quaternion = args.Reading()?.Quaternion()?;
                    let orientation = HostReading::Orientation {
                        w: f64::from(quaternion.W()?),
                        x: f64::from(quaternion.X()?),
                        y: f64::from(quaternion.Y()?),
                        z: f64::from(quaternion.Z()?),
                    };
                    give(&shared, orientation);
                }
                Ok(())
            });
            self.ReadingChanged(&handler)
        }

        fn detach(&self, token: i64) -> WinResult<()> {
            self.RemoveReadingChanged(token)
        }
    }

    impl WindowsSensor for LightSensor {
        fn open() -> WinResult<Self> {
            LightSensor::GetDefault()
        }

        fn minimum_ms(&self) -> WinResult<u32> {
            self.MinimumReportInterval()
        }

        fn set_report_ms(&self, ms: u32) -> WinResult<()> {
            self.SetReportInterval(ms)
        }

        fn attach(&self, shared: Shared) -> WinResult<i64> {
            type Handler = TypedEventHandler<LightSensor, LightSensorReadingChangedEventArgs>;
            let handler = Handler::new(move |_, args| {
                if let Some(args) = args.as_ref() {
                    let lux = f64::from(args.Reading()?.IlluminanceInLux()?);
                    give(&shared, HostReading::Illuminance { lux });
                }
                Ok(())
            });
            self.ReadingChanged(&handler)
        }

        fn detach(&self, token: i64) -> WinResult<()> {
            self.RemoveReadingChanged(token)
        }
    }

    /// A Windows sensor while its handler is attached.
    struct Running<S: WindowsSensor> {
        sensor: S,
        token: i64,
        minimum_ms: u32,
        shared: Shared,
    }

    // SAFETY: the sensor classes of Windows.Devices.Sensors are agile, so they may be used
    // and released on any thread; the rest of the fields are Send.
    unsafe impl<S: WindowsSensor> Send for Running<S> {}

    impl<S: WindowsSensor> RunningSensor for Running<S> {
        fn set_interval(&mut self, interval: Duration) -> Result<(), String> {
            let ms = report_interval_ms(interval, self.minimum_ms);
            self.sensor.set_report_ms(ms).map_err(|e| e.to_string())
        }
    }

    impl<S: WindowsSensor> Drop for Running<S> {
        fn drop(&mut self) {
            *self.shared.lock().unwrap_or_else(|e| e.into_inner()) = None;
            let _ = self.sensor.detach(self.token);
            // 0 gives the sensor back its default rate, and lets Windows power it down.
            let _ = self.sensor.set_report_ms(0);
        }
    }

    /// Makes sure this thread can use Windows Runtime objects. Calling it again is harmless.
    fn com() {
        // SAFETY: joins this thread to the multithreaded apartment, which it keeps for its
        // life; a thread already in one is left as it is.
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    }

    fn describe<S: WindowsSensor>(kind: SensorKind) -> Option<HostSensorInfo> {
        let sensor = S::open().ok()?;
        let minimum_ms = sensor.minimum_ms().unwrap_or(16);
        Some(HostSensorInfo {
            kind,
            name: format!("Host {}", kind.name()),
            vendor: "DSA".into(),
            min_interval: Duration::from_millis(u64::from(minimum_ms)),
        })
    }

    fn start<S: WindowsSensor>(
        kind: SensorKind,
        interval: Duration,
        sink: ReadingSink,
    ) -> Result<Box<dyn RunningSensor>, String> {
        let name = kind.name();
        let sensor = match S::open() {
            Ok(sensor) => sensor,
            Err(e) => return Err(format!("this computer's {name} is gone: {e}")),
        };
        let minimum_ms = sensor.minimum_ms().unwrap_or(16);
        let ms = report_interval_ms(interval, minimum_ms);
        if let Err(e) = sensor.set_report_ms(ms) {
            return Err(format!("the {name} did not take a rate of {ms} ms: {e}"));
        }
        let shared: Shared = Arc::new(Mutex::new(Some(sink)));
        let token = match sensor.attach(Arc::clone(&shared)) {
            Ok(token) => token,
            Err(e) => {
                let _ = sensor.set_report_ms(0);
                return Err(format!("the {name} gives no readings: {e}"));
            }
        };
        Ok(Box::new(Running {
            sensor,
            token,
            minimum_ms,
            shared,
        }))
    }

    /// The computer's default sensors, opened when the guest asks for them.
    #[derive(Debug, Default, Clone, Copy)]
    pub struct WindowsSensors;

    impl HostSensors for WindowsSensors {
        fn available(&self) -> Vec<HostSensorInfo> {
            com();
            SensorKind::ALL
                .into_iter()
                .filter_map(|kind| match kind {
                    SensorKind::Accelerometer => describe::<Accelerometer>(kind),
                    SensorKind::Gyroscope => describe::<Gyrometer>(kind),
                    SensorKind::RotationVector => describe::<OrientationSensor>(kind),
                    SensorKind::Light => describe::<LightSensor>(kind),
                })
                .collect()
        }

        fn start(
            &self,
            kind: SensorKind,
            interval: Duration,
            sink: ReadingSink,
        ) -> Result<Box<dyn RunningSensor>, String> {
            com();
            match kind {
                SensorKind::Accelerometer => start::<Accelerometer>(kind, interval, sink),
                SensorKind::Gyroscope => start::<Gyrometer>(kind, interval, sink),
                SensorKind::RotationVector => start::<OrientationSensor>(kind, interval, sink),
                SensorKind::Light => start::<LightSensor>(kind, interval, sink),
            }
        }
    }
}
