//! The refresh rate of the user's fastest monitor, for the guest display's refresh rate.
//!
//! The service runs as root outside the user's graphical session and cannot ask the desktop
//! about its monitors. The window manager, which runs in that session, hears each output's
//! current mode from the Wayland compositor, and records the fastest one's rate in the user's
//! `tmp` folder whenever it changes ([`RefreshRecorder`]). The service reads it every few
//! seconds while the user's Android runs ([`read_record`]) and hands it to the session, which
//! with the power saver profile sets the rate the guest's display refreshes at. Without a
//! record the guest keeps the rate it boots with.

use std::path::{Path, PathBuf};

use dsa_host_core::guest_refresh;

/// The record's name in the user's `tmp` folder.
const RECORD_NAME: &str = "monitor-refresh.txt";

/// More than any real record.
const MAX_RECORD_BYTES: usize = 64;

/// Where the record is, in the user's `tmp` folder `tmp`.
pub fn record_path(tmp: &Path) -> PathBuf {
    tmp.join(RECORD_NAME)
}

/// The rate recorded in `tmp`, in hertz, when there is a record with a rate in it.
pub fn read_record(tmp: &Path) -> Option<u32> {
    let bytes = std::fs::read(record_path(tmp)).ok()?;
    let text = std::str::from_utf8(bytes.get(..MAX_RECORD_BYTES.min(bytes.len()))?).ok()?;
    guest_refresh::parse_record(text)
}

/// Writes the record whole, so the service never reads half of one.
pub fn write_record(tmp: &Path, hz: u32) -> std::io::Result<()> {
    std::fs::create_dir_all(tmp)?;
    let path = record_path(tmp);
    let partial = path.with_extension("txt.new");
    std::fs::write(&partial, guest_refresh::record_text(hz))?;
    std::fs::rename(&partial, &path)
}

/// A Wayland output mode's refresh rate, which `wl_output` gives in millihertz, in whole
/// hertz: 59940 is 60. Zero or less is a rate the compositor does not know.
pub fn hz_from_millihertz(refresh: i32) -> u32 {
    if refresh <= 0 {
        return 0;
    }
    (refresh as u32).saturating_add(500) / 1000
}

/// Keeps the record up to date from the window manager's outputs.
pub struct RefreshRecorder {
    tmp: Option<PathBuf>,
    last: Option<u32>,
}

impl RefreshRecorder {
    /// Records in `tmp`, the user's `tmp` folder; with none, records nothing.
    pub fn new(tmp: Option<PathBuf>) -> RefreshRecorder {
        RefreshRecorder { tmp, last: None }
    }

    /// Records the fastest of `rates_mhz`, each output's current rate in millihertz, if it
    /// changed. Outputs whose rate is unknown leave the last record. A failure is returned
    /// once, not on every call.
    pub fn record(&mut self, rates_mhz: impl IntoIterator<Item = i32>) -> Result<(), String> {
        let Some(hz) = guest_refresh::fastest(rates_mhz.into_iter().map(hz_from_millihertz)) else {
            return Ok(());
        };
        if self.last == Some(hz) {
            return Ok(());
        }
        self.last = Some(hz);
        match &self.tmp {
            Some(tmp) => write_record(tmp, hz)
                .map_err(|e| format!("recording the monitors' refresh rate: {e}")),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dsa-refresh-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn fr10_a_wayland_output_rate_in_millihertz_is_whole_hertz() {
        assert_eq!(hz_from_millihertz(59_940), 60);
        assert_eq!(hz_from_millihertz(120_000), 120);
        assert_eq!(hz_from_millihertz(143_856), 144);
        assert_eq!(hz_from_millihertz(0), 0);
        assert_eq!(hz_from_millihertz(-1), 0);
    }

    #[test]
    fn fr10_battery_saver_caps_the_guest_refresh_rate_the_window_manager_recorded() {
        use dsa_host_core::guest_refresh::refresh_rate;
        let dir = empty_dir("record");
        assert_eq!(read_record(&dir), None);
        let mut recorder = RefreshRecorder::new(Some(dir.clone()));
        recorder.record([60_000, 119_880]).unwrap();
        let hz = read_record(&dir).unwrap();
        assert_eq!(hz, 120);
        assert_eq!(refresh_rate(hz, true), 60);
        assert_eq!(refresh_rate(hz, false), 120);
        // Outputs that name no rate leave the record as it was.
        recorder.record([0]).unwrap();
        assert_eq!(read_record(&dir), Some(120));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
