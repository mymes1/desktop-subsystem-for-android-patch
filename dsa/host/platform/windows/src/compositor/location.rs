//! The host's location for the user's Android, from the Windows location service.
//!
//! The service runs as the machine and cannot see the user's location privacy settings, so
//! the window manager, which runs in the user's session, gives Android the location. Windows
//! applies the user's settings to it as to any desktop app, and shows that location is in use
//! while it runs. It runs `Windows.Devices.Geolocation.Geolocator` only while the user's
//! Android wants a location ([`dsa_host_core::location::follow`]), and stops it as soon as
//! Android wants none.

use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Duration;

use dsa_host_core::location::{self, Access, Fix, LocationEvent, LocationProvider};
use dsa_transport::Connector;
use windows::Devices::Geolocation::{
    AltitudeReferenceSystem, GeolocationAccessStatus, Geolocator, Geoposition,
    PositionAccuracy, PositionChangedEventArgs, PositionStatus, StatusChangedEventArgs,
};
use windows::Foundation::{IReference, TypedEventHandler};

use crate::compositor::log::{note, CompositorLogger};

/// The build the window manager names itself by when it gives the location.
const BUILD: &str = "dsa-wm location";

/// Windows' clock counts 100 ns ticks from 1601; this many of them lie before 1970.
const UNIX_EPOCH_TICKS: i64 = 116_444_736_000_000_000;

/// Gives the user's Android the computer's location until `stop` is set.
pub fn follow(
    mut connector: Box<dyn Connector>,
    stop: Arc<AtomicBool>,
    logger: Option<Arc<CompositorLogger>>,
) {
    // The Windows location service is a WinRT object, used from this thread and its events.
    let _com = MultithreadedApartment::enter();
    let mut provider = WindowsGeolocator::default();
    let log = |line: &str| note(logger.as_deref(), line);
    location::follow(&mut *connector, BUILD, &mut provider, &stop, &log);
}

/// COM for this thread, in the multithreaded apartment, so waiting for an answer from the
/// location service blocks nothing it needs.
struct MultithreadedApartment {
    initialised: bool,
}

impl MultithreadedApartment {
    fn enter() -> MultithreadedApartment {
        use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
        // SAFETY: initialises COM for this thread; undone on drop when it succeeded.
        let result = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        MultithreadedApartment {
            initialised: result.is_ok(),
        }
    }
}

impl Drop for MultithreadedApartment {
    fn drop(&mut self) {
        if self.initialised {
            // SAFETY: balances the successful CoInitializeEx of this thread.
            unsafe { windows::Win32::System::Com::CoUninitialize() };
        }
    }
}

/// The Windows location service, while it runs for Android.
#[derive(Default)]
pub struct WindowsGeolocator {
    running: Option<Running>,
}

struct Running {
    geolocator: Geolocator,
    position_token: i64,
    status_token: i64,
}

impl LocationProvider for WindowsGeolocator {
    fn start(
        &mut self,
        interval: Duration,
        events: Sender<LocationEvent>,
    ) -> Result<(), (Access, String)> {
        self.stop();
        self.running = Some(open(interval, events)?);
        Ok(())
    }

    fn stop(&mut self) {
        // Without its handlers the Geolocator stops tracking, and Windows' indicator goes.
        if let Some(running) = self.running.take() {
            let _ = running
                .geolocator
                .RemovePositionChanged(running.position_token);
            let _ = running.geolocator.RemoveStatusChanged(running.status_token);
        }
    }
}

impl Drop for WindowsGeolocator {
    fn drop(&mut self) {
        self.stop();
    }
}

fn unavailable(what: &str, error: windows::core::Error) -> (Access, String) {
    (Access::Unavailable, format!("{what}: {error}"))
}

