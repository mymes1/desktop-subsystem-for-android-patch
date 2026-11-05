//! The host's system settings, game controllers and window settings for apps, through the
//! local API over the in-memory transport.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use dsa_host_core::app_windows::AppWindowSettings;
use dsa_host_core::gamepads::{buttons, GamepadEvent, GamepadInfo, GamepadRumble, GamepadState};
use dsa_host_core::host_api::{
    ClientError, HostApiClient, HostApiService, PerUser, ServiceFacts, UserAndroid,
};
use dsa_host_core::host_api_conformance::RunningService;
use dsa_host_core::ime::ImeEvent;
use dsa_host_core::input::PointerEvent;
use dsa_host_core::keys::KeyEvent;
use dsa_host_core::session::SessionState;
use dsa_host_core::settings::MemorySettings;
use dsa_host_core::system_settings::SystemSettings;
use dsa_host_core::tasks::TaskRequestError;
use dsa_transport::local::{self, MemoryLocalConnector};

const WAIT: Duration = Duration::from_secs(10);

/// An Android that records what the host's system tells it.
#[derive(Default)]
struct Following {
    settings: Mutex<Vec<SystemSettings>>,
    gamepads: Mutex<Vec<GamepadEvent>>,
    rumble: Mutex<Vec<Sender<GamepadRumble>>>,
}

impl Following {
    fn settings(&self, count: usize) -> Vec<SystemSettings> {
        wait_for(|| self.settings.lock().unwrap().clone(), count)
    }

    fn gamepads(&self, count: usize) -> Vec<GamepadEvent> {
        wait_for(|| self.gamepads.lock().unwrap().clone(), count)
    }

    fn rumble(&self, effect: GamepadRumble) {
        self.rumble
            .lock()
            .unwrap()
            .retain(|watcher| watcher.send(effect).is_ok());
    }
}

