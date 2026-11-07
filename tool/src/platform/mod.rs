//! OS-specific pieces: driver state, driver binding, bus reset, privileges.

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::*;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use self::linux::*;

#[cfg(not(any(windows, target_os = "linux")))]
mod other;
#[cfg(not(any(windows, target_os = "linux")))]
pub use self::other::*;

/// One line of the driver / host report shown to the user.
#[derive(Clone, Debug)]
pub struct ReportItem {
    /// Some(true) = fine, Some(false) = needs attention, None = informational.
    pub ok: Option<bool>,
    pub text: String,
}

#[allow(dead_code)]
impl ReportItem {
    pub fn good(text: impl Into<String>) -> Self {
        Self {
            ok: Some(true),
            text: text.into(),
        }
    }
    pub fn bad(text: impl Into<String>) -> Self {
        Self {
            ok: Some(false),
            text: text.into(),
        }
    }
    pub fn info(text: impl Into<String>) -> Self {
        Self {
            ok: None,
            text: text.into(),
        }
    }
}

/// Result of the host-side checks.
#[derive(Clone, Debug, Default)]
#[allow(dead_code)]
pub struct HostReport {
    pub items: Vec<ReportItem>,
    /// The vendor (M-Audio) driver package or kernel driver is present and must go first.
    pub vendor_driver: bool,
    /// Something the tool can fix itself (bind WinUSB, write a udev rule) is still missing.
    pub needs_setup: bool,
    /// The OS wants a reboot before device configuration is reliable.
    pub needs_reboot: bool,
    /// The tool is not running with the rights it needs.
    pub needs_privilege: bool,
}
