//! The host's location for the guest: the rules between what the guest wants and the host's
//! location service, with a fake service, and the local API that carries both between the
//! service and the user's session, over the in-memory transport.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use dsa_host_core::host_api::{HostApiClient, PerUser, UserAndroid};
use dsa_host_core::host_api_conformance::RunningService;
use dsa_host_core::ime::ImeEvent;
use dsa_host_core::input::PointerEvent;
use dsa_host_core::keys::KeyEvent;
use dsa_host_core::location::{
    self, Access, Fix, LocationBridge, LocationEvent, LocationHub, LocationProvider, Wanted,
};
use dsa_host_core::session::SessionState;
use dsa_host_core::tasks::TaskRequestError;
use dsa_protocol::control::{self, envelope::Body};
use dsa_transport::local;

const WAIT: Duration = Duration::from_secs(10);

/// What the fake location service was asked and what it has to say.
#[derive(Default)]
struct Service {
    /// Each start, with its interval, and each stop (`None`), in order.
    calls: Vec<Option<Duration>>,
    /// Set to refuse every start, as a host whose user turned location off does.
    refuse: Option<(Access, String)>,
    /// Where the running service sends what it reports.
    events: Option<Sender<LocationEvent>>,
}

/// The host's location service as a test drives it.
#[derive(Clone, Default)]
struct FakeProvider(Arc<Mutex<Service>>);

impl FakeProvider {
    fn calls(&self) -> Vec<Option<Duration>> {
        self.0.lock().unwrap().calls.clone()
    }

    fn running(&self) -> bool {
        self.0.lock().unwrap().events.is_some()
    }

    /// Reports `event` as the running service would. Returns whether one was running.
    fn report(&self, event: LocationEvent) -> bool {
        match &self.0.lock().unwrap().events {
            Some(events) => events.send(event).is_ok(),
            None => false,
        }
    }
}

impl LocationProvider for FakeProvider {
    fn start(
        &mut self,
        interval: Duration,
        events: Sender<LocationEvent>,
    ) -> Result<(), (Access, String)> {
        let mut service = self.0.lock().unwrap();
        if let Some(refusal) = service.refuse.clone() {
            return Err(refusal);
        }
        service.calls.push(Some(interval));
        service.events = Some(events);
        Ok(())
    }

    fn stop(&mut self) {
        let mut service = self.0.lock().unwrap();
        service.calls.push(None);
        service.events = None;
    }
}

/// A position in Seoul with every value known.
fn seoul() -> Fix {
    Fix {
        latitude_degrees: 37.5,
        longitude_degrees: 127.0,
        horizontal_accuracy_meters: Some(10.0),
        altitude_meters: Some(50.0),
        vertical_accuracy_meters: Some(3.0),
        speed_meters_per_second: Some(2.5),
        bearing_degrees: Some(90.0),
        unix_time_ms: 1_700_000_000_000,
    }
}

fn allowed() -> LocationEvent {
    LocationEvent::Status {
        access: Access::Allowed,
        detail: String::new(),
    }
}

#[test]
fn fr26_the_location_service_runs_only_while_android_wants_a_location() {
    let fake = FakeProvider::default();
    let mut provider = fake.clone();
    {
        let mut bridge = LocationBridge::new(&mut provider);
        assert!(bridge.want(Wanted::NONE).is_empty());
        assert!(
            fake.calls().is_empty(),
            "nothing runs before a location is wanted"
        );

        let every_second = Wanted::every(Duration::from_secs(1));
        assert_eq!(bridge.want(every_second), vec![allowed()]);
        assert!(bridge.running());
        // The same word again changes nothing.
        assert!(bridge.want(every_second).is_empty());
        // A new interval restarts the service at it.
        bridge.want(Wanted::every(Duration::from_secs(5)));
        assert!(bridge.want(Wanted::NONE).is_empty());
        assert!(!bridge.running());
        assert_eq!(
            fake.calls(),
            vec![
                Some(Duration::from_secs(1)),
                None,
                Some(Duration::from_secs(5)),
                None,
            ]
        );

        bridge.want(every_second);
    }
    assert!(
        !fake.running(),
        "the service stops when the bridge goes away"
    );
}

#[test]
fn fr26_the_interval_android_asks_for_is_kept_within_bounds() {
    assert_eq!(
        Wanted::every(Duration::ZERO).interval,
        location::DEFAULT_INTERVAL
    );
    assert_eq!(
        Wanted::every(Duration::from_millis(1)).interval,
        location::MIN_INTERVAL
    );
    assert_eq!(
        Wanted::every(Duration::from_secs(86_400)).interval,
        location::MAX_INTERVAL
    );
    let request = control::LocationRequest {
        active: true,
        interval_ms: 2000,
    };
    assert_eq!(
        Wanted::from_control(&request),
        Wanted::every(Duration::from_secs(2))
    );
    let request = control::LocationRequest {
        active: false,
        interval_ms: 2000,
    };
    assert_eq!(Wanted::from_control(&request), Wanted::NONE);
}

