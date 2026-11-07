//! DFU 1.1 + XMOS vendor requests over nusb, for the M-Track 2X2's lib_xua DFU implementation.
//!
//! Firmware facts that shape this module (all observed on the device, see firmware/README.md):
//!  * Runtime (audio) mode is 0763:400B with the DFU interface at number 3; DFU mode re-enumerates
//!    as 0763:C00B with one DFU interface at number 0.
//!  * THE ONE-REQUEST RULE: in runtime mode only ONE DFU-interface request per power-up is safe and
//!    it must be DFU_DETACH. Any second request (GETSTATUS, GETSTATE, the XMOS 0xf2 reset) hangs the
//!    control endpoint until the device is power-cycled. In DFU mode the rule does not apply.
//!  * DFU_GETSTATE hangs the device in either mode; never send it.
//!  * DFU_DETACH does not reset the device ("will not detach"); the host must bus-reset it.
//!  * DFU_UPLOAD only returns the upgrade image (0 bytes when only the factory image exists).
//!  * DFU_DNLOAD is ignored while an upgrade image is present: REVERTFACTORY first.
//!  * XMOS_DFU_RESETFROMDFU (0xf3) is safe in DFU mode and reboots into audio mode.

use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use nusb::transfer::{ControlIn, ControlOut, ControlType, Recipient};
use nusb::{Device, DeviceInfo, Interface, MaybeFuture};

pub const VID: u16 = 0x0763;
pub const PID_RUNTIME: u16 = 0x400B;
pub const PID_DFU: u16 = 0xC00B;

pub const DFU_DETACH: u8 = 0;
pub const DFU_DNLOAD: u8 = 1;
pub const DFU_UPLOAD: u8 = 2;
pub const DFU_GETSTATUS: u8 = 3;
#[allow(dead_code)]
pub const DFU_CLRSTATUS: u8 = 4;
// DFU_GETSTATE (5) deliberately has no constant: it hangs this device.
pub const DFU_ABORT: u8 = 6;
pub const XMOS_DFU_REVERTFACTORY: u8 = 0xF1;
pub const XMOS_DFU_RESETFROMDFU: u8 = 0xF3;

pub const BLOCK: usize = 64;
pub const DETACH_WINDOW_MS: u16 = 30000;
const TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Runtime,
    Dfu,
}

pub fn status_name(s: u8) -> &'static str {
    [
        "OK",
        "errTARGET",
        "errFILE",
        "errWRITE",
        "errERASE",
        "errCHECK_ERASED",
        "errPROG",
        "errVERIFY",
        "errADDRESS",
        "errNOTDONE",
        "errFIRMWARE",
        "errVENDOR",
        "errUSBR",
        "errPOR",
        "errUNKNOWN",
        "errSTALLEDPKT",
    ]
    .get(s as usize)
    .copied()
    .unwrap_or("err?")
}

pub fn state_name(s: u8) -> &'static str {
    [
        "appIDLE",
        "appDETACH",
        "dfuIDLE",
        "dfuDNLOAD-SYNC",
        "dfuDNBUSY",
        "dfuDNLOAD-IDLE",
        "dfuMANIFEST-SYNC",
        "dfuMANIFEST",
        "dfuMANIFEST-WAIT-RESET",
        "dfuUPLOAD-IDLE",
        "dfuERROR",
    ]
    .get(s as usize)
    .copied()
    .unwrap_or("state?")
}

pub fn mode_of(info: &DeviceInfo) -> Option<Mode> {
    if info.vendor_id() != VID {
        return None;
    }
    match info.product_id() {
        PID_RUNTIME => Some(Mode::Runtime),
        PID_DFU => Some(Mode::Dfu),
        _ => None,
    }
}

/// Every M-Track 2X2 on the bus, in runtime or DFU mode.
pub fn find_devices() -> Vec<(DeviceInfo, Mode)> {
    match nusb::list_devices().wait() {
        Ok(it) => it.filter_map(|d| mode_of(&d).map(|m| (d, m))).collect(),
        Err(_) => Vec::new(),
    }
}

pub fn find_mode(mode: Mode) -> Option<DeviceInfo> {
    find_devices().into_iter().find(|(_, m)| *m == mode).map(|(d, _)| d)
}

/// Short human description of a device.
pub fn describe(info: &DeviceInfo, mode: Mode) -> String {
    let v = info.device_version();
    format!(
        "{:04x}:{:04x} firmware {}.{:02x} on bus {} addr {} ({})",
        info.vendor_id(),
        info.product_id(),
        v >> 8,
        v & 0xff,
        info.bus_id(),
        info.device_address(),
        match mode {
            Mode::Runtime => "audio mode",
            Mode::Dfu => "DFU mode",
        }
    )
}

/// Find the DFU interface (class 0xFE, subclass 1) in the active configuration.
pub fn dfu_interface_number(dev: &Device) -> Option<(u8, u8)> {
    for cfg in dev.configurations() {
        for intf in cfg.interfaces() {
            let alt = intf.first_alt_setting();
            if alt.class() == 0xFE && alt.subclass() == 0x01 {
                return Some((intf.interface_number(), alt.protocol()));
            }
        }
    }
    None
}

#[allow(dead_code)]
pub struct DfuDevice {
    pub info: DeviceInfo,
    pub device: Device,
    pub interface: Interface,
    pub intf_num: u8,
    pub mode: Mode,
}