fn wait_for<T>(read: impl Fn() -> Vec<T>, count: usize) -> Vec<T> {
    let deadline = Instant::now() + WAIT;
    loop {
        let got = read();
        if got.len() >= count || Instant::now() > deadline {
            return got;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

impl UserAndroid for Following {
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
    fn set_system_settings(&self, settings: SystemSettings) {
        self.settings.lock().unwrap().push(settings);
    }
    fn gamepad(&self, event: GamepadEvent) {
        self.gamepads.lock().unwrap().push(event);
    }
    fn watch_gamepad_rumble(&self) -> Option<Receiver<GamepadRumble>> {
        let (tx, rx) = mpsc::channel();
        self.rumble.lock().unwrap().push(tx);
        Some(rx)
    }
}

fn service(android: Arc<Following>) -> (MemoryLocalConnector, RunningService) {
    let (listener, connector) = local::pair();
    let androids = Arc::new(PerUser::new(move |_: &str| {
        Ok(Arc::clone(&android) as Arc<dyn UserAndroid>)
    }));
    let service = HostApiService::new(androids, "test service")
        .with_settings(Arc::new(MemorySettings::default()), ServiceFacts::default());
    (
        connector,
        RunningService::start_service(Box::new(listener), service),
    )
}

fn seoul() -> SystemSettings {
    SystemSettings {
        time_zone: "Asia/Seoul".into(),
        locales: vec!["ko-KR".into(), "en-US".into()],
        regional_format: "ko-KR".into(),
        time_24_hour: true,
        first_day_of_week: 1,
        dark_mode: true,
        accent_color: 0xFF00_78D4,
    }
}

#[test]
fn fr30_the_local_api_gives_the_users_android_their_system_settings() {
    let android = Arc::new(Following::default());
    let (mut connector, _service) = service(Arc::clone(&android));
    let mut client = HostApiClient::connect(&mut connector, "test", WAIT).unwrap();
    client.set_system_settings(&seoul()).unwrap();
    // A value Android could not take is left out on the way.
    let odd = SystemSettings {
        time_zone: "../zoneinfo".into(),
        ..seoul()
    };
    client.set_system_settings(&odd).unwrap();
    assert_eq!(
        android.settings(2),
        vec![
            seoul(),
            SystemSettings {
                time_zone: String::new(),
                ..seoul()
            }
        ]
    );
}

fn pad() -> GamepadInfo {
    GamepadInfo {
        pad_id: 0,
        name: "Xbox Controller".into(),
        vendor_id: 0x045e,
        product_id: 0x028e,
        rumble: true,
    }
}

#[test]
fn fr30_the_local_api_carries_controllers_and_their_rumble_and_ends_them_with_the_connection() {
    let android = Arc::new(Following::default());
    let (mut connector, _service) = service(Arc::clone(&android));
    let client = HostApiClient::connect(&mut connector, "test", WAIT).unwrap();
    let mut stream = client.watch_gamepads().unwrap();
    let state = GamepadState {
        pad_id: 0,
        buttons: buttons::Y | buttons::START,
        left_x: 300,
        left_y: -32768,
        right_x: 32767,
        right_y: 0,
        left_trigger: 128,
        right_trigger: 255,
    };
    stream.send(&GamepadEvent::Connected(pad())).unwrap();
    stream.send(&GamepadEvent::State(state)).unwrap();
    assert_eq!(
        android.gamepads(2),
        vec![GamepadEvent::Connected(pad()), GamepadEvent::State(state)]
    );

    let effect = GamepadRumble {
        pad_id: 0,
        strong: 65535,
        weak: 20000,
        duration_ms: 500,
    };
    android.rumble(effect);
    // A rumble for a controller this connection does not carry is not its to play.
    android.rumble(GamepadRumble {
        pad_id: 9,
        ..effect
    });
    assert_eq!(stream.next(WAIT).unwrap(), Some(effect));
    assert_eq!(stream.next(Duration::from_millis(200)).unwrap(), None);

    // The window manager that read the controller goes away: so does the controller.
    drop(stream);
    assert_eq!(
        android.gamepads(3)[2],
        GamepadEvent::Disconnected { pad_id: 0 }
    );
}

#[test]
fn fr30_window_settings_for_an_app_are_kept_and_reset_through_the_local_api() {
    let android = Arc::new(Following::default());
    let (mut connector, _service) = service(Arc::clone(&android));
    let mut client = HostApiClient::connect(&mut connector, "test", WAIT).unwrap();
    assert!(client.app_window_settings().unwrap().is_empty());
    let notes = AppWindowSettings {
        package_name: "com.example.notes".into(),
        fixed_size: true,
        start_size_dp: Some((400, 700)),
    };
    assert_eq!(
        client.set_app_window_settings(&notes).unwrap(),
        vec![notes.clone()]
    );
    assert_eq!(client.app_window_settings().unwrap(), vec![notes.clone()]);
    match client.set_app_window_settings(&AppWindowSettings {
        start_size_dp: Some((10, 10)),
        ..notes.clone()
    }) {
        Err(ClientError::Service { detail, .. }) => {
            assert!(detail.contains("start size"), "{detail}")
        }
        other => panic!("expected the size to be refused, got {other:?}"),
    }
    let reset = AppWindowSettings {
        package_name: notes.package_name.clone(),
        ..AppWindowSettings::default()
    };
    assert!(client.set_app_window_settings(&reset).unwrap().is_empty());
}

/// Host settings a test changes, as the user would in the host's settings.
#[derive(Clone, Default)]
struct FakeHostSettings {
    now: Arc<Mutex<SystemSettings>>,
    changed: Arc<std::sync::atomic::AtomicBool>,
}

impl FakeHostSettings {
    fn change(&self, settings: SystemSettings) {
        *self.now.lock().unwrap() = settings;
        self.changed
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

impl dsa_host_core::system_settings::SystemSettingsSource for FakeHostSettings {
    fn read(&mut self) -> SystemSettings {
        self.now.lock().unwrap().clone()
    }
    fn wait_for_change(&mut self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self
                .changed
                .swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                return true;
            }
            thread::sleep(Duration::from_millis(5));
        }
        false
    }
}

#[test]
fn fr30_a_change_to_the_hosts_settings_reaches_android() {
    let android = Arc::new(Following::default());
    let (connector, _service) = service(Arc::clone(&android));
    let host = FakeHostSettings::default();
    host.change(seoul());
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let follower = {
        let (mut source, stop) = (host.clone(), Arc::clone(&stop));
        let mut connector = connector.clone();
        thread::spawn(move || {
            dsa_host_core::system_settings::follow(
                &mut connector,
                "test",
                &mut source,
                &stop,
                &|_: &str| {},
            )
        })
    };
    assert_eq!(android.settings(1)[0], seoul());
    // The user turns the host's dark mode off and picks another accent color.
    let light = SystemSettings {
        dark_mode: false,
        accent_color: 0xFFE8_1123,
        ..seoul()
    };
    host.change(light.clone());
    assert_eq!(android.settings(2)[1], light);
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    follower.join().unwrap();
}

/// Controllers a test plugs in, presses and unplugs, and the rumble they were given.
#[derive(Clone, Default)]
struct FakePads {
    now: Arc<Mutex<Vec<(GamepadInfo, GamepadState)>>>,
    motors: Arc<Mutex<Vec<(u32, u16, u16)>>>,
}

impl dsa_host_core::gamepads::GamepadSource for FakePads {
    fn read(&mut self) -> Vec<(GamepadInfo, GamepadState)> {
        self.now.lock().unwrap().clone()
    }
    fn rumble(&mut self, pad_id: u32, strong: u16, weak: u16) {
        self.motors.lock().unwrap().push((pad_id, strong, weak));
    }
}

#[test]
fn fr30_a_controllers_state_reaches_android_with_the_same_values_while_a_window_is_in_front() {
    let android = Arc::new(Following::default());
    let (connector, _service) = service(Arc::clone(&android));
    let pads = FakePads::default();
    let pressed = GamepadState {
        pad_id: 0,
        buttons: buttons::A | buttons::RIGHT_SHOULDER,
        left_x: -32768,
        left_y: 1200,
        right_x: 0,
        right_y: 32767,
        left_trigger: 0,
        right_trigger: 255,
    };
    pads.now.lock().unwrap().push((pad(), pressed));
    let front = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let follower = {
        let (mut source, stop, front) = (pads.clone(), Arc::clone(&stop), Arc::clone(&front));
        let mut connector = connector.clone();
        thread::spawn(move || {
            let active = move || front.load(std::sync::atomic::Ordering::SeqCst);
            dsa_host_core::gamepads::follow(
                &mut connector,
                "test",
                &mut source,
                &active,
                &stop,
                &|_: &str| {},
            )
        })
    };
    // With no window of Android in front, the controller is there but nothing is held.
    assert_eq!(
        android.gamepads(2),
        vec![
            GamepadEvent::Connected(pad()),
            GamepadEvent::State(GamepadState {
                pad_id: 0,
                ..GamepadState::default()
            })
        ]
    );
    front.store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(android.gamepads(3)[2], GamepadEvent::State(pressed));

    // A game rumbles the controller for a moment; the motors stop when it is over.
    wait_for(|| android.rumble.lock().unwrap().clone(), 1);
    android.rumble(GamepadRumble {
        pad_id: 0,
        strong: 50000,
        weak: 3000,
        duration_ms: 50,
    });
    let motors = wait_for(|| pads.motors.lock().unwrap().clone(), 2);
    assert_eq!(motors, vec![(0, 50000, 3000), (0, 0, 0)]);

    // The controller is unplugged.
    pads.now.lock().unwrap().clear();
    assert_eq!(
        android.gamepads(4)[3],
        GamepadEvent::Disconnected { pad_id: 0 }
    );
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    follower.join().unwrap();
}
