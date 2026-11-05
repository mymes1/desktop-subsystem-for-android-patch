//! The computer's location for the user's Android, from GeoClue2.
//!
//! GeoClue2 is the location service of the Linux desktops. It asks the desktop's agent (the
//! one that shows the location switch and the per-app prompt), so the user's own location
//! settings apply to DSA as to any app. DSA names itself `dsa`, the desktop file the package
//! installs. The client runs only while Android wants a location
//! ([`dsa_host_core::location::follow`]) and is stopped as soon as it does not.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dsa_host_core::location::{self, Access, Fix, LocationEvent, LocationProvider};
use dsa_transport::Connector;
use zbus::blocking::Connection;
use zbus::zvariant::OwnedObjectPath;

use super::dbus::{failure_of, proxy, Failure};
use super::Log;

const SERVICE: &str = "org.freedesktop.GeoClue2";
const MANAGER_PATH: &str = "/org/freedesktop/GeoClue2/Manager";
const MANAGER: &str = "org.freedesktop.GeoClue2.Manager";
const CLIENT: &str = "org.freedesktop.GeoClue2.Client";
const LOCATION: &str = "org.freedesktop.GeoClue2.Location";

/// The desktop file DSA is known to GeoClue by.
pub const DESKTOP_ID: &str = "dsa";

/// `GCLUE_ACCURACY_LEVEL_EXACT`: as exact as the computer can tell.
const ACCURACY_EXACT: u32 = 8;

/// The build the follower names itself by.
const BUILD: &str = "dsa-devices location";

/// Gives the user's Android the computer's location until `stop` is set.
pub fn follow(mut connector: Box<dyn Connector>, stop: Arc<AtomicBool>, log: Log) {
    let mut provider = GeoClue::default();
    let log = move |line: &str| log(line);
    location::follow(&mut *connector, BUILD, &mut provider, &stop, &log);
}

/// GeoClue2's client, running while Android wants a location.
#[derive(Default)]
pub struct GeoClue {
    running: Option<(Arc<AtomicBool>, JoinHandle<()>)>,
}

/// GeoClue says "unknown" with these.
fn known(value: f64, unknown_below: f64) -> Option<f64> {
    (value.is_finite() && value > unknown_below).then_some(value)
}

