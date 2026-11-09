//! Address lookups for the apps in the user's Android.
//!
//! The Linux desktop has no geocoder of its own: GeoClue finds positions, not addresses. The
//! follower answers from the administrative boundaries the package installs
//! (`/usr/lib/dsa/geodata/address-boundaries.bin`, read by
//! [`dsa_host_core::geocode::BoundaryGeocoder`]), as the Windows and macOS hosts do. Nothing is
//! sent over the network, so the position stays on the computer.

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
