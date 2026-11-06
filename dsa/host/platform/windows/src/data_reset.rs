//! Resetting a user's Android data on Windows: their data disk is replaced by a fresh one.
//!
//! The disk is where the service keeps it for that user (their profile's `DSA` folder, or the
//! configuration's `data_dir`), as [`HostConfig::user_vm`] finds it through the paths module.
//! The fresh disk is made the way a new user's is: a copy of the current Android image's disk
//! template, with the image version recorded beside it, or an empty disk for an installation
//! that boots a fixed system image. The image store and the settings folder are not touched.

use std::path::Path;
use std::sync::Arc;

use dsa_host_core::data_reset::TemplateDataDisk;
use dsa_host_core::update::store::ImageStore;

use crate::host_config::HostConfig;
use crate::user_disk::{bring_disk_to, prepare_user_data_disk};

/// The extension of the file beside a user's disk that records the image version it holds.
const IMAGE_VERSION: &str = "image-version";

/// The data reset of the service: `images` is the update store the next start boots from, if
/// the service uses one.
pub fn windows_data_reset(config: HostConfig, images: Option<Arc<ImageStore>>) -> TemplateDataDisk {
    let finder = config.clone();
    TemplateDataDisk::new(
        Box::new(move |sid: &str| {
            finder
                .user_vm(sid)
                .map(|user| user.config.data_disk)
                .map_err(|e| e.to_string())
        }),
        Box::new(move |disk: &Path| make_fresh_disk(&config, images.as_deref(), disk)),
    )
    .with_beside(IMAGE_VERSION)
}

fn make_fresh_disk(
    config: &HostConfig,
    images: Option<&ImageStore>,
    disk: &Path,
) -> Result<(), String> {
    let Some(store) = images else {
        return prepare_user_data_disk(disk, config.data_disk_size_gb);
    };
    let current = store.state().map_err(|e| e.to_string())?.current;
    let files = config.image_files(Some(&store.image_dir(&current.version)));
    match &files.disk_template {
        Some(template) => bring_disk_to(disk, template, &current.version),
        None => prepare_user_data_disk(disk, config.data_disk_size_gb),
    }
}
