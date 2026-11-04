//! What the Windows cameras are offered as: the privacy settings that block them, where the
//! enclosure places them, and their rates.

use dsa_host_core::camera::{facing_of, Facing, Panel};
use dsa_host_windows::camera_capture::{consent_blocks, device_key, frame_rate, panel_from_windows};

#[test]
fn fr26_cameras_blocked_in_the_privacy_settings_are_not_offered() {
    let allow = Some("Allow".to_string());
    let deny = Some("Deny".to_string());
    assert!(!consent_blocks(&[allow.clone(), allow.clone(), allow.clone()]));
    // Nothing written yet: Windows allows.
    assert!(!consent_blocks(&[None, None, None]));
    // Camera access off for the user, for the machine, or for desktop apps.
    assert!(consent_blocks(&[deny.clone(), allow.clone(), allow.clone()]));
    assert!(consent_blocks(&[allow.clone(), deny.clone(), None]));
    assert!(consent_blocks(&[allow, None, deny]));
}

#[test]
fn fr26_the_enclosure_panel_gives_the_android_facing() {
    assert_eq!(facing_of(Some(panel_from_windows(1))), Facing::Front);
    assert_eq!(facing_of(Some(panel_from_windows(2))), Facing::Back);
    // Unknown, or on an edge: external.
    assert_eq!(panel_from_windows(0), Panel::Unknown);
    assert_eq!(facing_of(Some(panel_from_windows(0))), Facing::External);
    assert_eq!(facing_of(Some(panel_from_windows(3))), Facing::External);
    assert_eq!(facing_of(Some(panel_from_windows(42))), Facing::External);
}

#[test]
fn fr26_a_camera_and_its_enclosure_are_matched_by_device() {
    // Media Foundation's camera interface and the video capture interface of one device.
    let device = r"\\?\USB#VID_04F2&PID_B6DD&MI_00#6&1A2B3C4D&0&0000";
    let camera = format!(r"{device}#{{e5323777-f976-4f5b-9b55-b94699c46e44}}\GLOBAL");
    let capture = format!(r"{device}#{{65e8773d-8f56-11d0-a3b9-00a0c9223196}}\global");
    assert_eq!(device_key(&camera), device_key(&capture.to_lowercase()));
    let other = r"\\?\USB#VID_046D&PID_085B&MI_00#7&2&0&0000#{e5323777}\GLOBAL";
    assert_ne!(device_key(&camera), device_key(other));
}

#[test]
fn fr26_frame_rates_round_to_whole_frames() {
    assert_eq!(frame_rate(30, 1), 30);
    assert_eq!(frame_rate(30000, 1001), 30);
    assert_eq!(frame_rate(15, 2), 8);
    assert_eq!(frame_rate(30, 0), 0);
}
