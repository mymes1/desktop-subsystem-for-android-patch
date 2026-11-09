//! Address lookups for the apps in the user's Android.
//!
//! Windows' own geocoder (`Windows.Services.Maps.MapLocationFinder`) needs a Bing Maps key and
//! goes with Bing Maps for Enterprise, which Microsoft is retiring, so the window manager asks
//! OpenStreetMap's Nominatim instead ([`dsa_host_core::geocode::Nominatim`]), which needs no
//! key. It asks through WinHTTP, which follows the user's proxy settings, from the user's
//! session, one lookup at a time.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use dsa_host_core::geocode::{self, Nominatim};
use dsa_transport::Connector;

use crate::compositor::log::{note, CompositorLogger};
use crate::https::WinHttpFetch;

/// What the window manager calls itself to the service.
const BUILD: &str = "dsa-wm geocode";

/// How long one step of a lookup over the network may take. An app waits for the answer.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(15);

/// Answers the address lookups of the user's Android until `stop`.
pub fn serve(
    mut connector: Box<dyn Connector>,
    stop: Arc<AtomicBool>,
    logger: Option<Arc<CompositorLogger>>,
) {
    let mut geocoder = Nominatim::from_env(WinHttpFetch {
        timeout: LOOKUP_TIMEOUT,
    });
    let log = |line: &str| note(logger.as_deref(), line);
    geocode::serve(&mut *connector, BUILD, &mut geocoder, &stop, &log);
}