#[test]
fn fr26_a_fix_reaches_android_with_the_same_values() {
    let fake = FakeProvider::default();
    let mut provider = fake.clone();
    let mut bridge = LocationBridge::new(&mut provider);
    bridge.want(Wanted::every(Duration::from_secs(1)));
    assert!(fake.report(LocationEvent::Fix(seoul())));
    let taken = bridge.take();
    assert_eq!(taken, vec![LocationEvent::Fix(seoul())]);

    let expected = control::LocationFix {
        latitude_degrees: 37.5,
        longitude_degrees: 127.0,
        horizontal_accuracy_meters: 10.0,
        has_altitude: true,
        altitude_meters: 50.0,
        vertical_accuracy_meters: 3.0,
        has_speed: true,
        speed_meters_per_second: 2.5,
        has_bearing: true,
        bearing_degrees: 90.0,
        unix_time_ms: 1_700_000_000_000,
    };
    assert_eq!(taken[0].to_control(), Body::LocationFix(expected));
    // The local API carries it unchanged.
    assert_eq!(Fix::from_host_api(&seoul().to_host_api()), Ok(seoul()));
}

#[test]
fn fr26_values_the_host_does_not_know_are_left_out() {
    let bare = Fix {
        horizontal_accuracy_meters: None,
        altitude_meters: None,
        vertical_accuracy_meters: None,
        speed_meters_per_second: Some(f32::NAN),
        bearing_degrees: None,
        ..seoul()
    };
    let checked = bare.checked().unwrap();
    assert_eq!(checked.speed_meters_per_second, None);
    let fix = checked.to_control();
    assert!(!fix.has_altitude && !fix.has_speed && !fix.has_bearing);
    assert_eq!(fix.horizontal_accuracy_meters, 0.0);
    assert_eq!(Fix::from_host_api(&checked.to_host_api()), Ok(checked));

    // A bearing past a full turn comes back within one.
    let turned = Fix {
        bearing_degrees: Some(450.0),
        ..seoul()
    };
    assert_eq!(turned.checked().unwrap().bearing_degrees, Some(90.0));

    // A position that is not on Earth never reaches Android.
    let lost = Fix {
        latitude_degrees: 91.0,
        ..seoul()
    };
    assert!(lost.checked().is_err());
    let lost = Fix {
        longitude_degrees: f64::INFINITY,
        ..seoul()
    };
    assert!(Fix::from_host_api(&lost.to_host_api()).is_err());
}

#[test]
fn fr26_denied_location_sends_a_status_and_no_fix() {
    let fake = FakeProvider::default();
    fake.0.lock().unwrap().refuse = Some((
        Access::Denied,
        "location is off for desktop apps".to_string(),
    ));
    let mut provider = fake.clone();
    let mut bridge = LocationBridge::new(&mut provider);
    assert_eq!(
        bridge.want(Wanted::every(Duration::from_secs(1))),
        vec![LocationEvent::Status {
            access: Access::Denied,
            detail: "location is off for desktop apps".to_string(),
        }]
    );
    assert!(!bridge.running());
    assert!(!fake.report(LocationEvent::Fix(seoul())));
    assert!(bridge.take().is_empty());

    // Once the user turns location on, the next try starts the service.
    fake.0.lock().unwrap().refuse = None;
    assert_eq!(bridge.retry(), vec![allowed()]);
    assert!(bridge.running());
    assert!(bridge.retry().is_empty());
    drop(bridge);

    assert_eq!(
        LocationEvent::Status {
            access: Access::Denied,
            detail: String::new(),
        }
        .to_control(),
        Body::LocationStatus(control::LocationStatus {
            access: control::LocationAccess::Denied as i32,
            detail: String::new(),
        })
    );
}

#[test]
fn fr26_what_the_service_reports_after_it_stopped_is_dropped() {
    let fake = FakeProvider::default();
    let mut provider = fake.clone();
    let mut bridge = LocationBridge::new(&mut provider);
    bridge.want(Wanted::every(Duration::from_secs(1)));
    let late = fake.0.lock().unwrap().events.clone().unwrap();
    bridge.want(Wanted::NONE);
    let _ = late.send(LocationEvent::Fix(seoul()));
    assert!(bridge.take().is_empty());
}

