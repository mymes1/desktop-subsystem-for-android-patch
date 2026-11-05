//! The host's share of each user's Android: its QEMU process, and the window processes in the
//! user's session, from `/proc`.

use dsa_host_core::memory::{HostMemory, HostProcessMemory, HostRole};

/// `VmRSS` from a `/proc/<pid>/status`, in KiB.
pub fn resident_kb(status: &str) -> Option<u64> {
    kb_field(status, "VmRSS:")
}

/// The memory only this process has, from a `/proc/<pid>/smaps_rollup`, in KiB: its private
/// pages, resident or swapped out.
pub fn private_kb(smaps_rollup: &str) -> Option<u64> {
    let clean = kb_field(smaps_rollup, "Private_Clean:")?;
    let dirty = kb_field(smaps_rollup, "Private_Dirty:")?;
    let swap = kb_field(smaps_rollup, "Swap:").unwrap_or(0);
    Some(clean + dirty + swap)
}

fn kb_field(text: &str, name: &str) -> Option<u64> {
    text.lines()
        .find_map(|line| line.strip_prefix(name))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|kb| kb.parse().ok())
}

/// The owner's user ID, from a `/proc/<pid>/status`.
pub fn owner_uid(status: &str) -> Option<u32> {
    status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|uid| uid.parse().ok())
}

/// The role of a process named `name` in a user's session, if it is one of DSA's windows.
pub fn window_role(name: &str) -> Option<HostRole> {
    match name {
        "dsa-compositor" => Some(HostRole::WindowManager),
        "dsa-app-window" => Some(HostRole::AppWindow),
        _ => None,
    }
}

fn process(pid: u32, role: HostRole) -> Option<HostProcessMemory> {
    let dir = format!("/proc/{pid}");
    let status = std::fs::read_to_string(format!("{dir}/status")).ok()?;
    let working_set_kb = resident_kb(&status)?;
    let private = std::fs::read_to_string(format!("{dir}/smaps_rollup"))
        .ok()
        .and_then(|text| private_kb(&text))
        .unwrap_or(working_set_kb);
    Some(HostProcessMemory {
        role,
        pid,
        working_set_kb,
        private_kb: private,
    })
}

/// [`HostMemory`] from `/proc`.
pub struct ProcHostMemory;

impl HostMemory for ProcHostMemory {
    fn processes(&self, user: &str, vm_id: Option<&str>) -> Vec<HostProcessMemory> {
        let mut found = Vec::new();
        if let Some(pid) = vm_id
            .and_then(|cid| cid.parse().ok())
            .and_then(crate::launcher::vm_process)
        {
            found.extend(process(pid, HostRole::Vm));
        }
        let Ok(uid) = crate::users::uid_of(user) else {
            return found;
        };
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return found;
        };
        for entry in entries.flatten() {
            let Some(pid) = entry.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else {
                continue;
            };
            let Ok(name) = std::fs::read_to_string(entry.path().join("comm")) else {
                continue;
            };
            let Some(role) = window_role(name.trim()) else {
                continue;
            };
            let owned = std::fs::read_to_string(entry.path().join("status"))
                .ok()
                .and_then(|status| owner_uid(&status));
            if owned == Some(uid) {
                found.extend(process(pid, role));
            }
        }
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nfr1_the_vms_resident_and_private_memory_are_read_from_proc() {
        let status = "Name:\tqemu-system-x86\nUid:\t1000\t1000\t1000\t1000\nVmRSS:\t  812345 kB\n";
        assert_eq!(resident_kb(status), Some(812_345));
        assert_eq!(owner_uid(status), Some(1000));
        let rollup = "Rss: 9 kB\nPrivate_Clean:  100 kB\nPrivate_Dirty:  700000 kB\nSwap: 20 kB\n";
        assert_eq!(private_kb(rollup), Some(700_120));
        assert_eq!(window_role("dsa-compositor"), Some(HostRole::WindowManager));
        assert_eq!(window_role("bash"), None);
    }

    #[test]
    fn nfr1_this_process_is_measured() {
        let me = process(std::process::id(), HostRole::Vm).unwrap();
        assert!(me.working_set_kb > 0);
    }
}
