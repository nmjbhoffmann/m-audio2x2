//! Fallback for platforms the tool was not written for (macOS builds compile, but the
//! bus-reset path is untested).

use super::{HostReport, ReportItem};
use anyhow::Result;
use nusb::{Device, DeviceInfo, MaybeFuture};

pub const OS_NAME: &str = "this platform";

pub fn host_report(_runtime: Option<&DeviceInfo>, _dfu: Option<&DeviceInfo>) -> HostReport {
    HostReport {
        items: vec![ReportItem::info("No host-side checks on this platform.")],
        ..Default::default()
    }
}

pub fn read_config_descriptor(info: &DeviceInfo) -> Result<Vec<u8>> {
    let dev = info.open().wait()?;
    let cfg = dev.configurations().next().map(|c| c.as_bytes().to_vec());
    cfg.ok_or_else(|| anyhow::anyhow!("no configuration descriptor"))
}

pub fn bus_reset(_info: &DeviceInfo, dev: &Device, _log: &dyn Fn(String)) -> Result<()> {
    dev.reset().wait()?;
    Ok(())
}

pub fn prepare_runtime_dfu_access(_log: &dyn Fn(String)) -> Result<bool> {
    Ok(false)
}

pub fn prepare_dfu_mode_access(_log: &dyn Fn(String)) -> Result<bool> {
    Ok(false)
}

pub fn remove_vendor_driver(_log: &dyn Fn(String)) -> Result<bool> {
    Ok(false)
}

pub fn open_error_hint(_e: &anyhow::Error) -> Option<String> {
    None
}
