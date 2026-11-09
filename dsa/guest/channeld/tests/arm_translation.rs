//! The ARM translation package's image an older host left on the user's disk, which the host
//! reads and then removes through the router as it reaches it, with a scratch folder standing
//! in for the user's disk.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Arc;

use dsa_guest_channeld::arm_translation::{ArmTranslation, BootFacts, IMAGE};
use dsa_guest_channeld::router::Router;
use dsa_protocol::control::envelope::Body;
use dsa_protocol::control::{
    AddonImage, AddonImageRead, ArmTranslationQuery, ArmTranslationRemove, ArmTranslationState,
    Envelope,
};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dsa-channeld-arm-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn router(dir: &Path) -> (Router, mpsc::Receiver<Vec<u8>>) {
    let (to_host, from_guest) = mpsc::channel();
    let facts = BootFacts {
        active: Box::new(|| false),
        abilist: Box::new(|| "x86_64".to_string()),
    };
    let router = Router::new(to_host, |_| Ok(()))
        .on_arm_translation(Arc::new(ArmTranslation::new(dir.to_path_buf(), facts)));
    (router, from_guest)
}

fn send(router: &Router, body: Body) {
    router.from_host(&dsa_protocol::encode(&Envelope { body: Some(body) }));
}

fn answer(from_guest: &mpsc::Receiver<Vec<u8>>) -> ArmTranslationState {
    let frame = from_guest.try_recv().expect("the daemon answers");
    match dsa_protocol::decode(&frame).unwrap().body {
        Some(Body::ArmTranslationState(state)) => state,
        other => panic!("expected ArmTranslationState, got {other:?}"),
    }
}

/// What an older host sent: the image in its folder.
fn left(dir: &Path, image: &[u8]) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join(IMAGE), image).unwrap();
}

#[test]
fn fr25_an_image_left_on_data_is_reported_and_read_in_parts() {
    let dir = scratch("read");
    let (router, from_guest) = router(&dir);
    send(&router, Body::ArmTranslationQuery(ArmTranslationQuery {}));
    let before = answer(&from_guest);
    assert!(!before.installed);
    assert_eq!(before.abilist, "x86_64");

    let image: Vec<u8> = (0..10_000u32).map(|i| (i * 7) as u8).collect();
    left(&dir, &image);
    send(&router, Body::ArmTranslationQuery(ArmTranslationQuery {}));
    assert!(answer(&from_guest).installed);
    send(
        &router,
        Body::AddonImageRead(AddonImageRead {
            image: AddonImage::ArmTranslation as i32,
            offset: 4096,
            length: 100,
        }),
    );
    let frame = from_guest.try_recv().unwrap();
    let part = match dsa_protocol::decode(&frame).unwrap().body {
        Some(Body::AddonImagePart(part)) => part,
        other => panic!("expected AddonImagePart, got {other:?}"),
    };
    assert!(part.error.is_empty(), "{}", part.error);
    assert_eq!(part.offset, 4096);
    assert_eq!(part.size, image.len() as u64);
    assert_eq!(part.data, image[4096..4196]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn fr25_removing_the_package_deletes_the_image_left_on_data() {
    let dir = scratch("remove");
    let (router, from_guest) = router(&dir);
    left(&dir, b"image");
    send(&router, Body::ArmTranslationRemove(ArmTranslationRemove {}));
    let removed = answer(&from_guest);
    assert!(removed.done, "{}", removed.error);
    assert!(!removed.installed);
    assert!(!dir.join(IMAGE).exists());
    // Removing what is not there is done too.
    send(&router, Body::ArmTranslationRemove(ArmTranslationRemove {}));
    assert!(answer(&from_guest).done);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn fr25_the_package_messages_never_reach_the_guest_clients() {
    let dir = scratch("clients");
    let (router, from_guest) = router(&dir);
    send(&router, Body::ArmTranslationQuery(ArmTranslationQuery {}));
    // Only the daemon's own answer comes out; no listener is handed the frame.
    assert!(matches!(
        dsa_protocol::decode(&from_guest.try_recv().unwrap())
            .unwrap()
            .body,
        Some(Body::ArmTranslationState(_))
    ));
    assert!(from_guest.try_recv().is_err());
    let peer = dsa_guest_channeld::router::Peer(1234);
    let state = Body::ArmTranslationState(ArmTranslationState::default());
    assert!(router
        .from_guest(peer, dsa_protocol::encode(&Envelope { body: Some(state) }))
        .is_err());
    let _ = std::fs::remove_dir_all(&dir);
}
