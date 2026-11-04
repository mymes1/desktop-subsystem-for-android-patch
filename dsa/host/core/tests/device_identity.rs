//! The device identity a user chooses, and how it reaches the guest at boot.

use dsa_host_core::bootconfig;
use dsa_host_core::device_identity::{
    append_overlay, ramdisk_overlay, split_fingerprint, DeviceIdentity, DeviceIdentityStore, Field,
    FileDeviceIdentities, ImageBuild, MemoryDeviceIdentities, RAMDISK_PROP_PATH,
};

fn build() -> ImageBuild {
    ImageBuild {
        release: "16".into(),
        id: "BP2A.250605.031".into(),
        incremental: "13690935.dsa0123456789ab".into(),
        tags: "release-keys".into(),
    }
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir =
        std::env::temp_dir().join(format!("dsa-device-identity-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// The files in a newc cpio archive, by name, the way the kernel unpacks it.
fn cpio_files(base: &[u8]) -> Vec<(String, u32, Vec<u8>)> {
    let mut files = Vec::new();
    let mut at = 0usize;
    loop {
        let header = &base[at..at + 110];
        assert_eq!(&header[..6], b"070701", "a newc header at {at}");
        let field = |i: usize| {
            u32::from_str_radix(
                std::str::from_utf8(&header[6 + i * 8..14 + i * 8]).unwrap(),
                16,
            )
            .unwrap()
        };
        let mode = field(1);
        let size = field(6) as usize;
        let name_size = field(11) as usize;
        let name = std::str::from_utf8(&base[at + 110..at + 110 + name_size - 1])
            .unwrap()
            .to_string();
        at = (at + 110 + name_size).div_ceil(4) * 4;
        let content = base[at..at + size].to_vec();
        at = (at + size).div_ceil(4) * 4;
        if name == "TRAILER!!!" {
            break;
        }
        files.push((name, mode, content));
    }
    assert_eq!(at, base.len(), "nothing after the trailer");
    files
}

fn property(text: &str, key: &str) -> Option<String> {
    text.lines()
        .find_map(|line| line.strip_prefix(&format!("{key}=")).map(str::to_string))
}

#[test]
fn fr17_the_defaults_are_dsas_own() {
    let e = DeviceIdentity::default().effective(Some(&build()));
    assert_eq!(e.manufacturer, "thisisthepy");
    assert_eq!(e.brand, "DSA");
    assert_eq!(e.model, "Desktop Subsystem for Android");
    assert_eq!(e.device, "dsa_x86_64");
    assert_eq!(e.name, "dsa_x86_64");
    assert_eq!(e.characteristics, "tablet");
    assert_eq!(
        e.fingerprint,
        "DSA/dsa_x86_64/dsa_x86_64:16/BP2A.250605.031/13690935.dsa0123456789ab:user/release-keys"
    );
}

#[test]
fn fr17_an_unchanged_identity_adds_nothing_to_the_boot() {
    assert_eq!(
        DeviceIdentity::default().boot_properties(Some(&build())),
        None
    );
    // Setting a value to its default is no change either.
    let mut identity = DeviceIdentity::default();
    identity
        .set(Field::Model, "Desktop Subsystem for Android")
        .unwrap();
    identity.set(Field::Brand, "DSA").unwrap();
    assert!(identity.is_default());
}

#[test]
fn fr17_a_changed_identity_reaches_every_partition() {
    let mut identity = DeviceIdentity::default();
    identity.set(Field::Manufacturer, "Example Corp.").unwrap();
    identity.set(Field::Brand, "example").unwrap();
    identity.set(Field::Model, "Example Tab (2026)").unwrap();
    identity.set(Field::Device, "ex_tab").unwrap();
    identity.set(Field::Name, "ex_tab_x86").unwrap();
    identity.set(Field::Characteristics, "phone").unwrap();
    let props = identity
        .boot_properties(Some(&build()))
        .expect("a property file");

    assert_eq!(
        property(&props, "ro.product.model").as_deref(),
        Some("Example Tab (2026)")
    );
    for partition in [
        "system",
        "system_ext",
        "vendor",
        "product",
        "odm",
        "vendor_dlkm",
    ] {
        assert_eq!(
            property(&props, &format!("ro.product.{partition}.manufacturer")).as_deref(),
            Some("Example Corp.")
        );
        assert_eq!(
            property(&props, &format!("ro.product.{partition}.device")).as_deref(),
            Some("ex_tab")
        );
        assert_eq!(
            property(&props, &format!("ro.{partition}.build.fingerprint")).as_deref(),
            Some("example/ex_tab_x86/ex_tab:16/BP2A.250605.031/13690935.dsa0123456789ab:user/release-keys")
        );
    }
    assert_eq!(
        property(&props, "ro.product.brand").as_deref(),
        Some("example")
    );
    assert_eq!(
        property(&props, "ro.product.name").as_deref(),
        Some("ex_tab_x86")
    );
    assert_eq!(
        property(&props, "ro.build.characteristics").as_deref(),
        Some("phone")
    );
    assert_eq!(
        property(&props, "ro.build.product").as_deref(),
        Some("ex_tab")
    );
    assert_eq!(
        property(&props, "ro.build.flavor").as_deref(),
        Some("ex_tab_x86-user")
    );
    assert_eq!(
        property(&props, "ro.build.fingerprint").as_deref(),
        Some("example/ex_tab_x86/ex_tab:16/BP2A.250605.031/13690935.dsa0123456789ab:user/release-keys")
    );
}

#[test]
fn fr17_fingerprint_parts_and_the_whole_string_combine_as_set() {
    let mut identity = DeviceIdentity::default();
    identity.set(Field::FingerprintType, "userdebug").unwrap();
    identity.set(Field::FingerprintTags, "dev-keys").unwrap();
    identity.set(Field::FingerprintRelease, "15").unwrap();
    let e = identity.effective(Some(&build()));
    assert_eq!(
        e.fingerprint,
        "DSA/dsa_x86_64/dsa_x86_64:15/BP2A.250605.031/13690935.dsa0123456789ab:userdebug/dev-keys"
    );

    // The whole string wins over its parts for Build.FINGERPRINT; each partition's own
    // fingerprint stays the composed one, which keeps the image's tag.
    identity
        .set(
            Field::Fingerprint,
            "google/oriole/oriole:14/UQ1A.240105.004/11206848:user/release-keys",
        )
        .unwrap();
    let props = identity.boot_properties(Some(&build())).unwrap();
    assert_eq!(
        property(&props, "ro.build.fingerprint").as_deref(),
        Some("google/oriole/oriole:14/UQ1A.240105.004/11206848:user/release-keys")
    );
    assert_eq!(
        property(&props, "ro.system.build.fingerprint").as_deref(),
        Some("DSA/dsa_x86_64/dsa_x86_64:15/BP2A.250605.031/13690935.dsa0123456789ab:userdebug/dev-keys")
    );
    // Without the image's build, only a whole string can be a fingerprint.
    let mut parts_only = DeviceIdentity::default();
    parts_only.set(Field::Brand, "example").unwrap();
    let props = parts_only.boot_properties(None).unwrap();
    assert_eq!(property(&props, "ro.build.fingerprint"), None);
    assert_eq!(
        property(&props, "ro.product.brand").as_deref(),
        Some("example")
    );
}

#[test]
fn fr17_values_that_are_not_allowed_are_refused() {
    let long_model = "x".repeat(65);
    let long_name = "n".repeat(33);
    let refused: [(Field, &str); 16] = [
        (Field::Manufacturer, " leading space"),
        (Field::Model, "line\nbreak"),
        (Field::Model, "\u{d55c}\u{ae00}"),
        (Field::Model, &long_model),
        (Field::Brand, "two words"),
        (Field::Brand, "slash/brand"),
        (Field::Device, "-starts-with-dash"),
        (Field::Name, &long_name),
        (Field::Characteristics, "Tablet"),
        (Field::Characteristics, "tablet,,phone"),
        (Field::FingerprintType, "debug"),
        (Field::FingerprintRelease, "16:1"),
        (Field::FingerprintTags, "release keys"),
        (
            Field::Fingerprint,
            "DSA/dsa_x86_64:16/ID/1:user/release-keys",
        ),
        (Field::Fingerprint, "DSA/dsa/dsa:16/ID/1:root/release-keys"),
        (
            Field::Fingerprint,
            "DSA/dsa/dsa:16/ID/1:user/release-keys:extra",
        ),
    ];
    for (field, value) in refused {
        let mut identity = DeviceIdentity::default();
        assert!(
            identity.set(field, value).is_err(),
            "{} {value:?} was taken",
            field.key()
        );
        assert!(identity.is_default(), "a refused value changed nothing");
    }
    let mut identity = DeviceIdentity::default();
    identity
        .set(Field::Characteristics, "tablet,nosdcard")
        .unwrap();
    identity
        .set(
            Field::Fingerprint,
            "DSA/dsa_x86_64/dsa_x86_64:16/BP2A.250605.031/1.dsaab:user/release-keys",
        )
        .unwrap();
    assert!(split_fingerprint("a/b/c:d/e/f:g/h").is_some());
    assert!(split_fingerprint("a/b/c:d/e/f:g").is_none());
}

#[test]
fn fr17_every_field_has_a_name_the_command_takes() {
    for field in Field::ALL {
        assert_eq!(Field::parse(field.key()).unwrap(), field);
    }
    assert!(Field::parse("serial")
        .unwrap_err()
        .contains("fingerprint.tags"));
}

#[test]
fn fr17_the_identity_is_kept_per_user_and_reset_restores_the_defaults() {
    let dir = scratch("store");
    let store = FileDeviceIdentities::new(&dir);
    let mut mine = DeviceIdentity::default();
    mine.set(Field::Model, "My tablet").unwrap();
    store.set("S-1-5-21-1", &mine).unwrap();
    assert_eq!(store.get("S-1-5-21-1").unwrap().model, "My tablet");
    assert!(
        store.get("S-1-5-21-2").unwrap().is_default(),
        "another user's is untouched"
    );
    assert!(dir.join("users").join("S-1-5-21-1.device.json").exists());

    // An identity with a value that is not allowed changes nothing.
    let broken = DeviceIdentity {
        brand: "no spaces allowed".into(),
        ..DeviceIdentity::default()
    };
    assert!(store.set("S-1-5-21-1", &broken).is_err());
    assert_eq!(store.get("S-1-5-21-1").unwrap().model, "My tablet");

    store.set("S-1-5-21-1", &DeviceIdentity::default()).unwrap();
    assert!(store.get("S-1-5-21-1").unwrap().is_default());
    assert!(!dir.join("users").join("S-1-5-21-1.device.json").exists());
    assert!(
        store.get("../escape").is_err(),
        "a user name never leaves the folder"
    );
    let _ = std::fs::remove_dir_all(&dir);

    let memory = MemoryDeviceIdentities::default();
    memory.set("me", &mine).unwrap();
    assert_eq!(memory.get("me").unwrap(), mine);
}

#[test]
fn fr17_the_identity_file_goes_in_the_initrd_before_the_bootconfig() {
    let props = "ro.product.model=My tablet\n";
    let overlay = ramdisk_overlay(props);
    let files = cpio_files(&overlay);
    let names: Vec<&str> = files.iter().map(|(name, _, _)| name.as_str()).collect();
    assert_eq!(
        names,
        [
            "system",
            "system/etc",
            "system/etc/ramdisk",
            RAMDISK_PROP_PATH
        ]
    );
    assert_eq!(files[0].1 & 0o170000, 0o040000, "a folder");
    assert_eq!(files[3].1, 0o100644, "a plain file init can read");
    assert_eq!(files[3].2, props.as_bytes());

    // An initrd that does not end on four bytes: the archive starts on the next boundary,
    // as the kernel needs for an uncompressed archive after a compressed one.
    let initrd = b"\x1f\x8b gzip of seven".to_vec();
    let combined = append_overlay(&initrd, &overlay);
    let start = initrd.len().div_ceil(4) * 4;
    assert_eq!(&combined[..initrd.len()], &initrd[..]);
    assert!(combined[initrd.len()..start].iter().all(|&b| b == 0));
    assert_eq!(&combined[start..], &overlay[..]);
    // The bootconfig still ends the initrd, where the kernel looks for it.
    let booted = bootconfig::append(&combined, "androidboot.example = \"1\"");
    assert!(booted.ends_with(b"#BOOTCONFIG\n"));
    assert_eq!(&booted[..combined.len()], &combined[..]);
}

#[test]
fn fr17_the_image_build_comes_from_image_json() {
    let manifest: serde_json::Value = serde_json::json!({
        "version": "17.0.0",
        "identity": {"release": "16", "id": "BP2A.250605.031",
                     "incremental": "13690935.dsa0123456789ab", "tags": "release-keys"}
    });
    assert_eq!(ImageBuild::from_manifest(&manifest), Some(build()));
    assert_eq!(
        ImageBuild::from_manifest(&serde_json::json!({"version": "1"})),
        None
    );
}
