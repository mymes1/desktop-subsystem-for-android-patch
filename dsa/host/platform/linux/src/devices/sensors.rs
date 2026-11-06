//! The computer's sensors for the apps in the guest: the kernel's IIO devices, and
//! iio-sensor-proxy where the kernel's are not readable.
//!
//! A convertible or tablet carries an accelerometer, often a gyroscope, a light sensor and a
//! fused orientation, which the kernel shows under `/sys/bus/iio/devices`. Each is read in
//! the units IIO gives once its scale and offset are applied (m/s^2, rad/s, lux, a unit
//! quaternion), turned by the mount matrix the firmware gives so its axes are the screen's,
//! and handed on in the host's units, which the core turns into Android's. A sensor is read
//! only while an app in the guest listens to it.
//!
//! Where iio-sensor-proxy holds the devices (it buffers them, and some drivers then refuse
//! direct reads), the light level and the accelerometer's orientation come from the proxy
//! instead: the light in lux, and gravity along the screen edge that points down.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use dsa_host_core::sensors::{
    HostReading, HostSensorInfo, HostSensors, ReadingSink, RunningSensor, SensorKind,
    STANDARD_GRAVITY,
};
use zbus::blocking::Connection;

use super::dbus::proxy;

const IIO_DEVICES: &str = "/sys/bus/iio/devices";

const PROXY_SERVICE: &str = "net.hadess.SensorProxy";
const PROXY_PATH: &str = "/net/hadess/SensorProxy";
const PROXY: &str = "net.hadess.SensorProxy";

/// Where a sensor's readings come from.
#[derive(Debug, Clone, PartialEq)]
enum Source {
    /// An IIO device's folder, and the channel prefix (`in_accel`, `in_anglvel`,
    /// `in_illuminance`, `in_rot_quaternion`).
    Iio { dir: PathBuf, channel: &'static str },
    /// iio-sensor-proxy.
    Proxy,
}

/// The computer's sensors.
#[derive(Default)]
pub struct LinuxSensors;

impl LinuxSensors {
    pub fn new() -> LinuxSensors {
        LinuxSensors
    }
}

/// The IIO channel prefix for a kind of sensor.
fn channel(kind: SensorKind) -> &'static str {
    match kind {
        SensorKind::Accelerometer => "in_accel",
        SensorKind::Gyroscope => "in_anglvel",
        SensorKind::RotationVector => "in_rot_quaternion",
        SensorKind::Light => "in_illuminance",
    }
}

fn read_text(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().map(|t| t.trim().to_string())
}

fn read_number(path: &Path) -> Option<f64> {
    read_text(path)?.parse().ok()
}

/// A channel's scale: its own, else its type's, else 1.
fn scale(dir: &Path, prefix: &str, axis: &str) -> f64 {
    read_number(&dir.join(format!("{prefix}_{axis}_scale")))
        .or_else(|| read_number(&dir.join(format!("{prefix}_scale"))))
        .unwrap_or(1.0)
}

fn offset(dir: &Path, prefix: &str, axis: &str) -> f64 {
    read_number(&dir.join(format!("{prefix}_{axis}_offset")))
        .or_else(|| read_number(&dir.join(format!("{prefix}_offset"))))
        .unwrap_or(0.0)
}

/// One axis of a channel, in IIO's units.
fn axis_value(dir: &Path, prefix: &str, axis: &str) -> Option<f64> {
    if let Some(input) = read_number(&dir.join(format!("{prefix}_{axis}_input"))) {
        return Some(input);
    }
    let raw = read_number(&dir.join(format!("{prefix}_{axis}_raw")))?;
    Some((raw + offset(dir, prefix, axis)) * scale(dir, prefix, axis))
}

