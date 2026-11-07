//! Linux: the kernel leaves the DFU interface unclaimed (snd-usb-audio takes 0-2 only), so no
//! driver work is needed. The only host-side requirement is permission to open the device node
//! under /dev/bus/usb, which a udev rule (or running as root) provides.

use super::{HostReport, ReportItem};
use crate::dfu::{PID_DFU, PID_RUNTIME, VID};
use anyhow::{Context, Result};
use nusb::{Device, DeviceInfo, MaybeFuture};
use std::fs;
use std::path::Path;

pub const OS_NAME: &str = "Linux";
pub const UDEV_RULE_PATH: &str = "/etc/udev/rules.d/70-mtrack2x2-dfu.rules";

pub fn udev_rule_text() -> String {
    format!(
        "# M-Audio M-Track 2X2: allow users to run the firmware fix tool (DFU) without root\n\
         SUBSYSTEM==\"usb\", ATTR{{idVendor}}==\"{VID:04x}\", ATTR{{idProduct}}==\"{PID_RUNTIME:04x}\", MODE=\"0666\", TAG+=\"uaccess\"\n\
         SUBSYSTEM==\"usb\", ATTR{{idVendor}}==\"{VID:04x}\", ATTR{{idProduct}}==\"{PID_DFU:04x}\", MODE=\"0666\", TAG+=\"uaccess\"\n"
    )
}

fn is_root() -> bool {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc_geteuid() == 0 }
}

extern "C" {
    #[link_name = "geteuid"]
    fn libc_geteuid() -> u32;
}

fn can_open(info: &DeviceInfo) -> bool {
    let p = format!("/dev/bus/usb/{:03}/{:03}", info.busnum(), info.device_address());
    fs::OpenOptions::new().read(true).write(true).open(p).is_ok()
}

pub fn host_report(runtime: Option<&DeviceInfo>, dfu: Option<&DeviceInfo>) -> HostReport {
    let mut r = HostReport::default();
    let rule = Path::new(UDEV_RULE_PATH).exists();
    if is_root() {
        r.items
            .push(ReportItem::good("Running as root: USB access is available."));
    } else if rule {
        r.items
            .push(ReportItem::good(format!("udev rule present ({UDEV_RULE_PATH}).")));
    } else {
        r.items.push(ReportItem::bad(format!(
            "No udev rule at {UDEV_RULE_PATH} and not running as root. Run the tool with sudo/pkexec once \
             (it can install the rule), or install the rule by hand."
        )));
        r.needs_setup = true;
        r.needs_privilege = true;
    }
    for (name, d) in [("audio-mode", runtime), ("DFU-mode", dfu)] {
        if let Some(d) = d {
            if can_open(d) {
                r.items
                    .push(ReportItem::good(format!("{name} device node is writable.")));
                r.needs_privilege = false;
            } else {
                r.items.push(ReportItem::bad(format!(
                    "{name} device node is not writable by this user."
                )));
                r.needs_privilege = true;
            }
        }
    }
    r
}

/// sysfs exposes the descriptors to everyone, so the bug check needs no privileges.
pub fn read_config_descriptor(info: &DeviceInfo) -> Result<Vec<u8>> {
    let p = info.sysfs_path().join("descriptors");
    let all = fs::read(&p).with_context(|| format!("read {}", p.display()))?;
    if all.len() < 18 + 9 {
        anyhow::bail!("descriptor file too short");
    }
    let cfg = &all[18..];
    let total = cfg[2] as usize | (cfg[3] as usize) << 8;
    Ok(cfg[..total.min(cfg.len())].to_vec())
}

pub fn bus_reset(_info: &DeviceInfo, dev: &Device, log: &dyn Fn(String)) -> Result<()> {
    log("Resetting the USB device (USBDEVFS_RESET) ...".into());
    match dev.reset().wait() {
        Ok(()) => Ok(()),
        // The device re-enumerates with a new address; errors here are usually benign.
        Err(e) => {
            log(format!("  reset returned {e} (normal when the device re-enumerates)"));
            Ok(())
        }
    }
}

/// Install the udev rule when running as root. Returns true if something was changed.
pub fn prepare_runtime_dfu_access(log: &dyn Fn(String)) -> Result<bool> {
    if Path::new(UDEV_RULE_PATH).exists() {
        return Ok(false);
    }
    if !is_root() {
        anyhow::bail!(
            "USB access needs root or a udev rule. Either run this tool once with sudo/pkexec (it will \
             install {UDEV_RULE_PATH}), or create that file with:\n{}",
            udev_rule_text()
        );
    }
    fs::write(UDEV_RULE_PATH, udev_rule_text()).with_context(|| format!("write {UDEV_RULE_PATH}"))?;
    log(format!("Installed {UDEV_RULE_PATH}"));
    for args in [["control", "--reload-rules"], ["trigger", "--subsystem-match=usb"]] {
        let _ = std::process::Command::new("udevadm").args(args).status();
    }
    log("Reloaded udev rules.".into());
    Ok(true)
}

pub fn prepare_dfu_mode_access(log: &dyn Fn(String)) -> Result<bool> {
    prepare_runtime_dfu_access(log)
}

pub fn remove_vendor_driver(_log: &dyn Fn(String)) -> Result<bool> {
    Ok(false)
}

pub fn open_error_hint(e: &anyhow::Error) -> Option<String> {
    let s = e.to_string().to_lowercase();
    if s.contains("permission") || s.contains("access") {
        Some(format!(
            "The USB device node is not writable. Run the tool with sudo/pkexec once (it installs \
             {UDEV_RULE_PATH}), then replug the interface."
        ))
    } else {
        None
    }
}
