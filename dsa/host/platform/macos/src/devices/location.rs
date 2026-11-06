//! The Mac's location through CoreLocation, for the guest's GNSS HAL.
//!
//! The location manager lives on the main thread, whose run loop delivers its fixes and
//! permission changes; they are passed on to the bridge's channel as they come.

use dsa_host_core::location::{Access, Fix};

/// What a `CLLocation` says, before it is turned into the guest's terms.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RawLocation {
    pub latitude: f64,
    pub longitude: f64,
    /// Negative when unknown, as every accuracy, speed and course CoreLocation reports.
    pub horizontal_accuracy: f64,
    pub altitude: f64,
    pub vertical_accuracy: f64,
    pub speed: f64,
    pub course: f64,
    /// Seconds since the Unix epoch.
    pub timestamp: f64,
}

/// The fix for `raw`, or `None` for one with no valid position.
pub fn fix(raw: &RawLocation) -> Option<Fix> {
    let valid = |v: f64| v.is_finite() && v >= 0.0;
    if !raw.latitude.is_finite()
        || !raw.longitude.is_finite()
        || raw.latitude.abs() > 90.0
        || raw.longitude.abs() > 180.0
        || !valid(raw.horizontal_accuracy)
    {
        return None;
    }
    let has_altitude = valid(raw.vertical_accuracy) && raw.altitude.is_finite();
    Some(Fix {
        latitude_degrees: raw.latitude,
        longitude_degrees: raw.longitude,
        horizontal_accuracy_meters: Some(raw.horizontal_accuracy as f32),
        altitude_meters: has_altitude.then_some(raw.altitude),
        vertical_accuracy_meters: has_altitude.then_some(raw.vertical_accuracy as f32),
        speed_meters_per_second: valid(raw.speed).then_some(raw.speed as f32),
        bearing_degrees: valid(raw.course).then_some((raw.course % 360.0) as f32),
        unix_time_ms: if raw.timestamp.is_finite() && raw.timestamp > 0.0 {
            (raw.timestamp * 1000.0) as u64
        } else {
            0
        },
    })
}

/// The access a `CLAuthorizationStatus` gives. `None` while the user has not been asked yet.
pub fn access(status: i32) -> Option<(Access, &'static str)> {
    match status {
        0 => None,
        1 => Some((
            Access::Denied,
            "location services are restricted on this Mac",
        )),
        2 => Some((
            Access::Denied,
            "DSA may not use the location; allow it in System Settings, Privacy & Security, \
             Location Services",
        )),
        3 | 4 => Some((Access::Allowed, "DSA may use the location")),
        _ => Some((
            Access::Unavailable,
            "location services report an unknown state",
        )),
    }
}

#[cfg(target_os = "macos")]
pub use provider::CoreLocation;

#[cfg(target_os = "macos")]
mod provider {
    use std::sync::mpsc::Sender;
    use std::sync::{Mutex, OnceLock};
    use std::time::Duration;

