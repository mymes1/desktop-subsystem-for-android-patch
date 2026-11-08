//! The administrator prompt on Linux: polkit.
//!
//! When an Android app asks for root and the host's policy wants the administrator's word, the
//! service asks polkit to authorize DSA's action (`io.github.thisisthepy.DSA.root-elevation`,
//! installed in `/usr/share/polkit-1/actions` by the packages) for the requesting user's
//! desktop session, with user interaction allowed. The polkit agent of that session then asks
//! for an administrator's password, as it does for any other program; the action's default is
//! `auth_admin` and polkit keeps no authorization, so each grant is a fresh decision. The
//! details name the app, its package and the command, and set the prompt's message to them.
//!
//! polkit agents take an icon by theme name only, so the prompt shows DSA's icon: the app's
//! own picture is a file, which no agent reads.
//!
//! A prompt that times out is cancelled (`CancelCheckAuthorization`), which takes the agent's
//! dialog down. A dismissed dialog is a refusal.
//!
//! The rules (the details, which session is asked, what each answer means) are plain
//! functions, tested on every host through a fake [`Authority`]; the system bus behind
//! [`SystemBusAuthority`] is Linux only.

use std::collections::HashMap;

use dsa_host_core::root_approval::{AdminPrompt, ApprovalRequest, PromptAnswer};

/// DSA's polkit action, as the policy file defines it.
pub const ACTION_ID: &str = "io.github.thisisthepy.DSA.root-elevation";

/// The policy file's name, in the packaging folder and once installed.
pub const POLICY_FILE: &str = "io.github.thisisthepy.DSA.policy";

/// Where the packages install the policy file, the folder polkit reads actions from.
pub const POLICY_DIR: &str = "/usr/share/polkit-1/actions";

/// The icon the prompt shows: DSA's, from the hicolor theme the packages install it in.
pub const ICON_NAME: &str = "io.github.thisisthepy.DSA";

/// `CheckAuthorization`'s flag that lets polkit ask the person.
pub const ALLOW_USER_INTERACTION: u32 = 1;

/// The D-Bus error polkit answers with when the check was cancelled.
pub const CANCELLED_ERROR: &str = "org.freedesktop.PolicyKit1.Error.Cancelled";

/// The details the check carries. `polkit.message` and `polkit.icon_name` set the agent's
/// message and icon (polkit lets only a root caller set them, and the service is root); the
/// rest name the request for the agent and for rules that read them.
pub fn details(request: &ApprovalRequest) -> Vec<(String, String)> {
    let command = if request.command.is_empty() {
        "(an interactive root shell)".to_string()
    } else {
        request.command_line()
    };
    vec![
        ("polkit.message".into(), request.message()),
        ("polkit.icon_name".into(), ICON_NAME.into()),
        ("app".into(), request.app_name().to_string()),
        ("package".into(), request.package.clone()),
        ("command".into(), command),
    ]
}

/// The ID the check is cancelled by, unique to the request.
pub fn cancellation_id(uid: u32, request: &ApprovalRequest) -> String {
    format!("dsa-root-{uid}-{}", request.request_id)
}

/// One login session, as logind lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginSession {
    pub id: String,
    pub uid: u32,
    /// Whether the session is in front on its seat.
    pub active: bool,
    /// `wayland`, `x11`, `tty` and so on.
    pub kind: String,
    /// Whether the session is a remote one (ssh).
    pub remote: bool,
}

impl LoginSession {
    fn graphical(&self) -> bool {
        matches!(self.kind.as_str(), "wayland" | "x11" | "mir")
    }
}

/// The session the prompt goes to: the user's desktop session in front, else any desktop
/// session of theirs, else any local session of theirs. Their windows, and so the app's, are
/// in the desktop session; its polkit agent is the one that asks.
pub fn pick_session(sessions: &[LoginSession], uid: u32) -> Option<String> {
    let mine: Vec<&LoginSession> = sessions
        .iter()
        .filter(|s| s.uid == uid && !s.remote)
        .collect();
    let rank = |s: &&LoginSession| match (s.graphical(), s.active) {
        (true, true) => 0,
        (true, false) => 1,
        (false, true) => 2,
        (false, false) => 3,
    };
    mine.iter().min_by_key(rank).map(|s| s.id.clone())
}

/// What `CheckAuthorization` answered.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CheckResult {
    pub is_authorized: bool,
    pub is_challenge: bool,
    pub details: HashMap<String, String>,
}