impl DfuDevice {
    /// Open the device and claim its DFU interface. No request is sent to the DFU interface.
    pub fn open(info: DeviceInfo) -> Result<DfuDevice> {
        let device = info.open().wait().context("open USB device")?;
        let (intf_num, proto) = dfu_interface_number(&device).ok_or_else(|| anyhow!("device has no DFU interface"))?;
        let mode = match proto {
            2 => Mode::Dfu,
            _ => Mode::Runtime,
        };
        let interface = device
            .detach_and_claim_interface(intf_num)
            .wait()
            .with_context(|| format!("claim DFU interface {intf_num}"))?;
        Ok(DfuDevice {
            info,
            device,
            interface,
            intf_num,
            mode,
        })
    }

    fn out(&self, request: u8, value: u16, data: &[u8]) -> Result<()> {
        self.interface
            .control_out(
                ControlOut {
                    control_type: ControlType::Class,
                    recipient: Recipient::Interface,
                    request,
                    value,
                    index: self.intf_num as u16,
                    data,
                },
                TIMEOUT,
            )
            .wait()
            .map_err(|e| anyhow!("DFU request {request:#04x}: {e}"))
    }

    fn inp(&self, request: u8, value: u16, length: u16) -> Result<Vec<u8>> {
        self.interface
            .control_in(
                ControlIn {
                    control_type: ControlType::Class,
                    recipient: Recipient::Interface,
                    request,
                    value,
                    index: self.intf_num as u16,
                    length,
                },
                TIMEOUT,
            )
            .wait()
            .map_err(|e| anyhow!("DFU request {request:#04x}: {e}"))
    }

    /// A standard GET_DESCRIPTOR(device) through the same handle. On Windows a WinUSB handle can
    /// get into a state where every transfer fails instantly (only a reboot clears it); this
    /// probe detects that without spending the one allowed runtime DFU request.
    pub fn probe_handle(&self) -> Result<()> {
        let d = self
            .interface
            .control_in(
                ControlIn {
                    control_type: ControlType::Standard,
                    recipient: Recipient::Device,
                    request: 0x06, // GET_DESCRIPTOR
                    value: 0x0100, // DEVICE descriptor, index 0
                    index: 0,
                    length: 18,
                },
                TIMEOUT,
            )
            .wait()
            .map_err(|e| anyhow!("GET_DESCRIPTOR probe failed: {e}"))?;
        if d.len() < 18 {
            bail!("GET_DESCRIPTOR probe returned {} bytes", d.len());
        }
        Ok(())
    }

    /// The single runtime-mode request. The caller must bus-reset the device afterwards.
    pub fn detach(&self) -> Result<()> {
        self.out(DFU_DETACH, DETACH_WINDOW_MS, &[])
    }

    /// (bStatus, bwPollTimeout, bState). Only safe in DFU mode on this device.
    pub fn get_status(&self) -> Result<(u8, u32, u8)> {
        let b = self.inp(DFU_GETSTATUS, 0, 6)?;
        if b.len() < 6 {
            bail!("short GETSTATUS reply ({} bytes)", b.len());
        }
        Ok((b[0], b[1] as u32 | (b[2] as u32) << 8 | (b[3] as u32) << 16, b[4]))
    }

    pub fn upload_block(&self, block: u16) -> Result<Vec<u8>> {
        self.inp(DFU_UPLOAD, block, BLOCK as u16)
    }

    pub fn abort(&self) -> Result<()> {
        self.out(DFU_ABORT, 0, &[])
    }

    /// Read the whole upgrade image (empty when only the factory image is present).
    pub fn read_image(&self, mut progress: impl FnMut(usize)) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        let mut block: u16 = 0;
        loop {
            let data = self.upload_block(block)?;
            if data.is_empty() {
                break;
            }
            out.extend_from_slice(&data);
            block = block
                .checked_add(1)
                .ok_or_else(|| anyhow!("upload did not terminate"))?;
            if block.is_multiple_of(64) {
                progress(out.len());
            }
        }
        Ok(out)
    }

    /// Write an upgrade image. `image` must already be in chip (bit-reversed) order.
    pub fn download(&self, image: &[u8], mut progress: impl FnMut(usize, usize)) -> Result<()> {
        let mut padded = image.to_vec();
        padded.resize(image.len().div_ceil(BLOCK) * BLOCK, 0);
        let blocks = padded.len() / BLOCK;
        for n in 0..blocks {
            self.out(DFU_DNLOAD, n as u16, &padded[n * BLOCK..(n + 1) * BLOCK])
                .with_context(|| format!("block {n}"))?;
            let (st, _, state) = self.get_status().with_context(|| format!("status after block {n}"))?;
            if st != 0 {
                bail!(
                    "device reported {} ({}) on block {n}",
                    status_name(st),
                    state_name(state)
                );
            }
            if n % 16 == 0 || n + 1 == blocks {
                progress(n + 1, blocks);
            }
        }
        self.out(DFU_DNLOAD, 0, &[]).context("terminating zero-length block")?;
        let (st, _, state) = self.get_status().context("final status")?;
        if st != 0 {
            bail!(
                "device reported {} ({}) after the download",
                status_name(st),
                state_name(state)
            );
        }
        Ok(())
    }

    pub fn revert_factory(&self) -> Result<()> {
        self.out(XMOS_DFU_REVERTFACTORY, 0, &[])
    }

    /// Reboot into audio mode. The device may reset before the status stage, so an error here
    /// is not necessarily a failure; the caller waits for the runtime device to reappear.
    pub fn reset_from_dfu(&self) -> Result<()> {
        self.out(XMOS_DFU_RESETFROMDFU, 0, &[])
    }
}
