//! The privileged permission allowlist the Google apps image carries.
//!
//! Android grants a privileged app (one in a partition's `priv-app`) a permission of the
//! platform's that is `privileged` or `signature|privileged` only when a `privapp-permissions`
//! list of the app's own partition names it. While `ro.control_privapp_permissions` is
//! `enforce`, a permission an app asks for that no list names stops `system_server` once the
//! system is ready, at every boot: Android never finishes starting. LiteGapps' lists are made
//! for a phone's Android and can lag behind the Android DSA runs (Google Play services asking
//! for `ACCESS_COMPANION_INFO` on Android 17 did exactly that).
//!
//! So the image carries a list of its own in each partition, [`FILE_NAME`]: every permission
//! each privileged app of that partition asks for that LiteGapps' lists there do not already
//! grant or deny. Every permission asked for is listed, not only the privileged ones, because
//! which permissions are privileged is the guest framework's to say (its `framework-res.apk`,
//! inside the system image the host never reads, and different between Android 16 and 17),
//! and Android reads the list only for the permissions that are privileged: an entry for any
//! other is never consulted, while an entry missing is a boot that never finishes. A permission
//! LiteGapps denies stays denied, since a list that both grants and denies one leaves the
//! outcome to the order Android reads the files in.

use std::collections::{BTreeMap, BTreeSet};

/// The list the image carries in each partition's `etc/permissions`.
pub const FILE_NAME: &str = "dsa-gapps-privapp-permissions.xml";

/// What the lists of one partition decide for one package.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Decisions {
    pub granted: BTreeSet<String>,
    pub denied: BTreeSet<String>,
}

/// Reads the `privapp-permissions` entries of the permission list `xml` into `into`, by
/// package. Anything else in the file is passed over, and so is a file that is not XML.
pub fn read_list(xml: &str, into: &mut BTreeMap<String, Decisions>) {
    let mut package: Option<String> = None;
    for tag in tags(xml) {
        let (name, attributes, closing, empty) = tag;
        match (name.as_str(), closing) {
            ("privapp-permissions", true) => package = None,
            ("privapp-permissions", false) => {
                package = attribute(&attributes, "package");
                if empty {
                    package = None;
                }
            }
            ("permission" | "deny-permission", false) => {
                let (Some(package), Some(permission)) =
                    (package.as_ref(), attribute(&attributes, "name"))
                else {
                    continue;
                };
                let decisions = into.entry(package.clone()).or_default();
                if name == "permission" {
                    decisions.granted.insert(permission);
                } else {
                    decisions.denied.insert(permission);
                }
            }
            _ => {}
        }
    }
}

/// The permissions of `requested` (by package) that `shipped` neither grants nor denies.
pub fn missing(
    requested: &BTreeMap<String, BTreeSet<String>>,
    shipped: &BTreeMap<String, Decisions>,
) -> BTreeMap<String, BTreeSet<String>> {
    let none = Decisions::default();
    requested
        .iter()
        .map(|(package, permissions)| {
            let decided = shipped.get(package).unwrap_or(&none);
            let left: BTreeSet<String> = permissions
                .iter()
                .filter(|p| !decided.granted.contains(*p) && !decided.denied.contains(*p))
                .cloned()
                .collect();
            (package.clone(), left)
        })
        .filter(|(_, left)| !left.is_empty())
        .collect()
}

/// The permission list granting `grants` (by package), as Android reads it.
pub fn write_list(grants: &BTreeMap<String, BTreeSet<String>>) -> String {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
         <!-- Made by DSA when it made this Google apps image: the permissions the privileged\n     \
         apps here ask for that the package's own lists do not decide. -->\n\
         <permissions>\n",
    );
    for (package, permissions) in grants {
        out.push_str(&format!(
            "    <privapp-permissions package=\"{}\">\n",
            escape(package)
        ));
        for permission in permissions {
            out.push_str(&format!(
                "        <permission name=\"{}\"/>\n",
                escape(permission)
            ));
        }
        out.push_str("    </privapp-permissions>\n");
    }
    out.push_str("</permissions>\n");
    out
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn unescape(text: &str) -> String {
    text.replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// One tag: its name, its attributes, whether it closes an element (`</name>`), and whether it
/// is empty (`<name/>`).
type Tag = (String, Vec<(String, String)>, bool, bool);

/// The tags of `xml`, comments, declarations and processing instructions left out.
fn tags(xml: &str) -> Vec<Tag> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find('<') {
        rest = &rest[start..];
        if let Some(after) = rest.strip_prefix("<!--") {
            match after.find("-->") {
                Some(end) => rest = &after[end + 3..],
                None => break,
            }
            continue;
        }
        let Some(end) = rest.find('>') else {
            break;
        };
        let inner = &rest[1..end];
        rest = &rest[end + 1..];
        if inner.starts_with('?') || inner.starts_with('!') {
            continue;
        }
        let (closing, inner) = match inner.strip_prefix('/') {
            Some(inner) => (true, inner),
            None => (false, inner),
        };
        let (empty, inner) = match inner.trim_end().strip_suffix('/') {
            Some(inner) => (true, inner),
            None => (false, inner),
        };
        let name_end = inner
            .find(|c: char| c.is_whitespace())
            .unwrap_or(inner.len());
        let name = inner[..name_end].to_string();
        out.push((name, attributes(&inner[name_end..]), closing, empty));
    }
    out
}

