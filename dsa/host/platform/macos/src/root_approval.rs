//! The administrator prompt on macOS: Authorization Services.
//!
//! When an Android app asks for root and the host's policy wants the administrator's word, the
//! agent asks Authorization Services for DSA's right (`io.github.thisisthepy.DSA.root-elevation`)
//! with interaction allowed. macOS then shows its own administrator dialog in the user's login
//! session, where the agent runs: an administrator's name and password approve. The dialog's
//! prompt names the app, its package and the command, and its icon is the app's own.
//!
//! The right is added to the authorization database the first time, with the system's
//! `authenticate-admin` rule: an administrator every time, nothing shared or kept, so each
//! grant is a fresh decision. DSA keeps its own per-app grace. Adding a right that does not
//! exist yet needs no prompt; a right that exists is left as the administrator has it.
//!
//! macOS has no call that takes the dialog down, so a prompt that times out stays until the
//! person answers it, and that late answer is ignored: the request has already failed.
//!
//! The rules (the right, the prompt's words, what each status means) are plain functions,
//! tested on every host through a fake [`Authorization`]; the calls into the Security framework
//! behind [`SecurityAuthorization`] are macOS only.

use std::path::{Path, PathBuf};

use dsa_host_core::root_approval::{AdminPrompt, ApprovalRequest, PromptAnswer};

/// DSA's right in the authorization database.
pub const RIGHT: &str = "io.github.thisisthepy.DSA.root-elevation";

/// The rule the right is added with: authenticate as an administrator, each time.
pub const RULE: &str = "authenticate-admin";

/// What the right is, as the authorization database describes it.
pub const DESCRIPTION: &str = "Run an Android app's command as root";

/// `errAuthorizationSuccess`.
pub const SUCCESS: i32 = 0;
/// `errAuthorizationDenied`: the credentials were wrong, or the account may not approve.
pub const DENIED: i32 = -60005;
/// `errAuthorizationCanceled`: the person closed the dialog.
pub const CANCELED: i32 = -60006;
/// `errAuthorizationInteractionNotAllowed`: no dialog can be shown, as outside a login
/// session.
pub const INTERACTION_NOT_ALLOWED: i32 = -60007;

/// What an Authorization Services status means for the request.
pub fn answer_of(status: i32) -> PromptAnswer {
    match status {
        SUCCESS => PromptAnswer::Approved,
        DENIED => PromptAnswer::Denied,
        CANCELED => PromptAnswer::Cancelled,
        INTERACTION_NOT_ALLOWED => PromptAnswer::Unavailable(
            "macOS would not show its administrator dialog outside a login session".to_string(),
        ),
        other => PromptAnswer::Unavailable(format!("Authorization Services failed ({other})")),
    }
}

/// Authorization Services, as the prompt uses it.
pub trait Authorization: Send + Sync {
    /// Adds DSA's right to the authorization database unless it is there.
    fn ensure_right(&self) -> Result<(), String>;
    /// Asks for DSA's right with the dialog showing `prompt` and the image at `icon`, and
    /// returns the status.
    fn copy_right(&self, prompt: &str, icon: Option<&Path>) -> i32;
}

/// The administrator prompt of the user's Android, through Authorization Services.
pub struct AuthorizationPrompt<A: Authorization> {
    api: A,
    /// Where an app's icon is written for the dialog to show.
    icons: PathBuf,
}

impl<A: Authorization> AuthorizationPrompt<A> {
    /// The prompt, writing the apps' icons for the dialog into `icons`.
    pub fn new(api: A, icons: PathBuf) -> AuthorizationPrompt<A> {
        AuthorizationPrompt { api, icons }
    }

    /// Writes the app's icon where the dialog reads it, when the guest gave one.
    fn icon_for(&self, request: &ApprovalRequest) -> Option<PathBuf> {
        if request.icon_png.is_empty() {
            return None;
        }
        let safe: String = request
            .package
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '.' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let path = self.icons.join(format!("{safe}.png"));
        std::fs::create_dir_all(&self.icons).ok()?;
        std::fs::write(&path, &request.icon_png).ok()?;
        Some(path)
    }
}

impl<A: Authorization> AdminPrompt for AuthorizationPrompt<A> {
    fn ask(&self, request: &ApprovalRequest) -> PromptAnswer {
        if let Err(why) = self.api.ensure_right() {
            return PromptAnswer::Unavailable(format!(
                "the right {RIGHT} is not in the authorization database: {why}"
            ));
        }
        let icon = self.icon_for(request);
        answer_of(self.api.copy_right(&request.message(), icon.as_deref()))
    }
}

#[cfg(target_os = "macos")]
pub use security::SecurityAuthorization;

#[cfg(target_os = "macos")]
mod security {
    use std::ffi::{c_char, c_void, CString};
    use std::path::Path;
    use std::ptr;

    use super::{Authorization, DENIED, DESCRIPTION, RIGHT, RULE, SUCCESS};

    #[repr(C)]
    struct AuthorizationItem {
        name: *const c_char,
        value_length: usize,
        value: *mut c_void,
        flags: u32,
    }

