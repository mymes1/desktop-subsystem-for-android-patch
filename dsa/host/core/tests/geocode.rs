//! Address lookups through the host: the checks on a guest's lookup, the Nominatim geocoder
//! over a fake download, and the wire forms of what it finds.

use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;
use std::time::{Duration, Instant};

use dsa_host_core::geocode::{
    self, Address, Bounds, GeocodeErrorKind, GeocodeQuery, Geocoder, Lookup, Nominatim,
};
use dsa_host_core::update::store::Fetch;
use dsa_protocol::control;

/// A download that answers with `body`, or fails with `error`, and records each URL.
#[derive(Clone, Default)]
struct FakeFetch {
    urls: Rc<RefCell<Vec<(String, Instant)>>>,
    body: Rc<RefCell<Vec<u8>>>,
    error: Rc<RefCell<Option<String>>>,
}

impl Fetch for FakeFetch {
    fn fetch(&self, url: &str, out: &mut dyn Write) -> Result<u64, String> {
        self.urls
            .borrow_mut()
            .push((url.to_string(), Instant::now()));
        if let Some(error) = self.error.borrow().clone() {
            return Err(error);
        }
        let body = self.body.borrow();
        out.write_all(&body).unwrap();
        Ok(body.len() as u64)
    }
}

const SEARCH: &str = r#"[
  {"place_id":1,"lat":"37.5663","lon":"126.9779","name":"Seoul City Hall",
   "display_name":"Seoul City Hall, 110, Sejong-daero, Jung-gu, Seoul, 04524, South Korea",
   "address":{"amenity":"Seoul City Hall","house_number":"110","road":"Sejong-daero",
              "suburb":"Jung-gu","city":"Seoul","postcode":"04524","country":"South Korea",
              "country_code":"kr"}},
  {"place_id":2,"lat":"35.1796","lon":"129.0756","name":"Busan City Hall",
   "display_name":"Busan City Hall, Busan, South Korea",
   "address":{"city":"Busan","country":"South Korea","country_code":"kr"}}
]"#;

fn forward(query: &str) -> GeocodeQuery {
    GeocodeQuery::from_control(&control::GeocodeRequest {
        request_id: 1,
        query: query.into(),
        max_results: 5,
        language: "ko_KR".into(),
        ..Default::default()
    })
    .unwrap()
}

#[test]
fn fr36_a_forward_lookup_reads_the_addresses_nominatim_found() {
    let fetch = FakeFetch::default();
    *fetch.body.borrow_mut() = SEARCH.as_bytes().to_vec();
    let mut geocoder = Nominatim::new(fetch.clone(), "https://geocoder.example/");
    let found = geocoder.geocode(&forward("Seoul City Hall")).unwrap();
    assert_eq!(found.len(), 2);
    assert_eq!(
        found[0],
        Address {
            address_lines: vec![
                "Seoul City Hall, 110, Sejong-daero, Jung-gu, Seoul, 04524, South Korea".into()
            ],
            feature_name: "Seoul City Hall".into(),
            sub_thoroughfare: "110".into(),
            thoroughfare: "Sejong-daero".into(),
            sub_locality: "Jung-gu".into(),
            locality: "Seoul".into(),
            postal_code: "04524".into(),
            country_code: "KR".into(),
            country_name: "South Korea".into(),
            position: Some((37.5663, 126.9779)),
            ..Default::default()
        }
    );
    let urls = fetch.urls.borrow();
    assert_eq!(
        urls[0].0,
        "https://geocoder.example/search?format=jsonv2&addressdetails=1&limit=5\
         &q=Seoul%20City%20Hall&accept-language=ko-KR"
    );
}

#[test]
fn fr36_a_reverse_lookup_asks_for_the_position_and_reads_one_place() {
    let fetch = FakeFetch::default();
    *fetch.body.borrow_mut() =
        br#"{"lat":"37.5663","lon":"126.9779","display_name":"Jung-gu, Seoul, South Korea","address":{"city":"Seoul","country_code":"kr"}}"#
            .to_vec();
    let mut geocoder = Nominatim::new(fetch.clone(), geocode::NOMINATIM_URL);
    let query = GeocodeQuery::from_control(&control::GeocodeRequest {
        request_id: 2,
        reverse: true,
        latitude_degrees: 37.5663,
        longitude_degrees: 126.9779,
        max_results: 1,
        ..Default::default()
    })
    .unwrap();
    let found = geocoder.geocode(&query).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].locality, "Seoul");
    assert_eq!(
        fetch.urls.borrow()[0].0,
        "https://nominatim.openstreetmap.org/reverse?format=jsonv2&addressdetails=1\
         &lat=37.5663&lon=126.9779"
    );
}

#[test]
fn fr36_a_position_with_no_address_finds_none_rather_than_failing() {
    let fetch = FakeFetch::default();
    *fetch.body.borrow_mut() = br#"{"error":"Unable to geocode"}"#.to_vec();
    let mut geocoder = Nominatim::new(fetch, geocode::NOMINATIM_URL);
    let query = GeocodeQuery::from_control(&control::GeocodeRequest {
        reverse: true,
        max_results: 1,
        ..Default::default()
    })
    .unwrap();
    assert_eq!(geocoder.geocode(&query), Ok(Vec::new()));
}

