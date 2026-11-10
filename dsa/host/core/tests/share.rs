//! "Share on PC": the checks on what an app shares, what the host's share UI is given, and
//! the copy of the shared files from the guest.

use std::path::PathBuf;

use dsa_host_core::drag::{DragChunk, DragRead};
use dsa_host_core::share::{self, ShareContent, ShareOutcome, ShareRequest, ShareUi};
use dsa_protocol::control;

fn text(text: &str) -> control::DragItem {
    control::DragItem {
        mime_type: "text/plain".into(),
        is_text: true,
        text: text.into(),
        ..Default::default()
    }
}

fn file(name: &str, size: Option<u64>) -> control::DragItem {
    control::DragItem {
        name: name.into(),
        size: size.unwrap_or(0),
        size_known: size.is_some(),
        mime_type: "application/octet-stream".into(),
        ..Default::default()
    }
}

fn request(items: Vec<control::DragItem>) -> ShareRequest {
    ShareRequest::from_control(&control::ShareToHost {
        share_id: 5,
        title: String::new(),
        package_name: "com.example.notes".into(),
        app_label: "Notes".into(),
        items,
    })
    .unwrap()
}

/// A folder of its own for one test, empty.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dsa-share-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn fr36_links_and_text_are_told_apart_for_the_share_ui() {
    let shared = request(vec![
        text("https://example.com/a?b=c"),
        text("Meeting notes"),
        text("http://example.org"),
        text("see https://example.com"),
        text("  "),
    ]);
    let content = shared.content(Vec::new());
    assert_eq!(
        content,
        ShareContent {
            title: "Notes".into(),
            text: Some("Meeting notes\nsee https://example.com".into()),
            links: vec![
                "https://example.com/a?b=c".into(),
                "http://example.org".into()
            ],
            files: Vec::new(),
        }
    );
    assert!(!share::is_link("https://"));
    assert!(!share::is_link("ftp://example.com"));
    assert!(!share::is_link("https://exa mple.com"));
}

#[test]
fn fr36_a_share_that_breaks_the_rules_is_refused() {
    let empty = ShareRequest::from_control(&control::ShareToHost {
        share_id: 1,
        ..Default::default()
    });
    assert!(empty.is_err());
    let no_id = ShareRequest::from_control(&control::ShareToHost {
        share_id: 0,
        items: vec![text("x")],
        ..Default::default()
    });
    assert!(no_id.is_err());
    let many = ShareRequest::from_control(&control::ShareToHost {
        share_id: 1,
        items: vec![text("x"); share::MAX_SHARE_ITEMS + 1],
        ..Default::default()
    });
    assert!(many.is_err());
}

#[test]
fn fr36_shared_file_names_stay_in_the_share_folder_and_keep_apart() {
    let spool = scratch("names");
    let shared = request(vec![
        file("../../evil.txt", Some(3)),
        file("evil.txt", None),
    ]);
    let mut reads = Vec::new();
    let mut read = |read: DragRead| {
        reads.push(read);
        Ok(DragChunk {
            data: b"abc".to_vec(),
            end: true,
        })
    };
    let files = share::copy_files(&shared, &spool, &mut read).unwrap();
    assert_eq!(files.len(), 2);
    for path in &files {
        assert!(path.starts_with(&spool), "{}", path.display());
        assert_eq!(std::fs::read(path).unwrap(), b"abc");
    }
    assert_ne!(files[0], files[1]);
    assert_eq!(
        files[1].file_name().unwrap().to_string_lossy(),
        "evil (2).txt"
    );
    assert!(reads.iter().all(|r| r.drag_id == 5));
    assert_eq!(reads.iter().map(|r| r.item).collect::<Vec<_>>(), vec![0, 1]);
    let _ = std::fs::remove_dir_all(&spool);
}

#[test]
fn fr36_a_file_is_read_in_pieces_until_it_ends() {
    let spool = scratch("pieces");
    let shared = request(vec![file("big.bin", None)]);
    let mut offsets = Vec::new();
    let mut read = |read: DragRead| {
        offsets.push(read.offset);
        Ok(DragChunk {
            data: vec![7; 4],
            end: offsets.len() == 3,
        })
    };
    let files = share::copy_files(&shared, &spool, &mut read).unwrap();
    assert_eq!(offsets, vec![0, 4, 8]);
    assert_eq!(std::fs::read(&files[0]).unwrap(), vec![7; 12]);
    let _ = std::fs::remove_dir_all(&spool);
}

struct Never;

impl ShareUi for Never {
    fn share(&mut self, _: &ShareContent) -> ShareOutcome {
        panic!("the share UI is not shown for a share whose files could not be copied")
    }
}

#[test]
fn fr36_a_file_android_cannot_read_fails_the_share_and_leaves_nothing_behind() {
    let spool = scratch("unreadable");
    let shared = request(vec![file("a.txt", Some(1)), file("b.txt", Some(1))]);
    let mut read = |read: DragRead| {
        if read.item == 0 {
            Ok(DragChunk {
                data: b"a".to_vec(),
                end: true,
            })
        } else {
            Err("the provider is gone".to_string())
        }
    };
    let mut ended = Vec::new();
    let mut end = |id: u32, dropped: bool| ended.push((id, dropped));
    let outcome = share::share_one(&shared, &spool, &mut Never, &mut read, &mut end);
    assert!(matches!(outcome, ShareOutcome::Failed(why) if why.contains("b.txt")));
    assert_eq!(ended, vec![(5, false)], "Android gives the access back");
    assert_eq!(std::fs::read_dir(&spool).unwrap().count(), 0);
    let _ = std::fs::remove_dir_all(&spool);
}

#[test]
fn fr36_what_became_of_a_share_survives_the_local_api() {
    for outcome in [
        ShareOutcome::Shared,
        ShareOutcome::Canceled,
        ShareOutcome::Failed("no target".into()),
    ] {
        let wire = outcome.to_host_api(9);
        assert_eq!(wire.share_id, 9);
        assert_eq!(ShareOutcome::from_host_api(&wire), Ok(outcome.clone()));
        assert_eq!(outcome.to_control(9).share_id, 9);
    }
    let shared = request(vec![text("hi"), file("a.txt", Some(2))]);
    assert_eq!(
        ShareRequest::from_host_api(shared.to_host_api()),
        Ok(shared)
    );
}