/// The fix GeoClue's location object holds, from its properties.
pub fn fix_from(
    latitude: f64,
    longitude: f64,
    accuracy: f64,
    altitude: f64,
    speed: f64,
    heading: f64,
    unix_time_ms: u64,
) -> Fix {
    Fix {
        latitude_degrees: latitude,
        longitude_degrees: longitude,
        horizontal_accuracy_meters: known(accuracy, 0.0).map(|a| a as f32),
        // GeoClue gives -DBL_MAX for an altitude it does not know.
        altitude_meters: known(altitude, -1.0e300),
        vertical_accuracy_meters: None,
        // And -1 for a speed or heading it does not know.
        speed_meters_per_second: known(speed, -0.5).map(|s| s as f32),
        bearing_degrees: known(heading, -0.5).map(|h| h as f32),
        unix_time_ms,
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn access_of(why: &str) -> Access {
    match failure_of(why) {
        Failure::Denied => Access::Denied,
        Failure::Unavailable => Access::Unavailable,
    }
}

/// Reads the location object at `path`.
fn read_fix(connection: &Connection, path: &str) -> Result<Fix, String> {
    let location = proxy(connection, SERVICE, path.to_string(), LOCATION)?;
    let read = |name: &str| -> Result<f64, String> {
        location
            .get_property::<f64>(name)
            .map_err(|e| format!("reading the location's {name}: {e}"))
    };
    Ok(fix_from(
        read("Latitude")?,
        read("Longitude")?,
        read("Accuracy").unwrap_or(-1.0),
        read("Altitude").unwrap_or(f64::MIN),
        read("Speed").unwrap_or(-1.0),
        read("Heading").unwrap_or(-1.0),
        now_ms(),
    ))
}

impl LocationProvider for GeoClue {
    fn start(
        &mut self,
        interval: Duration,
        events: Sender<LocationEvent>,
    ) -> Result<(), (Access, String)> {
        self.stop();
        let unavailable = |why: String| (Access::Unavailable, why);
        let connection = Connection::system()
            .map_err(|e| unavailable(format!("the system bus cannot be reached: {e}")))?;
        let manager =
            proxy(&connection, SERVICE, MANAGER_PATH.to_string(), MANAGER).map_err(unavailable)?;
        let client_path: OwnedObjectPath = manager.call("GetClient", &()).map_err(|e| {
            let why = format!("GeoClue gives DSA no location client: {e}");
            (access_of(&why), why)
        })?;
        let client_path = client_path.as_str().to_string();
        let client =
            proxy(&connection, SERVICE, client_path.clone(), CLIENT).map_err(unavailable)?;
        let seconds = u32::try_from(interval.as_secs()).unwrap_or(u32::MAX);
        let setup = client
            .set_property("DesktopId", DESKTOP_ID)
            .and_then(|()| client.set_property("RequestedAccuracyLevel", ACCURACY_EXACT))
            .and_then(|()| client.set_property("TimeThreshold", seconds));
        if let Err(e) = setup {
            let why = format!("GeoClue's client cannot be set up: {e}");
            return Err((access_of(&why), why));
        }
        client.call::<_, _, ()>("Start", &()).map_err(|e| {
            let why = format!("GeoClue does not give DSA the location: {e}");
            (access_of(&why), why)
        })?;

        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let poll = interval.clamp(Duration::from_millis(200), Duration::from_secs(1));
        let thread = thread::Builder::new()
            .name("dsa-geoclue".into())
            .spawn(move || {
                let mut last = String::new();
                while !thread_stop.load(Ordering::Relaxed) {
                    match client.get_property::<OwnedObjectPath>("Location") {
                        Ok(path) if path.as_str() != "/" && path.as_str() != last => {
                            last = path.as_str().to_string();
                            match read_fix(&connection, &last) {
                                Ok(fix) => {
                                    if events.send(LocationEvent::Fix(fix)).is_err() {
                                        break;
                                    }
                                }
                                Err(why) => eprintln!("location: {why}"),
                            }
                        }
                        Ok(_) => {}
                        Err(e) => {
                            let why = format!("GeoClue's client ended: {e}");
                            let _ = events.send(LocationEvent::Status {
                                access: access_of(&why),
                                detail: why,
                            });
                            break;
                        }
                    }
                    thread::sleep(poll);
                }
                let _ = client.call::<_, _, ()>("Stop", &());
            })
            .map_err(|e| unavailable(format!("cannot start the location's thread: {e}")))?;
        self.running = Some((stop, thread));
        Ok(())
    }

    fn stop(&mut self) {
        if let Some((stop, thread)) = self.running.take() {
            stop.store(true, Ordering::Relaxed);
            let _ = thread.join();
        }
    }
}

impl Drop for GeoClue {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr26_geoclue_values_it_does_not_know_are_left_out() {
        let fix = fix_from(37.5, 127.0, 25.0, f64::MIN, -1.0, -1.0, 1_000);
        assert_eq!(fix.latitude_degrees, 37.5);
        assert_eq!(fix.longitude_degrees, 127.0);
        assert_eq!(fix.horizontal_accuracy_meters, Some(25.0));
        assert_eq!(fix.altitude_meters, None);
        assert_eq!(fix.speed_meters_per_second, None);
        assert_eq!(fix.bearing_degrees, None);
        assert!(fix.checked().is_ok());
    }

    #[test]
    fn fr26_geoclue_values_it_knows_reach_the_guest() {
        let fix = fix_from(-33.9, 151.2, 5.0, 40.0, 0.0, 90.0, 7);
        assert_eq!(fix.altitude_meters, Some(40.0));
        assert_eq!(fix.speed_meters_per_second, Some(0.0));
        assert_eq!(fix.bearing_degrees, Some(90.0));
        assert_eq!(fix.unix_time_ms, 7);
    }

    #[test]
    fn fr26_a_refused_client_is_denied_access() {
        assert_eq!(
            access_of("org.freedesktop.DBus.Error.AccessDenied: 'dsa' disallowed, no agent"),
            Access::Denied
        );
        assert_eq!(
            access_of("org.freedesktop.DBus.Error.ServiceUnknown"),
            Access::Unavailable
        );
    }
}