#[test]
fn fr36_errors_of_the_geocoding_service_map_to_androids_failures() {
    let fetch = FakeFetch::default();
    *fetch.error.borrow_mut() = Some("https://x: the server answered HTTP 429".into());
    let mut geocoder = Nominatim::new(fetch.clone(), geocode::NOMINATIM_URL);
    assert_eq!(
        geocoder.geocode(&forward("a")).unwrap_err().kind,
        GeocodeErrorKind::Busy
    );
    *fetch.error.borrow_mut() = Some("could not resolve the host".into());
    assert_eq!(
        geocoder.geocode(&forward("b")).unwrap_err().kind,
        GeocodeErrorKind::Unavailable
    );
    *fetch.error.borrow_mut() = None;
    *fetch.body.borrow_mut() = b"<html>".to_vec();
    assert_eq!(
        geocoder.geocode(&forward("c")).unwrap_err().kind,
        GeocodeErrorKind::Unavailable
    );

    // Each reaches the guest as its status.
    let busy: geocode::GeocodeOutcome = Err(geocode::GeocodeError::new(
        GeocodeErrorKind::Busy,
        "slow down",
    ));
    let result = geocode::result_to_control(4, &busy);
    assert_eq!(result.status, control::GeocodeStatus::Busy as i32);
    assert_eq!(result.message, "slow down");
}

#[test]
fn fr36_nominatim_is_asked_at_most_once_a_second_and_a_repeated_lookup_is_reused() {
    let fetch = FakeFetch::default();
    *fetch.body.borrow_mut() = SEARCH.as_bytes().to_vec();
    let mut geocoder = Nominatim::new(fetch.clone(), geocode::NOMINATIM_URL);
    geocoder.geocode(&forward("first")).unwrap();
    geocoder.geocode(&forward("second")).unwrap();
    geocoder.geocode(&forward("first")).unwrap();
    let urls = fetch.urls.borrow();
    assert_eq!(urls.len(), 2, "the repeated lookup came from the cache");
    let spacing = urls[1].1.duration_since(urls[0].1);
    assert!(
        spacing >= geocode::NOMINATIM_SPACING - Duration::from_millis(5),
        "{spacing:?}"
    );
}

#[test]
fn fr36_a_lookup_that_cannot_be_answered_is_refused_before_it_reaches_the_host() {
    let empty = GeocodeQuery::from_control(&control::GeocodeRequest {
        query: "   ".into(),
        ..Default::default()
    });
    assert_eq!(empty.unwrap_err().kind, GeocodeErrorKind::Invalid);
    let off_earth = GeocodeQuery::from_control(&control::GeocodeRequest {
        reverse: true,
        latitude_degrees: 91.0,
        ..Default::default()
    });
    assert_eq!(off_earth.unwrap_err().kind, GeocodeErrorKind::Invalid);

    // The count is kept in bounds, the language tag made plain, and bounds kept.
    let query = GeocodeQuery::from_control(&control::GeocodeRequest {
        query: "cafe".into(),
        max_results: 500,
        language: "en-US;drop".into(),
        has_bounds: true,
        lower_left_latitude: 37.0,
        lower_left_longitude: 126.0,
        upper_right_latitude: 38.0,
        upper_right_longitude: 127.0,
        ..Default::default()
    })
    .unwrap();
    assert_eq!(query.max_results, geocode::MAX_RESULTS);
    assert_eq!(query.language, "");
    assert_eq!(
        query.lookup,
        Lookup::Forward {
            query: "cafe".into(),
            bounds: Some(Bounds {
                lower_left_latitude: 37.0,
                lower_left_longitude: 126.0,
                upper_right_latitude: 38.0,
                upper_right_longitude: 127.0,
            })
        }
    );
    let url = Nominatim::new(FakeFetch::default(), geocode::NOMINATIM_URL).url(&query);
    assert!(url.ends_with("&q=cafe&viewbox=126,38,127,37"), "{url}");
}

#[test]
fn fr36_a_lookup_and_its_addresses_survive_the_local_api() {
    let query = forward("Seoul");
    assert_eq!(
        GeocodeQuery::from_host_api(&query.to_host_api()).unwrap(),
        query
    );
    let address = Address {
        address_lines: vec!["Seoul".into()],
        country_code: "kr".into(),
        position: Some((37.5, 127.0)),
        ..Default::default()
    }
    .checked();
    assert_eq!(address.country_code, "KR");
    let outcome: geocode::GeocodeOutcome = Ok(vec![address.clone()]);
    let report = geocode::report_to_host_api(3, &outcome);
    assert_eq!(geocode::outcome_from_host_api(report), Ok(outcome));
    assert_eq!(
        Address::from_host_api(address.to_host_api()),
        address,
        "the same address comes back"
    );
}