    #[repr(C)]
    struct AuthorizationItemSet {
        count: u32,
        items: *mut AuthorizationItem,
    }

    type AuthorizationRef = *mut c_void;
    type CFTypeRef = *const c_void;

    const FLAG_INTERACTION_ALLOWED: u32 = 1 << 0;
    const FLAG_EXTEND_RIGHTS: u32 = 1 << 1;
    const FLAG_DESTROY_RIGHTS: u32 = 1 << 3;
    const CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

    #[link(name = "Security", kind = "framework")]
    extern "C" {
        fn AuthorizationCreate(
            rights: *const AuthorizationItemSet,
            environment: *const AuthorizationItemSet,
            flags: u32,
            authorization: *mut AuthorizationRef,
        ) -> i32;
        fn AuthorizationCopyRights(
            authorization: AuthorizationRef,
            rights: *const AuthorizationItemSet,
            environment: *const AuthorizationItemSet,
            flags: u32,
            authorized_rights: *mut *mut AuthorizationItemSet,
        ) -> i32;
        fn AuthorizationFree(authorization: AuthorizationRef, flags: u32) -> i32;
        fn AuthorizationRightGet(right_name: *const c_char, definition: *mut CFTypeRef) -> i32;
        fn AuthorizationRightSet(
            authorization: AuthorizationRef,
            right_name: *const c_char,
            definition: CFTypeRef,
            description_key: CFTypeRef,
            bundle: CFTypeRef,
            locale_table_name: CFTypeRef,
        ) -> i32;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringCreateWithBytes(
            allocator: CFTypeRef,
            bytes: *const u8,
            length: isize,
            encoding: u32,
            external: u8,
        ) -> CFTypeRef;
        fn CFRelease(object: CFTypeRef);
    }

    /// A CoreFoundation string, released when dropped.
    struct CfString(CFTypeRef);

    impl CfString {
        fn new(text: &str) -> Option<CfString> {
            // SAFETY: the bytes are valid UTF-8 for their length, and the default allocator
            // is asked for.
            let string = unsafe {
                CFStringCreateWithBytes(
                    ptr::null(),
                    text.as_ptr(),
                    text.len() as isize,
                    CF_STRING_ENCODING_UTF8,
                    0,
                )
            };
            (!string.is_null()).then_some(CfString(string))
        }
    }

    impl Drop for CfString {
        fn drop(&mut self) {
            // SAFETY: the string was created here and is released once.
            unsafe { CFRelease(self.0) }
        }
    }

    /// An authorization reference, freed with its rights destroyed when dropped, so nothing
    /// granted outlives the one request.
    struct Session(AuthorizationRef);

    impl Session {
        fn create() -> Result<Session, i32> {
            let mut authorization: AuthorizationRef = ptr::null_mut();
            // SAFETY: no rights and no environment; the reference is written on success.
            let status =
                unsafe { AuthorizationCreate(ptr::null(), ptr::null(), 0, &mut authorization) };
            if status == SUCCESS && !authorization.is_null() {
                Ok(Session(authorization))
            } else {
                Err(status)
            }
        }
    }

    impl Drop for Session {
        fn drop(&mut self) {
            // SAFETY: the reference came from AuthorizationCreate and is freed once.
            unsafe {
                AuthorizationFree(self.0, FLAG_DESTROY_RIGHTS);
            }
        }
    }

    /// The Security framework's Authorization Services.
    pub struct SecurityAuthorization;

    impl Authorization for SecurityAuthorization {
        fn ensure_right(&self) -> Result<(), String> {
            let right = CString::new(RIGHT).map_err(|e| e.to_string())?;
            let mut definition: CFTypeRef = ptr::null();
            // SAFETY: the name is a NUL-terminated string; the definition, when given, is
            // released below.
            let status = unsafe { AuthorizationRightGet(right.as_ptr(), &mut definition) };
            if !definition.is_null() {
                // SAFETY: AuthorizationRightGet handed over this dictionary.
                unsafe { CFRelease(definition) };
            }
            if status == SUCCESS {
                return Ok(());
            }
            if status != DENIED {
                return Err(format!("reading the right failed ({status})"));
            }
            let session = Session::create().map_err(|s| format!("no authorization ({s})"))?;
            let rule = CfString::new(RULE).ok_or("no rule string")?;
            let description = CfString::new(DESCRIPTION).ok_or("no description string")?;
            // SAFETY: every pointer is live for the call; no bundle and no table, so the
            // description is used as it is.
            let status = unsafe {
                AuthorizationRightSet(
                    session.0,
                    right.as_ptr(),
                    rule.0,
                    description.0,
                    ptr::null(),
                    ptr::null(),
                )
            };
            if status == SUCCESS {
                Ok(())
            } else {
                Err(format!("adding the right failed ({status})"))
            }
        }

