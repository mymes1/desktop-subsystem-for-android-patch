//! How each app's windows follow a monitor of another scale, on the Apps page of DSA Settings
//! for Linux, without a display: what it reads from `dsa app-window`, what it sends, and how it
//! shows System (forced) while Android cannot do it yet.

use std::sync::{Arc, Mutex};

use dsa_settings_linux::api::{Dsa, Failure, Problem};
use dsa_settings_linux::model::{AppWindow, Client, DpiScaling};
use dsa_settings_linux::view;
use serde_json::json;

/// A stand-in for the service: it records each command line and gives the answer the test
/// set for it.
struct Fake {
    calls: Mutex<Vec<String>>,
    answers: Vec<(String, String)>,
}

impl Fake {
    fn new(answers: &[(&str, serde_json::Value)]) -> Arc<Fake> {
        Arc::new(Fake {
            calls: Mutex::new(Vec::new()),
            answers: answers
                .iter()
                .map(|(line, json)| (line.to_string(), json.to_string()))
                .collect(),
        })
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl Dsa for Fake {
    fn run(&self, args: &[String]) -> Result<String, Failure> {
        let line = args.join(" ");
        self.calls.lock().unwrap().push(line.clone());
        match self.answers.iter().find(|(l, _)| *l == line) {
            Some((_, out)) => Ok(out.clone()),
            None => Err(Failure::new(
                Problem::Refused,
                format!("no answer for {line}"),
            )),
        }
    }
}

fn client(fake: Arc<Fake>) -> Client {
    Client::new(fake.clone(), fake, false)
}

#[test]
fn fr4_dpi_scaling_choice_is_read_and_sent() {
    let fake = Fake::new(&[
        (
            "app-window --json",
            json!({
                "apps": [
                    {"package": "com.example.reader", "fixed_size": false, "start_size": null,
                     "dpi_scaling": "system"},
                    {"package": "com.example.notes", "fixed_size": true, "start_size": null},
                ],
                "system_forced_available": false,
            }),
        ),
        (
            "app-window com.example.reader --fixed-size off --start-size auto --dpi-scaling app --json",
            json!({"apps": []}),
        ),
    ]);
    let c = client(fake.clone());
    let list = c.app_window_list().unwrap();
    assert!(!list.system_forced_available);
    let reader = list
        .apps
        .iter()
        .find(|w| w.package == "com.example.reader")
        .unwrap();
    assert_eq!(reader.dpi_scaling, DpiScaling::System);
    // A dsa that names nothing for an app leaves it automatic.
    let notes = list
        .apps
        .iter()
        .find(|w| w.package == "com.example.notes")
        .unwrap();
    assert_eq!(notes.dpi_scaling, DpiScaling::Auto);
    assert_eq!(view::dpi_scaling_index(None), 0);
    assert_eq!(
        DpiScaling::ALL[view::dpi_scaling_index(Some(reader))],
        DpiScaling::System
    );

    c.set_app_window("com.example.reader", false, None, DpiScaling::App)
        .unwrap();
    assert_eq!(
        fake.calls().last().unwrap(),
        "app-window com.example.reader --fixed-size off --start-size auto --dpi-scaling app --json"
    );
}

#[test]
fn fr4_system_forced_is_marked_not_available_yet() {
    // A dsa that does not say whether Android can do it counts as one that cannot yet.
    let fake = Fake::new(&[(
        "app-window --json",
        json!({"apps": [{"package": "com.example.reader", "dpi_scaling": "system-forced"}]}),
    )]);
    let list = client(fake).app_window_list().unwrap();
    assert!(!list.system_forced_available);
    let reader = &list.apps[0];
    // It stays the app's stored choice.
    assert_eq!(reader.dpi_scaling, DpiScaling::SystemForced);

    let choices = view::dpi_scaling_choices(false);
    let scalings: Vec<DpiScaling> = choices.iter().map(|(_, s)| *s).collect();
    assert_eq!(scalings, DpiScaling::ALL);
    let (label, _) = &choices[3];
    assert!(label.contains("not available yet"), "{label}");
    assert!(choices[..3]
        .iter()
        .all(|(label, _)| !label.contains("not available")));
    // The row says why it works like System for now, and only while it is chosen.
    let tip = view::dpi_scaling_subtitle(Some(reader), false);
    assert!(tip.contains("works like System"), "{tip}");
    let other = AppWindow {
        dpi_scaling: DpiScaling::App,
        ..reader.clone()
    };
    assert!(!view::dpi_scaling_subtitle(Some(&other), false).contains("works like System"));
    // Once Android can do it, it is offered plainly.
    let (label, _) = &view::dpi_scaling_choices(true)[3];
    assert!(!label.contains("not available"), "{label}");
    assert!(!view::dpi_scaling_subtitle(Some(reader), true).contains("works like System"));
}

#[test]
fn fr4_dpi_scaling_words_are_the_ones_dsa_takes() {
    let codes: Vec<&str> = DpiScaling::ALL.iter().map(|s| s.code()).collect();
    assert_eq!(codes, ["auto", "app", "system", "system-forced"]);
    for scaling in DpiScaling::ALL {
        assert_eq!(DpiScaling::read(scaling.code()), scaling);
    }
    assert_eq!(DpiScaling::read(""), DpiScaling::Auto);
    assert_eq!(DpiScaling::read("enhanced"), DpiScaling::Auto);
}