/// The `name="value"` pairs of a tag after its name.
fn attributes(mut text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    loop {
        text = text.trim_start();
        let Some(eq) = text.find('=') else {
            break;
        };
        let name = text[..eq].trim().to_string();
        text = text[eq + 1..].trim_start();
        let Some(quote) = text.chars().next().filter(|c| *c == '"' || *c == '\'') else {
            break;
        };
        let Some(close) = text[1..].find(quote) else {
            break;
        };
        out.push((name, unescape(&text[1..1 + close])));
        text = &text[close + 2..];
    }
    out
}

fn attribute(attributes: &[(String, String)], name: &str) -> Option<String> {
    attributes
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn fr32_litegapps_lists_are_read_by_package() {
        let mut shipped = BTreeMap::new();
        read_list(
            "<?xml version='1.0' encoding='utf-8'?>\n<!-- <permission name=\"in.a.comment\"/> -->\n\
             <permissions>\n  <privapp-permissions package=\"com.google.android.gms\">\n    \
             <permission name=\"android.permission.BACKUP\" />\n    \
             <deny-permission name='android.permission.REBOOT'/>\n  </privapp-permissions>\n  \
             <permission name=\"outside.a.package\"/>\n  \
             <privapp-permissions package=\"com.android.vending\"/>\n</permissions>\n",
            &mut shipped,
        );
        assert_eq!(
            shipped,
            BTreeMap::from([(
                "com.google.android.gms".to_string(),
                Decisions {
                    granted: set(&["android.permission.BACKUP"]),
                    denied: set(&["android.permission.REBOOT"]),
                }
            )])
        );
        read_list("not xml at all <<<", &mut shipped);
        assert_eq!(shipped.len(), 1);
    }

    #[test]
    fn fr32_the_allowlist_grants_what_the_shipped_lists_leave_undecided() {
        let requested = BTreeMap::from([
            (
                "com.google.android.gms".to_string(),
                set(&[
                    "android.permission.ACCESS_COMPANION_INFO",
                    "android.permission.BACKUP",
                    "android.permission.REBOOT",
                ]),
            ),
            (
                "com.android.vending".to_string(),
                set(&["android.permission.BACKUP"]),
            ),
        ]);
        let shipped = BTreeMap::from([
            (
                "com.google.android.gms".to_string(),
                Decisions {
                    granted: set(&["android.permission.BACKUP"]),
                    denied: set(&["android.permission.REBOOT"]),
                },
            ),
            (
                "com.android.vending".to_string(),
                Decisions {
                    granted: set(&["android.permission.BACKUP"]),
                    denied: BTreeSet::new(),
                },
            ),
        ]);
        let grants = missing(&requested, &shipped);
        assert_eq!(
            grants,
            BTreeMap::from([(
                "com.google.android.gms".to_string(),
                set(&["android.permission.ACCESS_COMPANION_INFO"])
            )])
        );
        let xml = write_list(&grants);
        // What is written reads back as the same grants.
        let mut again = BTreeMap::new();
        read_list(&xml, &mut again);
        assert_eq!(
            again["com.google.android.gms"].granted,
            grants["com.google.android.gms"]
        );
        assert!(xml.contains(
            "<privapp-permissions package=\"com.google.android.gms\">\n        \
             <permission name=\"android.permission.ACCESS_COMPANION_INFO\"/>"
        ));
        assert_eq!(
            write_list(&BTreeMap::new())
                .matches("<permissions>")
                .count(),
            1
        );
    }
}