    use dsa_host_core::location::{Access, LocationEvent, LocationProvider};
    use objc2::encode::{Encode, Encoding, RefEncode};
    use objc2::msg_send;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject, ClassBuilder, Sel};
    use objc2::sel;

    use super::{access, fix, RawLocation};
    use crate::objc::{class, on_main};

    #[link(name = "CoreLocation", kind = "framework")]
    extern "C" {}

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Coordinate {
        latitude: f64,
        longitude: f64,
    }

    // CoreLocation declares the coordinate as an unnamed structure.
    unsafe impl Encode for Coordinate {
        const ENCODING: Encoding = Encoding::Struct("?", &[f64::ENCODING, f64::ENCODING]);
    }
    unsafe impl RefEncode for Coordinate {
        const ENCODING_REF: Encoding = Encoding::Pointer(&Self::ENCODING);
    }

    /// `kCLErrorDenied`.
    const ERROR_DENIED: isize = 1;
    /// `kCLErrorLocationUnknown`: no fix yet, which CoreLocation goes on looking for.
    const ERROR_UNKNOWN: isize = 0;

    static EVENTS: Mutex<Option<Sender<LocationEvent>>> = Mutex::new(None);

    fn emit(event: LocationEvent) {
        if let Ok(events) = EVENTS.lock() {
            if let Some(events) = events.as_ref() {
                let _ = events.send(event);
            }
        }
    }

    unsafe fn status_of(manager: *mut AnyObject) -> i32 {
        let responds: bool = msg_send![manager, respondsToSelector: sel!(authorizationStatus)];
        if responds {
            msg_send![manager, authorizationStatus]
        } else {
            msg_send![
                class(c"CLLocationManager").expect("checked"),
                authorizationStatus
            ]
        }
    }

    unsafe extern "C-unwind" fn did_update(
        _this: &AnyObject,
        _: Sel,
        _manager: *mut AnyObject,
        locations: *mut AnyObject,
    ) {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let location: *mut AnyObject = msg_send![locations, lastObject];
            if location.is_null() {
                return;
            }
            let coordinate: Coordinate = msg_send![location, coordinate];
            let date: *mut AnyObject = msg_send![location, timestamp];
            let raw = RawLocation {
                latitude: coordinate.latitude,
                longitude: coordinate.longitude,
                horizontal_accuracy: msg_send![location, horizontalAccuracy],
                altitude: msg_send![location, altitude],
                vertical_accuracy: msg_send![location, verticalAccuracy],
                speed: msg_send![location, speed],
                course: msg_send![location, course],
                timestamp: if date.is_null() {
                    0.0
                } else {
                    msg_send![date, timeIntervalSince1970]
                },
            };
            if let Some(fix) = fix(&raw) {
                emit(LocationEvent::Fix(fix));
            }
        }));
    }

    unsafe extern "C-unwind" fn did_change_authorization(
        _this: &AnyObject,
        _: Sel,
        manager: *mut AnyObject,
    ) {
        if let Some((access, detail)) = access(status_of(manager)) {
            emit(LocationEvent::Status {
                access,
                detail: detail.into(),
            });
        }
    }

    unsafe extern "C-unwind" fn did_fail(
        _this: &AnyObject,
        _: Sel,
        _manager: *mut AnyObject,
        error: *mut AnyObject,
    ) {
        let code: isize = if error.is_null() {
            -1
        } else {
            msg_send![error, code]
        };
        let event = match code {
            ERROR_UNKNOWN => return,
            ERROR_DENIED => LocationEvent::Status {
                access: Access::Denied,
                detail: "the user did not allow DSA to use the location".into(),
            },
            code => LocationEvent::Status {
                access: Access::Unavailable,
                detail: format!("location services failed (CoreLocation error {code})"),
            },
        };
        emit(event);
    }

    fn delegate_class() -> Result<&'static AnyClass, String> {
        static CLASS: OnceLock<Result<&'static AnyClass, String>> = OnceLock::new();
        CLASS
            .get_or_init(|| unsafe {
                let mut b = ClassBuilder::new(c"DSALocationDelegate", class(c"NSObject")?)
                    .ok_or("the location delegate class is already defined")?;
                b.add_method(
                    sel!(locationManager:didUpdateLocations:),
                    did_update as unsafe extern "C-unwind" fn(_, _, _, _),
                );
                b.add_method(
                    sel!(locationManagerDidChangeAuthorization:),
                    did_change_authorization as unsafe extern "C-unwind" fn(_, _, _),
                );
                b.add_method(
                    sel!(locationManager:didFailWithError:),
                    did_fail as unsafe extern "C-unwind" fn(_, _, _, _),
                );
                Ok(b.register())
            })
            .clone()
    }

    struct Running {
        manager: Retained<AnyObject>,
        _delegate: Retained<AnyObject>,
    }

    // Only touched on the main thread, where `on_main` runs.
    unsafe impl Send for Running {}

    static RUNNING: Mutex<Option<Running>> = Mutex::new(None);

    /// CoreLocation, as the guest's location bridge runs it.
    #[derive(Default)]
    pub struct CoreLocation;

    impl CoreLocation {
        pub fn new() -> CoreLocation {
            CoreLocation
        }
    }

    impl LocationProvider for CoreLocation {
        fn start(
            &mut self,
            interval: Duration,
            events: Sender<LocationEvent>,
        ) -> Result<(), (Access, String)> {
            let enabled: bool = unsafe {
                let class =
                    class(c"CLLocationManager").map_err(|why| (Access::Unavailable, why))?;
                msg_send![class, locationServicesEnabled]
            };
            if !enabled {
                return Err((
                    Access::Denied,
                    "location services are turned off on this Mac".into(),
                ));
            }
            delegate_class().map_err(|why| (Access::Unavailable, why))?;
            *EVENTS
                .lock()
                .map_err(|_| (Access::Unavailable, "lock failed".into()))? = Some(events);
            // A fix about as often as asked: move far enough at walking pace in the interval.
            let distance = (interval.as_secs_f64() * 1.5).max(1.0);
            on_main(move || unsafe {
                let (Ok(manager_class), Ok(delegate_class)) =
                    (class(c"CLLocationManager"), delegate_class())
                else {
                    return;
                };
                let manager: Option<Retained<AnyObject>> = msg_send![manager_class, new];
                let delegate: Option<Retained<AnyObject>> = msg_send![delegate_class, new];
                let (Some(manager), Some(delegate)) = (manager, delegate) else {
                    return;
                };
                let _: () = msg_send![&*manager, setDelegate: &*delegate];
                // kCLLocationAccuracyBest.
                let _: () = msg_send![&*manager, setDesiredAccuracy: -1.0f64];
                let _: () = msg_send![&*manager, setDistanceFilter: distance];
                let _: () = msg_send![&*manager, requestWhenInUseAuthorization];
                let _: () = msg_send![&*manager, startUpdatingLocation];
                if let Ok(mut running) = RUNNING.lock() {
                    *running = Some(Running {
                        manager,
                        _delegate: delegate,
                    });
                }
            });
            Ok(())
        }

        fn stop(&mut self) {
            if let Ok(mut events) = EVENTS.lock() {
                *events = None;
            }
            on_main(|| {
                let running = RUNNING.lock().ok().and_then(|mut r| r.take());
                if let Some(running) = running {
                    unsafe {
                        let _: () = msg_send![&*running.manager, stopUpdatingLocation];
                        let _: () = msg_send![&*running.manager, setDelegate: std::ptr::null::<AnyObject>()];
                    }
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seoul() -> RawLocation {
        RawLocation {
            latitude: 37.5665,
            longitude: 126.978,
            horizontal_accuracy: 35.0,
            altitude: 38.0,
            vertical_accuracy: 10.0,
            speed: -1.0,
            course: -1.0,
            timestamp: 1_700_000_000.5,
        }
    }

    #[test]
    fn fr26_a_fix_keeps_its_position_and_drops_what_is_unknown() {
        let fix = fix(&seoul()).unwrap();
        assert_eq!(fix.latitude_degrees, 37.5665);
        assert_eq!(fix.horizontal_accuracy_meters, Some(35.0));
        assert_eq!(fix.altitude_meters, Some(38.0));
        assert_eq!(fix.speed_meters_per_second, None);
        assert_eq!(fix.bearing_degrees, None);
        assert_eq!(fix.unix_time_ms, 1_700_000_000_500);
    }

    #[test]
    fn fr26_a_location_without_a_valid_position_is_not_a_fix() {
        let mut raw = seoul();
        raw.horizontal_accuracy = -1.0;
        assert_eq!(fix(&raw), None);
        let mut raw = seoul();
        raw.latitude = 91.0;
        assert_eq!(fix(&raw), None);
        let mut raw = seoul();
        raw.vertical_accuracy = -1.0;
        assert_eq!(fix(&raw).unwrap().altitude_meters, None);
    }

    #[test]
    fn fr26_the_users_permission_decides_access() {
        assert_eq!(access(0), None);
        assert_eq!(access(2).unwrap().0, Access::Denied);
        assert_eq!(access(1).unwrap().0, Access::Denied);
        assert_eq!(access(3).unwrap().0, Access::Allowed);
        assert_eq!(access(4).unwrap().0, Access::Allowed);
    }
}
