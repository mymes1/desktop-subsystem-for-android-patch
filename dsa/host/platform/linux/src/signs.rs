//! What a boot's console and Android's log show about a guest that is late, and what to tell
//! the user about it.

use std::path::Path;

/// What a boot has shown that explains a guest that is late, counted line by line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BootSigns {
    /// Lines where SELinux refused someone the control daemon's service.
    pub channel_denials: u64,
    /// The SELinux domain last refused it, such as `priv_app`.
    pub denied_domain: Option<String>,
    /// Lines where the hwcomposer could not reach the host's renderer.
    pub renderer_unreachable: u64,
}

impl BootSigns {
    /// Whether the agent was refused the channel for running as `priv_app`, which is what the
    /// package manager makes of a disk from other signing keys.
    pub fn agent_refused_for_keys(&self) -> bool {
        self.channel_denials > 0 && self.denied_domain.as_deref() == Some("priv_app")
    }

    /// Reads one line of the boot's console or of Android's log.
    pub fn read_line(&mut self, line: &str) {
        if line.contains("denied") && line.contains("IChannel/default") {
            self.channel_denials += 1;
            if let Some((_, rest)) = line.split_once("scontext=u:r:") {
                self.denied_domain = rest.split([':', ' ']).next().map(str::to_string);
            }
        } else if line.contains("cannot reach the host renderer") {
            self.renderer_unreachable += 1;
        }
    }
}

/// What to tell the user about a guest that has not finished booting.
pub fn explain_unfinished_boot(signs: &BootSigns, log: &Path) -> String {
    if signs.renderer_unreachable > 0 {
        format!(
            "Android could not reach its renderer {} times, so it cannot draw and keeps \
             restarting. The renderer (dsa-gfx) is started by DSA's window manager, which must \
             run in your desktop session. The whole boot is in {}.",
            signs.renderer_unreachable,
            log.display()
        )
    } else {
        format!(
            "The whole boot is in {}. A first boot on a new disk can take several minutes.",
            log.display()
        )
    }
}

/// What to tell the user about a guest that has booted without its agent.
pub fn explain_unreachable_agent(signs: &BootSigns, log: &Path, data_disk: &Path) -> String {
    let remedy = format!(
        "To start over, run `dsa reset`, or `dsa stop` and move {} somewhere else: Android makes \
         a new disk at its next start, without your apps and their data, which stay in the file \
         you moved.",
        data_disk.display()
    );
    if signs.channel_denials > 0 {
        let domain = signs
            .denied_domain
            .as_deref()
            .map(|domain| format!(" (it runs as {domain}, not platform_app)"))
            .unwrap_or_default();
        format!(
            "SELinux refused the agent the control channel {} times{domain}. That usually means \
             the data disk was made under an Android image signed with other keys. {remedy} The \
             whole boot is in {}.",
            signs.channel_denials,
            log.display()
        )
    } else {
        format!(
            "The agent may have crashed; the whole boot is in {}. If the data disk was made \
             under an Android image signed with other keys, that also stops the agent. {remedy}",
            log.display()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr2_an_agent_refused_for_its_keys_is_recognised() {
        let mut signs = BootSigns::default();
        signs.read_line(
            "avc: denied { find } for interface=dsa.IChannel/default \
             scontext=u:r:priv_app:s0:c512 tcontext=u:object_r:dsa_channel_service:s0",
        );
        assert!(signs.agent_refused_for_keys());
        let told = explain_unreachable_agent(
            &signs,
            Path::new("/h/logs/console.log"),
            Path::new("/h/data.qcow2"),
        );
        assert!(told.contains("priv_app") && told.contains("/h/data.qcow2"));
    }

    #[test]
    fn fr2_a_boot_without_its_renderer_says_so() {
        let mut signs = BootSigns::default();
        signs.read_line("hwcomposer: cannot reach the host renderer");
        assert!(explain_unfinished_boot(&signs, Path::new("/l")).contains("renderer 1 times"));
        assert!(!signs.agent_refused_for_keys());
    }
}
