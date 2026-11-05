//! The user's permission for the camera and the microphone, which macOS keeps per program
//! (its privacy settings, TCC).
//!
//! The first time an app in Android opens the camera or the microphone, the device lender
//! asks macOS, which shows its prompt with the reason in the lender's Info.plist, and waits
//! for the user's answer before it opens the device. An answer already given is not asked
//! again; a refusal is the user's, and the device reports itself as blocked until the user
//! allows it in System Settings, Privacy & Security. The location is asked for by
//! CoreLocation itself ([`super::location`]).

/// What macOS says about a program's access to a device (`AVAuthorizationStatus`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    /// Not asked yet: asking shows the prompt.
    NotAsked,
    /// Not allowed by a policy the user cannot change (parental controls, a profile).
    Restricted,
    /// The user said no.
    Denied,
    Allowed,
}

impl Permission {
    /// The permission `AVAuthorizationStatus` value `status` says.
    pub fn from_status(status: isize) -> Permission {
        match status {
            0 => Permission::NotAsked,
            1 => Permission::Restricted,
            2 => Permission::Denied,
            _ => Permission::Allowed,
        }
    }
}

/// A device macOS asks the user about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Device {
    Camera,
    Microphone,
}

impl Device {
    /// The `AVMediaType` for it.
    pub fn media_type(self) -> &'static str {
        match self {
            Device::Camera => "vide",
            Device::Microphone => "soun",
        }
    }

    /// What to tell the user who refused it.
    pub fn refused(self) -> &'static str {
        match self {
            Device::Camera => {
                "the user did not allow DSA to use the camera; it can be allowed in System \
                 Settings, Privacy & Security, Camera"
            }
            Device::Microphone => {
                "the user did not allow DSA to use the microphone; it can be allowed in System \
                 Settings, Privacy & Security, Microphone"
            }
        }
    }
}

#[cfg(target_os = "macos")]
pub use system::{ask, status};

#[cfg(target_os = "macos")]
mod system {
    use std::sync::mpsc::channel;
    use std::time::Duration;

    use block2::RcBlock;
    use objc2::msg_send;
    use objc2::runtime::Bool;

    use super::{Device, Permission};
    use crate::objc::{class, ns_string};

    #[link(name = "AVFoundation", kind = "framework")]
    extern "C" {}

    /// How long the prompt is waited for. The user may leave it open; the device is not
    /// opened meanwhile, and the next open asks again.
    const ANSWER: Duration = Duration::from_secs(120);

    /// What macOS says now about `device`.
    pub fn status(device: Device) -> Permission {
        let Ok(capture) = class(c"AVCaptureDevice") else {
            return Permission::Restricted;
        };
        let media = ns_string(device.media_type());
        let status: isize = unsafe { msg_send![capture, authorizationStatusForMediaType: &*media] };
        Permission::from_status(status)
    }

    /// Asks the user for `device` when they were not asked yet, and waits for the answer.
    /// `Ok` when it may be used; the reason otherwise.
    pub fn ask(device: Device) -> Result<(), String> {
        match status(device) {
            Permission::Allowed => return Ok(()),
            Permission::Denied => return Err(device.refused().into()),
            Permission::Restricted => {
                return Err(format!(
                    "a policy on this Mac does not let programs use the {}",
                    match device {
                        Device::Camera => "camera",
                        Device::Microphone => "microphone",
                    }
                ))
            }
            Permission::NotAsked => {}
        }
        let capture = class(c"AVCaptureDevice")?;
        let media = ns_string(device.media_type());
        let (answer, answered) = channel();
        let handler = RcBlock::new(move |granted: Bool| {
            let _ = answer.send(granted.as_bool());
        });
        unsafe {
            let _: () = msg_send![
                capture,
                requestAccessForMediaType: &*media,
                completionHandler: &*handler
            ];
        }
        match answered.recv_timeout(ANSWER) {
            Ok(true) => Ok(()),
            Ok(false) => Err(device.refused().into()),
            Err(_) => Err("the user has not answered macOS's prompt yet".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr26_the_users_answer_decides_access() {
        assert_eq!(Permission::from_status(0), Permission::NotAsked);
        assert_eq!(Permission::from_status(1), Permission::Restricted);
        assert_eq!(Permission::from_status(2), Permission::Denied);
        assert_eq!(Permission::from_status(3), Permission::Allowed);
        assert_eq!(Device::Camera.media_type(), "vide");
        assert_eq!(Device::Microphone.media_type(), "soun");
        assert!(Device::Microphone.refused().contains("Privacy & Security"));
    }
}