/// Asks Windows for the location, and starts the Geolocator with its handlers.
fn open(interval: Duration, events: Sender<LocationEvent>) -> Result<Running, (Access, String)> {
    let access = Geolocator::RequestAccessAsync()
        .and_then(|asked| asked.join())
        .map_err(|e| unavailable("asking Windows for the location", e))?;
    if access == GeolocationAccessStatus::Denied {
        let why = "location is off for desktop apps in Windows' privacy settings";
        return Err((Access::Denied, why.to_string()));
    }
    if access != GeolocationAccessStatus::Allowed {
        let why = "Windows did not say whether DSA may use the location";
        return Err((Access::Unavailable, why.to_string()));
    }
    let geolocator =
        Geolocator::new().map_err(|e| unavailable("opening the location service", e))?;
    geolocator
        .SetDesiredAccuracy(PositionAccuracy::High)
        .map_err(|e| unavailable("setting the accuracy", e))?;
    let interval_ms = u32::try_from(interval.as_millis()).unwrap_or(u32::MAX);
    geolocator
        .SetReportInterval(interval_ms)
        .map_err(|e| unavailable("setting the interval", e))?;

    let statuses = events.clone();
    let on_status = TypedEventHandler::<Geolocator, StatusChangedEventArgs>::new(
        move |_, args| {
            if let Some(args) = args.as_ref() {
                if let Some(event) = status_event(args.Status()?) {
                    let _ = statuses.send(event);
                }
            }
            Ok(())
        },
    );
    let status_token = geolocator
        .StatusChanged(&on_status)
        .map_err(|e| unavailable("following the location service", e))?;
    let on_position = TypedEventHandler::<Geolocator, PositionChangedEventArgs>::new(
        move |_, args| {
            if let Some(args) = args.as_ref() {
                let _ = events.send(LocationEvent::Fix(fix_of(&args.Position()?)?));
            }
            Ok(())
        },
    );
    let position_token = match geolocator.PositionChanged(&on_position) {
        Ok(token) => token,
        Err(e) => {
            let _ = geolocator.RemoveStatusChanged(status_token);
            return Err(unavailable("following the location", e));
        }
    };
    Ok(Running {
        geolocator,
        position_token,
        status_token,
    })
}

/// What a change of the location service's state tells Android, if anything.
pub fn status_event(status: PositionStatus) -> Option<LocationEvent> {
    let (access, detail) = match status {
        PositionStatus::Ready => (Access::Allowed, ""),
        PositionStatus::Disabled => (
            Access::Denied,
            "location was turned off in Windows' privacy settings",
        ),
        PositionStatus::NotAvailable => (
            Access::Unavailable,
            "this computer has no location service",
        ),
        _ => return None,
    };
    Some(LocationEvent::Status {
        access,
        detail: detail.to_string(),
    })
}

/// A Windows time (100 ns ticks since 1601, UTC) in milliseconds since the Unix epoch.
pub fn unix_time_ms(universal_time: i64) -> u64 {
    u64::try_from((universal_time - UNIX_EPOCH_TICKS) / 10_000).unwrap_or(0)
}

/// A value Windows may leave out, if it gave one.
fn known(value: windows::core::Result<IReference<f64>>) -> Option<f64> {
    value
        .ok()
        .and_then(|value| value.Value().ok())
        .filter(|value| value.is_finite())
}

/// A position from Windows, with the values it knows.
fn fix_of(position: &Geoposition) -> windows::core::Result<Fix> {
    let coordinate = position.Coordinate()?;
    let point = coordinate.Point()?;
    let place = point.Position()?;
    let altitude_accuracy = known(coordinate.AltitudeAccuracy());
    let has_altitude = altitude_accuracy.is_some()
        || point.AltitudeReferenceSystem()? != AltitudeReferenceSystem::Unspecified;
    Ok(Fix {
        latitude_degrees: place.Latitude,
        longitude_degrees: place.Longitude,
        horizontal_accuracy_meters: Some(coordinate.Accuracy()? as f32),
        altitude_meters: has_altitude.then_some(place.Altitude),
        vertical_accuracy_meters: altitude_accuracy.map(|a| a as f32),
        speed_meters_per_second: known(coordinate.Speed()).map(|s| s as f32),
        bearing_degrees: known(coordinate.Heading()).map(|h| h as f32),
        unix_time_ms: unix_time_ms(coordinate.Timestamp()?.UniversalTime),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr26_windows_time_becomes_unix_time() {
        assert_eq!(unix_time_ms(UNIX_EPOCH_TICKS), 0);
        let later = UNIX_EPOCH_TICKS + 1_700_000_000_000 * 10_000;
        assert_eq!(unix_time_ms(later), 1_700_000_000_000);
        assert_eq!(unix_time_ms(0), 0);
    }

    #[test]
    fn fr26_turning_location_off_in_windows_is_reported_as_denied() {
        match status_event(PositionStatus::Disabled) {
            Some(LocationEvent::Status { access, .. }) => assert_eq!(access, Access::Denied),
            other => panic!("expected a denied status, got {other:?}"),
        }
        match status_event(PositionStatus::Ready) {
            Some(LocationEvent::Status { access, .. }) => assert_eq!(access, Access::Allowed),
            other => panic!("expected an allowed status, got {other:?}"),
        }
        assert_eq!(status_event(PositionStatus::Initializing), None);
    }
}