/// Why `CheckAuthorization` gave no answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckError {
    /// The check was cancelled, as after a timeout.
    Cancelled,
    /// polkit could not be reached, or refused the call; the detail says why.
    Failed(String),
}

/// What the check's answer means for the request. A dialog the person closed is a refusal
/// polkit marks `polkit.dismissed`; a cancelled check is the host's own withdrawal.
pub fn answer_of(result: Result<CheckResult, CheckError>) -> PromptAnswer {
    match result {
        Ok(result) if result.is_authorized => PromptAnswer::Approved,
        Ok(result) => {
            if result.details.get("polkit.dismissed").map(String::as_str) == Some("true") {
                PromptAnswer::Cancelled
            } else if result.is_challenge {
                // Only returned without user interaction, which is always allowed here.
                PromptAnswer::Unavailable("polkit would not ask the person".to_string())
            } else {
                PromptAnswer::Denied
            }
        }
        Err(CheckError::Cancelled) => PromptAnswer::Cancelled,
        Err(CheckError::Failed(why)) => PromptAnswer::Unavailable(why),
    }
}

/// polkit and logind, as the prompt uses them.
pub trait Authority: Send + Sync {
    /// Every login session.
    fn sessions(&self) -> Result<Vec<LoginSession>, String>;
    /// Checks DSA's action for the session, letting polkit ask the person, and waits for the
    /// answer.
    fn check(
        &self,
        session_id: &str,
        details: &[(String, String)],
        cancellation_id: &str,
    ) -> Result<CheckResult, CheckError>;
    /// Cancels the check made with `cancellation_id`.
    fn cancel(&self, cancellation_id: &str);
}

/// The administrator prompt of one user's Android, through polkit.
pub struct PolkitPrompt<A: Authority> {
    authority: A,
    uid: u32,
}

impl<A: Authority> PolkitPrompt<A> {
    /// The prompt for the user `uid`, asked in their desktop session.
    pub fn new(authority: A, uid: u32) -> PolkitPrompt<A> {
        PolkitPrompt { authority, uid }
    }
}

impl<A: Authority> AdminPrompt for PolkitPrompt<A> {
    fn ask(&self, request: &ApprovalRequest) -> PromptAnswer {
        let sessions = match self.authority.sessions() {
            Ok(sessions) => sessions,
            Err(why) => return PromptAnswer::Unavailable(format!("logind: {why}")),
        };
        let Some(session) = pick_session(&sessions, self.uid) else {
            return PromptAnswer::Unavailable(format!(
                "user {} has no login session on this computer to ask in",
                self.uid
            ));
        };
        answer_of(self.authority.check(
            &session,
            &details(request),
            &cancellation_id(self.uid, request),
        ))
    }

    fn withdraw(&self, request: &ApprovalRequest) {
        self.authority.cancel(&cancellation_id(self.uid, request));
    }
}

#[cfg(target_os = "linux")]
pub use system_bus::SystemBusAuthority;

#[cfg(target_os = "linux")]
mod system_bus {
    use std::collections::HashMap;

    use zbus::blocking::{Connection, Proxy};
    use zbus::zvariant::{OwnedObjectPath, Value};

    use super::{
        Authority, CheckError, CheckResult, LoginSession, ACTION_ID, ALLOW_USER_INTERACTION,
        CANCELLED_ERROR,
    };

    const POLKIT: &str = "org.freedesktop.PolicyKit1";
    const POLKIT_PATH: &str = "/org/freedesktop/PolicyKit1/Authority";
    const POLKIT_AUTHORITY: &str = "org.freedesktop.PolicyKit1.Authority";
    const LOGIND: &str = "org.freedesktop.login1";
    const LOGIND_PATH: &str = "/org/freedesktop/login1";
    const LOGIND_MANAGER: &str = "org.freedesktop.login1.Manager";
    const LOGIND_SESSION: &str = "org.freedesktop.login1.Session";

    /// polkit and logind on the system bus. One connection makes both the check and its
    /// cancellation, as polkit takes a cancellation only from the caller that made the check.
    pub struct SystemBusAuthority {
        connection: Connection,
    }

    impl SystemBusAuthority {
        pub fn connect() -> Result<SystemBusAuthority, String> {
            let connection = Connection::system().map_err(|e| format!("the system bus: {e}"))?;
            Ok(SystemBusAuthority { connection })
        }
    }

