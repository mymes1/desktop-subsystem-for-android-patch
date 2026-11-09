//! Address lookups for the apps in the user's Android, through the Mac's own geocoder.
//!
//! macOS looks addresses up with `CLGeocoder`, Apple's service, which needs no key and gives
//! the same answers as Maps. The follower watches the lookups over the local API
//! ([`dsa_host_core::geocode::serve`]) and asks `CLGeocoder` on the main thread, where its
//! answers come; what it found goes back to Android as addresses. Apple limits how often one
//! program may ask; a lookup past that limit, like one without a network, fails as Android's
//! "service not available".

use dsa_host_core::geocode::{Address, GeocodeError, GeocodeErrorKind};

/// What a `CLPlacemark` says, as plain text. Empty strings are parts it does not give.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RawPlacemark {
    pub name: String,
    pub sub_thoroughfare: String,
    pub thoroughfare: String,
    pub sub_locality: String,
    pub locality: String,
    pub sub_administrative_area: String,
    pub administrative_area: String,
    pub postal_code: String,
    pub iso_country_code: String,
    pub country: String,
    /// Latitude and longitude, when the placemark has a location.
    pub position: Option<(f64, f64)>,
}

/// The placemark as an address, with one address line made of its parts in the order a
/// postal address reads.
pub fn address(raw: &RawPlacemark) -> Address {
    let street = [raw.sub_thoroughfare.as_str(), raw.thoroughfare.as_str()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let mut parts: Vec<&str> = Vec::new();
    if !raw.name.is_empty() && raw.name != street {
        parts.push(&raw.name);
    }
    for part in [
        street.as_str(),
        raw.sub_locality.as_str(),
        raw.locality.as_str(),
        raw.administrative_area.as_str(),
        raw.postal_code.as_str(),
        raw.country.as_str(),
    ] {
        if !part.is_empty() && !parts.contains(&part) {
            parts.push(part);
        }
    }
    Address {
        address_lines: vec![parts.join(", ")],
        feature_name: raw.name.clone(),
        premises: String::new(),
        sub_thoroughfare: raw.sub_thoroughfare.clone(),
        thoroughfare: raw.thoroughfare.clone(),
        sub_locality: raw.sub_locality.clone(),
        locality: raw.locality.clone(),
        sub_admin_area: raw.sub_administrative_area.clone(),
        admin_area: raw.administrative_area.clone(),
        postal_code: raw.postal_code.clone(),
        country_code: raw.iso_country_code.clone(),
        country_name: raw.country.clone(),
        position: raw.position,
    }
    .checked()
}

/// `kCLErrorDenied`.
const ERROR_DENIED: isize = 1;
/// `kCLErrorNetwork`, which is also what CLGeocoder answers past Apple's limit.
const ERROR_NETWORK: isize = 2;
/// `kCLErrorGeocodeFoundNoResult`.
const ERROR_NO_RESULT: isize = 8;
/// `kCLErrorGeocodeFoundPartialResult`.
const ERROR_PARTIAL_RESULT: isize = 9;

/// What a `CLError` code means for the lookup: `None` when it means no place was found, which
/// is an empty answer rather than a failure.
pub fn failure(code: isize, description: &str) -> Option<GeocodeError> {
    let kind = match code {
        ERROR_NO_RESULT | ERROR_PARTIAL_RESULT => return None,
        ERROR_DENIED => GeocodeErrorKind::Refused,
        ERROR_NETWORK if description.to_ascii_lowercase().contains("throttl") => {
            GeocodeErrorKind::Busy
        }
        _ => GeocodeErrorKind::Unavailable,
    };
    Some(GeocodeError::new(
        kind,
        format!("the Mac's geocoder failed (CLError {code}): {description}"),
    ))
}

#[cfg(target_os = "macos")]
pub use mac::{serve, CoreLocationGeocoder};

#[cfg(target_os = "macos")]
mod mac {
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc::channel;
    use std::sync::Arc;
    use std::time::Duration;

    use block2::RcBlock;
    use dsa_host_core::geocode::{
        self, GeocodeError, GeocodeErrorKind, GeocodeOutcome, GeocodeQuery, Geocoder, Lookup,
    };
    use dsa_transport::Connector;
    use objc2::encode::{Encode, Encoding, RefEncode};
    use objc2::msg_send;
    use objc2::rc::{autoreleasepool, Allocated, Retained};
    use objc2::runtime::AnyObject;

    use super::{address, failure, RawPlacemark};
    use crate::devices::Log;
    use crate::objc::{class, ns_string, on_main, string_of};

    #[link(name = "CoreLocation", kind = "framework")]
    extern "C" {}

    /// What the follower calls itself to the service.
    const BUILD: &str = "dsa-devices geocode";

    /// How long one lookup waits for CLGeocoder.
    const ANSWER: Duration = Duration::from_secs(30);

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

    /// Answers the address lookups of the user's Android until `stop`.
    pub fn serve(mut connector: Box<dyn Connector>, stop: Arc<AtomicBool>, log: Log) {
        let mut geocoder = CoreLocationGeocoder;
        let line = move |line: &str| log(line);
        geocode::serve(&mut *connector, BUILD, &mut geocoder, &stop, &line);
    }

    /// The Mac's geocoder.
    pub struct CoreLocationGeocoder;

    impl Geocoder for CoreLocationGeocoder {
        fn geocode(&mut self, query: &GeocodeQuery) -> GeocodeOutcome {
            let (done, answer) = channel();
            let query = query.clone();
            on_main(move || {
                let done_now = done.clone();
                let started = autoreleasepool(|_| unsafe { start(&query, done) });
                if let Err(why) = started {
                    let _ =
                        done_now.send(Err(GeocodeError::new(GeocodeErrorKind::Unavailable, why)));
                }
            });
            answer.recv_timeout(ANSWER).unwrap_or_else(|_| {
                Err(GeocodeError::new(
                    GeocodeErrorKind::Unavailable,
                    "the Mac's geocoder did not answer in time",
                ))
            })
        }
    }

    unsafe fn text(object: *mut AnyObject, key: &str) -> String {
        let value: *mut AnyObject = match key {
            "name" => msg_send![object, name],
            "subThoroughfare" => msg_send![object, subThoroughfare],
            "thoroughfare" => msg_send![object, thoroughfare],
            "subLocality" => msg_send![object, subLocality],
            "locality" => msg_send![object, locality],
            "subAdministrativeArea" => msg_send![object, subAdministrativeArea],
            "administrativeArea" => msg_send![object, administrativeArea],
            "postalCode" => msg_send![object, postalCode],
            "ISOcountryCode" => msg_send![object, ISOcountryCode],
            "country" => msg_send![object, country],
            _ => std::ptr::null_mut(),
        };
        string_of(value)
    }

    unsafe fn placemark(object: *mut AnyObject) -> RawPlacemark {
        let location: *mut AnyObject = msg_send![object, location];
        let position = if location.is_null() {
            None
        } else {
            let coordinate: Coordinate = msg_send![location, coordinate];
            Some((coordinate.latitude, coordinate.longitude))
        };
        RawPlacemark {
            name: text(object, "name"),
            sub_thoroughfare: text(object, "subThoroughfare"),
            thoroughfare: text(object, "thoroughfare"),
            sub_locality: text(object, "subLocality"),
            locality: text(object, "locality"),
            sub_administrative_area: text(object, "subAdministrativeArea"),
            administrative_area: text(object, "administrativeArea"),
            postal_code: text(object, "postalCode"),
            iso_country_code: text(object, "ISOcountryCode"),
            country: text(object, "country"),
            position,
        }
    }

    /// Starts the lookup on the main thread; its answer goes to `done` from CLGeocoder's
    /// completion handler, which also runs on the main thread.
    unsafe fn start(
        query: &GeocodeQuery,
        done: std::sync::mpsc::Sender<GeocodeOutcome>,
    ) -> Result<(), String> {
        let geocoder_class = class(c"CLGeocoder")?;
        let geocoder: Allocated<AnyObject> = msg_send![geocoder_class, alloc];
        let geocoder: Option<Retained<AnyObject>> = msg_send![geocoder, init];
        let geocoder = geocoder.ok_or("the Mac's geocoder could not be made")?;
        let locale: *mut AnyObject = if query.language.is_empty() {
            std::ptr::null_mut()
        } else {
            msg_send![class(c"NSLocale")?, localeWithLocaleIdentifier: &*ns_string(&query.language)]
        };
        let max = query.max_results as usize;
        // The handler keeps the geocoder alive until it has answered.
        let keep = geocoder.clone();
        let handler = RcBlock::new(move |placemarks: *mut AnyObject, error: *mut AnyObject| {
            let _ = &keep;
            let outcome = if !error.is_null() {
                let code: isize = msg_send![error, code];
                let description: *mut AnyObject = msg_send![error, localizedDescription];
                match failure(code, &string_of(description)) {
                    Some(error) => Err(error),
                    None => Ok(Vec::new()),
                }
            } else if placemarks.is_null() {
                Ok(Vec::new())
            } else {
                let count: usize = msg_send![placemarks, count];
                Ok((0..count.min(max))
                    .map(|i| {
                        let object: *mut AnyObject = msg_send![placemarks, objectAtIndex: i];
                        address(&placemark(object))
                    })
                    .collect())
            };
            let _ = done.send(outcome);
        });
        match &query.lookup {
            Lookup::Forward { query: name, .. } => {
                let _: () = msg_send![
                    &*geocoder,
                    geocodeAddressString: &*ns_string(name),
                    inRegion: std::ptr::null_mut::<AnyObject>(),
                    preferredLocale: locale,
                    completionHandler: &*handler
                ];
            }
            Lookup::Reverse {
                latitude,
                longitude,
            } => {
                let location: Allocated<AnyObject> = msg_send![class(c"CLLocation")?, alloc];
                let location: Option<Retained<AnyObject>> =
                    msg_send![location, initWithLatitude: *latitude, longitude: *longitude];
                let location = location.ok_or("the position could not be made a location")?;
                let _: () = msg_send![
                    &*geocoder,
                    reverseGeocodeLocation: &*location,
                    preferredLocale: locale,
                    completionHandler: &*handler
                ];
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr36_a_placemark_reads_as_an_android_address() {
        let raw = RawPlacemark {
            name: "Seoul City Hall".into(),
            sub_thoroughfare: "110".into(),
            thoroughfare: "Sejong-daero".into(),
            locality: "Seoul".into(),
            postal_code: "04524".into(),
            iso_country_code: "KR".into(),
            country: "South Korea".into(),
            position: Some((37.5663, 126.9779)),
            ..Default::default()
        };
        let address = address(&raw);
        assert_eq!(
            address.address_lines,
            vec!["Seoul City Hall, 110 Sejong-daero, Seoul, 04524, South Korea".to_string()]
        );
        assert_eq!(address.country_code, "KR");
        assert_eq!(address.position, Some((37.5663, 126.9779)));
    }

    #[test]
    fn fr36_the_macs_geocoder_errors_map_to_androids() {
        assert!(failure(8, "no result").is_none());
        assert_eq!(
            failure(1, "denied").unwrap().kind,
            GeocodeErrorKind::Refused
        );
        assert_eq!(
            failure(2, "offline").unwrap().kind,
            GeocodeErrorKind::Unavailable
        );
        assert_eq!(
            failure(2, "The request was throttled").unwrap().kind,
            GeocodeErrorKind::Busy
        );
    }
}
