//! How DSA itself is updated on Linux: by the system's package manager, never by DSA.
//!
//! The .deb and the .rpm register DSA's signed package repository when they are installed,
//! so a new version arrives through `apt`, `dnf`, `zypper` or the desktop's software center
//! like any other package. The service therefore carries no updater of its own. `dsa update`
//! is answered with where updates come from instead of an error, so the user learns what to
//! run rather than that something is missing.

use dsa_host_core::host_api::{ProgramUpdate, UpdateProgram};

/// What `dsa update` and the settings say about updating DSA on Linux.
pub const PACKAGE_MANAGER_UPDATES: &str =
    "DSA is updated by the system's package manager from DSA's package repository: \
     use the software center, or run `sudo apt update && sudo apt upgrade` (Debian, Ubuntu), \
     `sudo dnf upgrade` (Fedora) or `sudo zypper update` (openSUSE)";

/// Answers `dsa update` by pointing at the package manager. It never downloads or installs.
#[derive(Debug, Default, Clone, Copy)]
pub struct PackageManagerUpdates;

impl UpdateProgram for PackageManagerUpdates {
    fn update_now(&self) -> Result<ProgramUpdate, String> {
        Ok(ProgramUpdate {
            installing: None,
            detail: PACKAGE_MANAGER_UPDATES.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr15_linux_update_points_at_the_package_manager_and_installs_nothing() {
        let answer = PackageManagerUpdates.update_now().unwrap();
        assert_eq!(answer.installing, None);
        assert!(answer.detail.contains("package manager"));
        assert!(answer.detail.contains("apt"));
        assert!(answer.detail.contains("dnf"));
    }
}