#[test]
fn fr26_a_watcher_hears_what_android_wants_now_and_each_change() {
    let mut hub = LocationHub::default();
    let early = hub.watch();
    let wanted = Wanted::every(Duration::from_secs(1));
    hub.set(wanted);
    hub.set(wanted);
    let late = hub.watch();
    hub.set(Wanted::NONE);
    assert_eq!(
        early.try_iter().collect::<Vec<_>>(),
        vec![Wanted::NONE, wanted, Wanted::NONE]
    );
    assert_eq!(
        late.try_iter().collect::<Vec<_>>(),
        vec![wanted, Wanted::NONE]
    );
}

/// An Android that wants the location as its hub says, and keeps what the host sends it.
#[derive(Default)]
struct Locating {
    hub: Mutex<LocationHub>,
    received: Mutex<Vec<LocationEvent>>,
}

impl Locating {
    fn want(&self, wanted: Wanted) {
        self.hub.lock().unwrap().set(wanted);
    }

    /// Waits until it has received `count` events, and returns them.
    fn received(&self, count: usize) -> Vec<LocationEvent> {
        let deadline = Instant::now() + WAIT;
        loop {
            let received = self.received.lock().unwrap().clone();
            if received.len() >= count || Instant::now() > deadline {
                return received;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl UserAndroid for Locating {
    fn state(&self) -> SessionState {
        SessionState::Active
    }
    fn last_reason(&self) -> Option<String> {
        None
    }
    fn start(&self) {}
    fn stop(&self) {}
    fn pointer(&self, _: &PointerEvent) -> Result<(), TaskRequestError> {
        Ok(())
    }
    fn key(&self, _: &KeyEvent) -> Result<(), TaskRequestError> {
        Ok(())
    }
    fn ime(&self, _: &ImeEvent) -> Result<(), TaskRequestError> {
        Ok(())
    }
    fn share_clipboard(&self, _: bool) {}
    fn host_clipboard(&self, _: String) {}
    fn watch_clipboard(&self) -> Option<Receiver<String>> {
        None
    }
    fn watch_location(&self) -> Option<Receiver<Wanted>> {
        Some(self.hub.lock().unwrap().watch())
    }
    fn host_location(&self, event: LocationEvent) {
        self.received.lock().unwrap().push(event);
    }
}

fn locating_service(android: Arc<Locating>) -> (local::MemoryLocalConnector, RunningService) {
    let (listener, connector) = local::pair();
    let androids = Arc::new(PerUser::new(move |_: &str| {
        Ok(Arc::clone(&android) as Arc<dyn UserAndroid>)
    }));
    let service = RunningService::start(Box::new(listener), androids);
    (connector, service)
}

#[test]
fn fr26_the_local_api_carries_what_android_wants_and_the_hosts_fixes() {
    let android = Arc::new(Locating::default());
    let (mut connector, _service) = locating_service(Arc::clone(&android));
    let client = HostApiClient::connect(&mut connector, "test", WAIT).unwrap();
    let mut stream = client.watch_location().unwrap();
    assert_eq!(stream.next(WAIT).unwrap(), Some(Wanted::NONE));

    let wanted = Wanted::every(Duration::from_secs(2));
    android.want(wanted);
    assert_eq!(stream.next(WAIT).unwrap(), Some(wanted));

    stream.send(&allowed()).unwrap();
    // A position that is not on Earth is dropped by the service.
    let lost = Fix {
        latitude_degrees: -120.0,
        ..seoul()
    };
    stream.send(&LocationEvent::Fix(lost)).unwrap();
    stream.send(&LocationEvent::Fix(seoul())).unwrap();
    assert_eq!(
        android.received(2),
        vec![allowed(), LocationEvent::Fix(seoul())]
    );
}

#[test]
fn fr26_the_session_watcher_runs_the_location_service_while_android_wants_it() {
    let android = Arc::new(Locating::default());
    let (mut connector, _service) = locating_service(Arc::clone(&android));
    let fake = FakeProvider::default();
    let stop = Arc::new(AtomicBool::new(false));
    let follower = {
        let (mut provider, stop) = (fake.clone(), Arc::clone(&stop));
        thread::spawn(move || {
            let log = |_: &str| {};
            location::follow(&mut connector, "test", &mut provider, &stop, &log);
        })
    };

    android.want(Wanted::every(Duration::from_secs(1)));
    let deadline = Instant::now() + WAIT;
    while !fake.running() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        fake.running(),
        "the service runs once Android wants a location"
    );
    assert!(fake.report(LocationEvent::Fix(seoul())));
    assert_eq!(
        android.received(2),
        vec![allowed(), LocationEvent::Fix(seoul())]
    );

    android.want(Wanted::NONE);
    let deadline = Instant::now() + WAIT;
    while fake.running() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        !fake.running(),
        "the service stops once Android wants no location"
    );

    stop.store(true, Ordering::Relaxed);
    follower.join().unwrap();
}