/// Parses a mount matrix (`"x1, y1, z1; x2, y2, z2; x3, y3, z3"`).
pub fn parse_mount_matrix(text: &str) -> Option<[[f64; 3]; 3]> {
    let rows: Vec<Vec<f64>> = text
        .split(';')
        .map(|row| {
            row.split(',')
                .filter_map(|v| v.trim().parse().ok())
                .collect()
        })
        .collect();
    if rows.len() != 3 || rows.iter().any(|r| r.len() != 3) {
        return None;
    }
    let mut m = [[0.0; 3]; 3];
    for (i, row) in rows.iter().enumerate() {
        m[i].copy_from_slice(row);
    }
    Some(m)
}

fn mount_matrix(dir: &Path, prefix: &str) -> [[f64; 3]; 3] {
    read_text(&dir.join(format!("{prefix}_mount_matrix")))
        .or_else(|| read_text(&dir.join("mount_matrix")))
        .and_then(|t| parse_mount_matrix(&t))
        .unwrap_or([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]])
}

fn turn(m: &[[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    [0, 1, 2].map(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2])
}

/// A reading of the sensor at `source`.
fn read(
    kind: SensorKind,
    source: &Source,
    sensor_proxy: Option<&zbus::blocking::Proxy<'_>>,
) -> Option<HostReading> {
    match source {
        Source::Iio { dir, channel } => read_iio(kind, dir, channel),
        Source::Proxy => read_proxy(kind, sensor_proxy?),
    }
}

fn read_iio(kind: SensorKind, dir: &Path, prefix: &str) -> Option<HostReading> {
    match kind {
        SensorKind::Accelerometer | SensorKind::Gyroscope => {
            let v = [
                axis_value(dir, prefix, "x")?,
                axis_value(dir, prefix, "y")?,
                axis_value(dir, prefix, "z")?,
            ];
            let [x, y, z] = turn(&mount_matrix(dir, prefix), v);
            Some(if kind == SensorKind::Accelerometer {
                // IIO gives m/s^2; the host's unit is g.
                HostReading::Acceleration {
                    x: x / STANDARD_GRAVITY,
                    y: y / STANDARD_GRAVITY,
                    z: z / STANDARD_GRAVITY,
                }
            } else {
                // IIO gives rad/s; the host's unit is degrees a second.
                HostReading::AngularVelocity {
                    x: x.to_degrees(),
                    y: y.to_degrees(),
                    z: z.to_degrees(),
                }
            })
        }
        SensorKind::Light => {
            let lux = read_number(&dir.join(format!("{prefix}_input"))).or_else(|| {
                let raw = read_number(&dir.join(format!("{prefix}_raw")))?;
                Some(raw * read_number(&dir.join(format!("{prefix}_scale"))).unwrap_or(1.0))
            })?;
            Some(HostReading::Illuminance { lux })
        }
        SensorKind::RotationVector => {
            // Four components, "x y z w", one scale for all.
            let text = read_text(&dir.join(format!("{prefix}_raw")))?;
            let values: Vec<f64> = text
                .split_whitespace()
                .filter_map(|v| v.parse().ok())
                .collect();
            let [x, y, z, w] = <[f64; 4]>::try_from(values).ok()?;
            let s = read_number(&dir.join(format!("{prefix}_scale"))).unwrap_or(1.0);
            Some(HostReading::Orientation {
                w: w * s,
                x: x * s,
                y: y * s,
                z: z * s,
            })
        }
    }
}

/// Gravity, in the host's convention (a screen lying face up reads z = -1), for the
/// orientation iio-sensor-proxy names.
pub fn gravity_for(orientation: &str) -> Option<HostReading> {
    let (x, y) = match orientation {
        "normal" => (0.0, -1.0),
        "bottom-up" => (0.0, 1.0),
        "left-up" => (1.0, 0.0),
        "right-up" => (-1.0, 0.0),
        _ => return None,
    };
    Some(HostReading::Acceleration { x, y, z: 0.0 })
}

fn read_proxy(kind: SensorKind, proxy: &zbus::blocking::Proxy<'_>) -> Option<HostReading> {
    match kind {
        SensorKind::Light => {
            let level: f64 = proxy.get_property("LightLevel").ok()?;
            let unit: String = proxy.get_property("LightLevelUnit").ok()?;
            (unit == "lux").then_some(HostReading::Illuminance { lux: level })
        }
        SensorKind::Accelerometer => {
            let orientation: String = proxy.get_property("AccelerometerOrientation").ok()?;
            gravity_for(&orientation)
        }
        _ => None,
    }
}

/// The IIO devices that give `kind`, by their folders.
fn iio_sources(kind: SensorKind) -> Vec<Source> {
    let prefix = channel(kind);
    let probe = match kind {
        SensorKind::Accelerometer | SensorKind::Gyroscope => format!("{prefix}_x_raw"),
        SensorKind::RotationVector => format!("{prefix}_raw"),
        SensorKind::Light => format!("{prefix}_raw"),
    };
    let mut found: Vec<PathBuf> = fs::read_dir(IIO_DEVICES)
        .map(|dir| dir.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    found.sort();
    found
        .into_iter()
        .filter(|dir| {
            dir.join(&probe).exists()
                || (kind == SensorKind::Light && dir.join(format!("{prefix}_input")).exists())
        })
        .map(|dir| Source::Iio {
            dir,
            channel: prefix,
        })
        .collect()
}

fn proxy_has(kind: SensorKind) -> bool {
    let property = match kind {
        SensorKind::Accelerometer => "HasAccelerometer",
        SensorKind::Light => "HasAmbientLight",
        _ => return false,
    };
    let Ok(connection) = Connection::system() else {
        return false;
    };
    proxy(&connection, PROXY_SERVICE, PROXY_PATH.into(), PROXY)
        .ok()
        .and_then(|p| p.get_property::<bool>(property).ok())
        .unwrap_or(false)
}

/// The source to read `kind` from: an IIO device that reads now, else the proxy.
fn source_for(kind: SensorKind) -> Option<(Source, String)> {
    for source in iio_sources(kind) {
        if let Source::Iio { dir, .. } = &source {
            if read_iio(kind, dir, channel(kind)).is_some() {
                let name = read_text(&dir.join("name")).unwrap_or_else(|| kind.name().into());
                return Some((source, name));
            }
        }
    }
    proxy_has(kind).then(|| (Source::Proxy, format!("{} (iio-sensor-proxy)", kind.name())))
}

impl HostSensors for LinuxSensors {
    fn available(&self) -> Vec<HostSensorInfo> {
        SensorKind::ALL
            .iter()
            .filter_map(|&kind| {
                let (source, name) = source_for(kind)?;
                let min_interval = match (kind, &source) {
                    (SensorKind::Light, _) | (_, Source::Proxy) => Duration::from_millis(100),
                    _ => Duration::from_millis(10),
                };
                Some(HostSensorInfo {
                    kind,
                    name,
                    vendor: "Linux IIO".into(),
                    min_interval,
                })
            })
            .collect()
    }

    fn start(
        &self,
        kind: SensorKind,
        interval: Duration,
        sink: ReadingSink,
    ) -> Result<Box<dyn RunningSensor>, String> {
        let (source, _) =
            source_for(kind).ok_or_else(|| format!("this computer has no {}", kind.name()))?;
        let stop = Arc::new(AtomicBool::new(false));
        let micros = Arc::new(AtomicU64::new(interval.as_micros() as u64));
        let (thread_stop, thread_micros) = (Arc::clone(&stop), Arc::clone(&micros));
        let thread = thread::Builder::new()
            .name(format!("dsa-sensor-{}", channel(kind)))
            .spawn(move || {
                // The proxy hands out readings only to a client that claimed the sensor,
                // for as long as its connection lasts.
                let connection = (source == Source::Proxy)
                    .then(Connection::system)
                    .and_then(Result::ok);
                let sensor_proxy = connection
                    .as_ref()
                    .and_then(|c| proxy(c, PROXY_SERVICE, PROXY_PATH.into(), PROXY).ok());
                if let Some(proxy) = &sensor_proxy {
                    let claim = match kind {
                        SensorKind::Light => "ClaimLight",
                        _ => "ClaimAccelerometer",
                    };
                    let _ = proxy.call::<_, _, ()>(claim, &());
                }
                while !thread_stop.load(Ordering::Relaxed) {
                    if let Some(reading) = read(kind, &source, sensor_proxy.as_ref()) {
                        sink(reading);
                    }
                    let wait = Duration::from_micros(thread_micros.load(Ordering::Relaxed));
                    thread::sleep(wait.max(Duration::from_millis(5)));
                }
                if let Some(proxy) = &sensor_proxy {
                    let release = match kind {
                        SensorKind::Light => "ReleaseLight",
                        _ => "ReleaseAccelerometer",
                    };
                    let _ = proxy.call::<_, _, ()>(release, &());
                }
            })
            .map_err(|e| format!("cannot start the sensor's thread: {e}"))?;
        Ok(Box::new(Running {
            stop,
            micros,
            thread: Some(thread),
        }))
    }
}

struct Running {
    stop: Arc<AtomicBool>,
    micros: Arc<AtomicU64>,
    thread: Option<JoinHandle<()>>,
}

impl RunningSensor for Running {
    fn set_interval(&mut self, interval: Duration) -> Result<(), String> {
        self.micros
            .store(interval.as_micros() as u64, Ordering::Relaxed);
        Ok(())
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_device(files: &[(&str, &str)]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "dsa-iio-{}-{}",
            std::process::id(),
            files.len() * 31 + files[0].0.len()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for (name, value) in files {
            fs::write(dir.join(name), value).unwrap();
        }
        dir
    }

    #[test]
    fn fr26_an_iio_accelerometer_reads_in_g_on_the_screens_axes() {
        let dir = fake_device(&[
            ("in_accel_x_raw", "0\n"),
            ("in_accel_y_raw", "100\n"),
            ("in_accel_z_raw", "-1000\n"),
            ("in_accel_scale", "0.0098066500\n"),
            ("in_accel_mount_matrix", "0, 1, 0; 1, 0, 0; 0, 0, 1\n"),
        ]);
        let reading = read_iio(SensorKind::Accelerometer, &dir, "in_accel").unwrap();
        let HostReading::Acceleration { x, y, z } = reading else {
            panic!("not an acceleration: {reading:?}");
        };
        assert!((x - 0.1).abs() < 1e-6, "{x}");
        assert!(y.abs() < 1e-6, "{y}");
        assert!((z + 1.0).abs() < 1e-6, "{z}");
        let android = reading.to_android().unwrap();
        assert!((android[2] - 9.80665).abs() < 1e-3);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn fr26_an_iio_light_sensor_reads_in_lux() {
        let dir = fake_device(&[("in_illuminance_raw", "250"), ("in_illuminance_scale", "2")]);
        assert_eq!(
            read_iio(SensorKind::Light, &dir, "in_illuminance"),
            Some(HostReading::Illuminance { lux: 500.0 })
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn fr26_the_proxys_orientation_reads_as_gravity() {
        assert_eq!(
            gravity_for("normal"),
            Some(HostReading::Acceleration {
                x: 0.0,
                y: -1.0,
                z: 0.0
            })
        );
        assert_eq!(gravity_for("undefined"), None);
    }

    #[test]
    fn fr26_mount_matrices_are_read_and_bad_ones_refused() {
        assert_eq!(
            parse_mount_matrix("1, 0, 0; 0, -1, 0; 0, 0, 1"),
            Some([[1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0]])
        );
        assert_eq!(parse_mount_matrix("1, 0; 0, 1"), None);
    }
}