        fn copy_right(&self, prompt: &str, icon: Option<&Path>) -> i32 {
            let Ok(right) = CString::new(RIGHT) else {
                return -60008;
            };
            let session = match Session::create() {
                Ok(session) => session,
                Err(status) => return status,
            };
            let mut right_item = AuthorizationItem {
                name: right.as_ptr(),
                value_length: 0,
                value: ptr::null_mut(),
                flags: 0,
            };
            let rights = AuthorizationItemSet {
                count: 1,
                items: &mut right_item,
            };
            // The dialog's text and icon: kAuthorizationEnvironmentPrompt and
            // kAuthorizationEnvironmentIcon.
            let prompt_key = c"prompt";
            let icon_key = c"icon";
            let mut prompt_bytes = prompt.as_bytes().to_vec();
            let mut icon_bytes = icon
                .map(|path| path.to_string_lossy().into_owned().into_bytes())
                .unwrap_or_default();
            let mut environment_items = vec![AuthorizationItem {
                name: prompt_key.as_ptr(),
                value_length: prompt_bytes.len(),
                value: prompt_bytes.as_mut_ptr().cast(),
                flags: 0,
            }];
            if !icon_bytes.is_empty() {
                environment_items.push(AuthorizationItem {
                    name: icon_key.as_ptr(),
                    value_length: icon_bytes.len(),
                    value: icon_bytes.as_mut_ptr().cast(),
                    flags: 0,
                });
            }
            let environment = AuthorizationItemSet {
                count: environment_items.len() as u32,
                items: environment_items.as_mut_ptr(),
            };
            // SAFETY: the sets and the strings they point to live until the call returns;
            // the granted rights are not asked for back.
            unsafe {
                AuthorizationCopyRights(
                    session.0,
                    &rights,
                    &environment,
                    FLAG_INTERACTION_ALLOWED | FLAG_EXTEND_RIGHTS,
                    ptr::null_mut(),
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct FakeAuthorization {
        status: i32,
        right_missing: bool,
        asked: Mutex<Vec<(String, Option<PathBuf>)>>,
    }

    impl Authorization for FakeAuthorization {
        fn ensure_right(&self) -> Result<(), String> {
            if self.right_missing {
                Err("adding the right failed (-60005)".into())
            } else {
                Ok(())
            }
        }
        fn copy_right(&self, prompt: &str, icon: Option<&Path>) -> i32 {
            self.asked
                .lock()
                .unwrap()
                .push((prompt.to_string(), icon.map(Path::to_path_buf)));
            self.status
        }
    }

    fn fake(status: i32) -> FakeAuthorization {
        FakeAuthorization {
            status,
            right_missing: false,
            asked: Mutex::new(Vec::new()),
        }
    }

    fn request() -> ApprovalRequest {
        ApprovalRequest {
            request_id: 3,
            package: "com.example.backup".into(),
            app_label: "Backup".into(),
            icon_png: vec![0x89, b'P', b'N', b'G'],
            uid: 10_200,
            command: vec!["id".into()],
            interactive: false,
        }
    }

    fn icons(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("dsa-fr35-{name}-{}", std::process::id()))
    }

    #[test]
    fn fr35_authorization_statuses_map_to_approved_denied_and_cancelled() {
        assert_eq!(answer_of(SUCCESS), PromptAnswer::Approved);
        assert_eq!(answer_of(DENIED), PromptAnswer::Denied);
        assert_eq!(answer_of(CANCELED), PromptAnswer::Cancelled);
        assert!(matches!(
            answer_of(INTERACTION_NOT_ALLOWED),
            PromptAnswer::Unavailable(_)
        ));
        assert!(matches!(answer_of(-60008), PromptAnswer::Unavailable(_)));
    }

    #[test]
    fn fr35_the_macos_dialog_names_the_app_and_shows_its_icon() {
        let dir = icons("icon");
        let prompt = AuthorizationPrompt::new(fake(SUCCESS), dir.clone());
        assert_eq!(prompt.ask(&request()), PromptAnswer::Approved);
        let asked = prompt.api.asked.lock().unwrap();
        assert_eq!(
            asked[0].0,
            "The Android app Backup (com.example.backup) wants to run `id` as root."
        );
        let icon = asked[0].1.clone().expect("the app's icon");
        assert_eq!(icon, dir.join("com.example.backup.png"));
        assert_eq!(std::fs::read(&icon).unwrap(), request().icon_png);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn fr35_an_app_without_an_icon_is_asked_about_without_one() {
        let mut bare = request();
        bare.icon_png.clear();
        let prompt = AuthorizationPrompt::new(fake(CANCELED), icons("bare"));
        assert_eq!(prompt.ask(&bare), PromptAnswer::Cancelled);
        assert_eq!(prompt.api.asked.lock().unwrap()[0].1, None);
    }

    #[test]
    fn fr35_a_right_that_cannot_be_added_refuses_without_a_dialog() {
        let prompt = AuthorizationPrompt::new(
            FakeAuthorization {
                status: SUCCESS,
                right_missing: true,
                asked: Mutex::new(Vec::new()),
            },
            icons("missing"),
        );
        assert!(matches!(
            prompt.ask(&request()),
            PromptAnswer::Unavailable(_)
        ));
        assert!(prompt.api.asked.lock().unwrap().is_empty());
    }
}
