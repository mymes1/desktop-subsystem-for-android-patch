//! Address lookups for the apps in the user's Android.
//!
//! The device lender answers them from the administrative boundaries in the bundle
//! (`DSA.app/Contents/Resources/geodata/address-boundaries.bin`, read by
//! [`dsa_host_core::geocode::BoundaryGeocoder`]), as the Windows and Linux hosts do. The Mac's
//! own geocoder (`CLGeocoder`) is not used: it sends every position to Apple's servers, and
//! the position is to stay on the Mac.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use dsa_host_core::geocode::{self, BoundaryGeocoder};
use dsa_transport::Connector;

use super::Log;

/// The build the follower names itself by.
const BUILD: &str = "dsa-devices geocode";

/// Answers the address lookups of the user's Android until `stop`.
pub fn serve(mut connector: Box<dyn Connector>, stop: Arc<AtomicBool>, log: Log) {
    let mut geocoder = BoundaryGeocoder::for_this_program();
    log(&format!(
        "geocoding: answering from {}",
        geocoder.path().display()
    ));
    let line = move |line: &str| log(line);
    geocode::serve(&mut *connector, BUILD, &mut geocoder, &stop, &line);
}