    impl Authority for SystemBusAuthority {
        fn sessions(&self) -> Result<Vec<LoginSession>, String> {
            let reply = self
                .connection
                .call_method(
                    Some(LOGIND),
                    LOGIND_PATH,
                    Some(LOGIND_MANAGER),
                    "ListSessions",
                    &(),
                )
                .map_err(|e| format!("listing the sessions: {e}"))?;
            let listed: Vec<(String, u32, String, String, OwnedObjectPath)> = reply
                .body()
                .deserialize()
                .map_err(|e| format!("the session list is unreadable: {e}"))?;
            let mut sessions = Vec::new();
            for (id, uid, _user, _seat, path) in listed {
                let Ok(session) =
                    Proxy::new(&self.connection, LOGIND, path.as_str(), LOGIND_SESSION)
                else {
                    continue;
                };
                sessions.push(LoginSession {
                    id,
                    uid,
                    active: session.get_property::<bool>("Active").unwrap_or(false),
                    kind: session.get_property::<String>("Type").unwrap_or_default(),
                    remote: session.get_property::<bool>("Remote").unwrap_or(false),
                });
            }
            Ok(sessions)
        }

        fn check(
            &self,
            session_id: &str,
            details: &[(String, String)],
            cancellation_id: &str,
        ) -> Result<CheckResult, CheckError> {
            let mut subject: HashMap<&str, Value<'_>> = HashMap::new();
            subject.insert("session-id", Value::from(session_id));
            let details: HashMap<&str, &str> = details
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str()))
                .collect();
            let reply = self.connection.call_method(
                Some(POLKIT),
                POLKIT_PATH,
                Some(POLKIT_AUTHORITY),
                "CheckAuthorization",
                &(
                    ("unix-session", subject),
                    ACTION_ID,
                    details,
                    ALLOW_USER_INTERACTION,
                    cancellation_id,
                ),
            );
            let reply = match reply {
                Ok(reply) => reply,
                Err(zbus::Error::MethodError(name, _, _)) if name.as_str() == CANCELLED_ERROR => {
                    return Err(CheckError::Cancelled)
                }
                Err(e) => return Err(CheckError::Failed(format!("polkit: {e}"))),
            };
            let (is_authorized, is_challenge, details): (bool, bool, HashMap<String, String>) =
                reply
                    .body()
                    .deserialize()
                    .map_err(|e| CheckError::Failed(format!("polkit's answer: {e}")))?;
            Ok(CheckResult {
                is_authorized,
                is_challenge,
                details,
            })
        }

        fn cancel(&self, cancellation_id: &str) {
            if let Err(e) = self.connection.call_method(
                Some(POLKIT),
                POLKIT_PATH,
                Some(POLKIT_AUTHORITY),
                "CancelCheckAuthorization",
                &(cancellation_id,),
            ) {
                eprintln!("root: the timed out administrator prompt was not taken down: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct FakeAuthority {
        sessions: Vec<LoginSession>,
        answer: Result<CheckResult, CheckError>,
        checks: Mutex<Vec<(String, Vec<(String, String)>, String)>>,
        cancelled: Mutex<Vec<String>>,
    }

    impl FakeAuthority {
        fn answering(answer: Result<CheckResult, CheckError>) -> FakeAuthority {
            FakeAuthority {
                sessions: vec![
                    session("c1", 1000, false, "tty"),
                    session("2", 1000, true, "wayland"),
                    session("3", 1001, true, "wayland"),
                ],
                answer,
                checks: Mutex::new(Vec::new()),
                cancelled: Mutex::new(Vec::new()),
            }
        }
    }

    impl Authority for FakeAuthority {
        fn sessions(&self) -> Result<Vec<LoginSession>, String> {
            Ok(self.sessions.clone())
        }
        fn check(
            &self,
            session_id: &str,
            details: &[(String, String)],
            cancellation_id: &str,
        ) -> Result<CheckResult, CheckError> {
            self.checks.lock().unwrap().push((
                session_id.to_string(),
                details.to_vec(),
                cancellation_id.to_string(),
            ));
            self.answer.clone()
        }
        fn cancel(&self, cancellation_id: &str) {
            self.cancelled
                .lock()
                .unwrap()
                .push(cancellation_id.to_string());
        }
    }

    fn session(id: &str, uid: u32, active: bool, kind: &str) -> LoginSession {
        LoginSession {
            id: id.into(),
            uid,
            active,
            kind: kind.into(),
            remote: false,
        }
    }

    fn request() -> ApprovalRequest {
        ApprovalRequest {
            request_id: 42,
            package: "com.example.backup".into(),
            app_label: "Backup".into(),
            icon_png: Vec::new(),
            uid: 10_200,
            command: vec!["id".into()],
            interactive: false,
        }
    }

    fn authorized(yes: bool, dismissed: bool) -> Result<CheckResult, CheckError> {
        let mut details = HashMap::new();
        if dismissed {
            details.insert("polkit.dismissed".to_string(), "true".to_string());
        }
        Ok(CheckResult {
            is_authorized: yes,
            is_challenge: false,
            details,
        })
    }

    #[test]
    fn fr35_polkit_answers_map_to_approved_denied_and_dismissed() {
        assert_eq!(answer_of(authorized(true, false)), PromptAnswer::Approved);
        assert_eq!(answer_of(authorized(false, false)), PromptAnswer::Denied);
        assert_eq!(answer_of(authorized(false, true)), PromptAnswer::Cancelled);
        assert_eq!(
            answer_of(Err(CheckError::Cancelled)),
            PromptAnswer::Cancelled
        );
        assert!(matches!(
            answer_of(Err(CheckError::Failed("no polkit".into()))),
            PromptAnswer::Unavailable(_)
        ));
    }

    #[test]
    fn fr35_the_polkit_check_names_the_app_in_the_users_desktop_session() {
        let prompt = PolkitPrompt::new(FakeAuthority::answering(authorized(true, false)), 1000);
        assert_eq!(prompt.ask(&request()), PromptAnswer::Approved);
        let checks = prompt.authority.checks.lock().unwrap();
        let (session, details, cancel) = &checks[0];
        assert_eq!(session, "2");
        assert_eq!(cancel, "dsa-root-1000-42");
        let detail = |key: &str| {
            details
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.clone())
                .unwrap()
        };
        assert_eq!(
            detail("polkit.message"),
            "The Android app Backup (com.example.backup) wants to run `id` as root."
        );
        assert_eq!(detail("polkit.icon_name"), ICON_NAME);
        assert_eq!(detail("app"), "Backup");
        assert_eq!(detail("package"), "com.example.backup");
        assert_eq!(detail("command"), "id");
    }

    #[test]
    fn fr35_a_user_without_a_session_is_not_asked_and_is_refused() {
        let prompt = PolkitPrompt::new(FakeAuthority::answering(authorized(true, false)), 2000);
        assert!(matches!(
            prompt.ask(&request()),
            PromptAnswer::Unavailable(_)
        ));
        assert!(prompt.authority.checks.lock().unwrap().is_empty());
    }

    #[test]
    fn fr35_a_timed_out_polkit_check_is_cancelled_by_its_id() {
        let prompt = PolkitPrompt::new(FakeAuthority::answering(authorized(true, false)), 1000);
        prompt.withdraw(&request());
        assert_eq!(
            *prompt.authority.cancelled.lock().unwrap(),
            vec!["dsa-root-1000-42".to_string()]
        );
    }

    #[test]
    fn fr35_a_remote_session_is_never_asked() {
        let mut remote = session("9", 1000, true, "wayland");
        remote.remote = true;
        assert_eq!(pick_session(&[remote], 1000), None);
        assert_eq!(
            pick_session(
                &[
                    session("a", 1000, false, "wayland"),
                    session("b", 1000, true, "tty")
                ],
                1000
            ),
            Some("a".into())
        );
    }

    /// The packaging's policy file defines the action the prompt checks, asking for an
    /// administrator every time.
    #[test]
    fn fr35_the_polkit_policy_defines_the_action_for_administrators() {
        let policy = include_str!("../packaging/polkit/io.github.thisisthepy.DSA.policy");
        assert!(policy.contains(&format!("<action id=\"{ACTION_ID}\">")));
        for default in ["allow_any", "allow_inactive", "allow_active"] {
            assert!(
                policy.contains(&format!("<{default}>auth_admin</{default}>")),
                "{default} is not auth_admin"
            );
        }
        assert!(!policy.contains("auth_admin_keep"));
    }

    /// Both packages install the policy file where polkit reads actions.
    #[test]
    fn fr35_the_packages_install_the_polkit_policy() {
        let script = include_str!("../packaging/build-packages.sh");
        let installed = format!("{POLICY_DIR}/{POLICY_FILE}");
        assert!(
            script.contains(&format!("\"$here/polkit/{POLICY_FILE}\"")),
            "build-packages.sh does not install the polkit policy"
        );
        assert!(
            script.contains(&format!("\"$stage{POLICY_DIR}/\"")),
            "build-packages.sh does not install into {installed}"
        );
        let workflow = include_str!("../../../../../.github/workflows/test-linux-package.yml");
        assert!(
            workflow.contains(&installed),
            "the package test does not look for {installed}"
        );
    }
}
