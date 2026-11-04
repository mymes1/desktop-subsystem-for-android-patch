//! The ARM translation package the host installs in the guest, through the router as the host
//! reaches it, with a scratch folder standing in for the user's disk.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Arc;

use dsa_guest_channeld::arm_translation::{ArmTranslation, BootFacts, IMAGE};
use dsa_guest_channeld::router::Router;
use dsa_protocol::control::envelope::Body;
use dsa_protocol::control::{
    ArmTranslationChunk, ArmTranslationInstall, ArmTranslationQuery, ArmTranslationRemove,
    ArmTranslationState, Envelope,
};
use sha2::{Digest, Sha256};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "dsa-channeld-arm-{name}-{}",
        std::process::id()
    ));
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

fn install(router: &Router, id: u32, image: &[u8], sha256: Vec<u8>, piece: usize) {
    send(
        router,
        Body::ArmTranslationInstall(ArmTranslationInstall {
            transfer_id: id,
            size: image.len() as u64,
            sha256,
        }),
    );
    for (index, data) in image.chunks(piece).enumerate() {
        send(
            router,
            Body::ArmTranslationChunk(ArmTranslationChunk {
                transfer_id: id,
                offset: (index * piece) as u64,
                data: data.to_vec(),
            }),
        );
    }
}

#[test]
fn fr25_an_image_that_arrives_whole_is_put_in_place_and_reported() {
    let dir = scratch("whole");
    let (router, from_guest) = router(&dir);
    send(&router, Body::ArmTranslationQuery(ArmTranslationQuery {}));
    let before = answer(&from_guest);
    assert!(!before.installed);
    assert_eq!(before.abilist, "x86_64");

    let image: Vec<u8> = (0..10_000u32).map(|i| i as u8).collect();
    install(&router, 7, &image, Sha256::digest(&image).to_vec(), 4096);
    let answers: Vec<_> = std::iter::from_fn(|| from_guest.try_recv().ok()).collect();
    assert_eq!(answers.len(), 4, "one answer for the start and one per chunk");
    let last = match dsa_protocol::decode(answers.last().unwrap()).unwrap().body {
        Some(Body::ArmTranslationState(state)) => state,
        other => panic!("unexpected {other:?}"),
    };
    assert!(last.done, "{}", last.error);
    assert!(last.installed);
    assert_eq!(last.transfer_id, 7);
    assert_eq!(last.received, image.len() as u64);
    assert_eq!(std::fs::read(dir.join(IMAGE)).unwrap(), image);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn fr25_an_image_that_does_not_match_its_digest_leaves_the_old_one() {
    let dir = scratch("digest");
    let (router, from_guest) = router(&dir);
    let old = b"old image".to_vec();
    install(&router, 1, &old, Sha256::digest(&old).to_vec(), 1024);
    while from_guest.try_recv().is_ok() {}

    let new = b"new image, damaged on the way".to_vec();
    install(&router, 2, &new, Sha256::digest(b"something else").to_vec(), 1024);
    let answers: Vec<_> = std::iter::from_fn(|| from_guest.try_recv().ok()).collect();
    let last = match dsa_protocol::decode(answers.last().unwrap()).unwrap().body {
        Some(Body::ArmTranslationState(state)) => state,
        other => panic!("unexpected {other:?}"),
    };
    assert!(!last.done);
    assert!(last.error.contains("SHA-256"), "{}", last.error);
    assert_eq!(std::fs::read(dir.join(IMAGE)).unwrap(), old);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn fr25_a_piece_out_of_place_or_an_empty_image_is_refused() {
    let dir = scratch("order");
    let (router, from_guest) = router(&dir);
    let image = vec![1u8; 100];
    send(
        &router,
        Body::ArmTranslationInstall(ArmTranslationInstall {
            transfer_id: 3,
            size: 100,
            sha256: Sha256::digest(&image).to_vec(),
        }),
    );
    assert!(answer(&from_guest).error.is_empty());
    send(
        &router,
        Body::ArmTranslationChunk(ArmTranslationChunk {
            transfer_id: 3,
            offset: 50,
            data: vec![1u8; 50],
        }),
    );
    let refused = answer(&from_guest);
    assert!(refused.error.contains("out of place"), "{}", refused.error);
    assert!(!refused.installed);

    send(
        &router,
        Body::ArmTranslationInstall(ArmTranslationInstall {
            transfer_id: 4,
            size: 0,
            sha256: vec![0; 32],
        }),
    );
    assert!(!answer(&from_guest).error.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn fr25_removing_the_package_deletes_its_image() {
    let dir = scratch("remove");
    let (router, from_guest) = router(&dir);
    let image = b"image".to_vec();
    install(&router, 1, &image, Sha256::digest(&image).to_vec(), 1024);
    while from_guest.try_recv().is_ok() {}
    send(&router, Body::ArmTranslationRemove(ArmTranslationRemove {}));
    let removed = answer(&from_guest);
    assert!(removed.done);
    assert!(!removed.installed);
    assert!(!dir.join(IMAGE).exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn fr25_the_package_messages_never_reach_the_guest_clients() {
    let dir = scratch("clients");
    let (router, from_guest) = router(&dir);
    send(&router, Body::ArmTranslationQuery(ArmTranslationQuery {}));
    // Only the daemon's own answer comes out; no listener is handed the frame.
    assert!(matches!(
        dsa_protocol::decode(&from_guest.try_recv().unwrap()).unwrap().body,
        Some(Body::ArmTranslationState(_))
    ));
    assert!(from_guest.try_recv().is_err());
    let _ = std::fs::remove_dir_all(&dir);
}
