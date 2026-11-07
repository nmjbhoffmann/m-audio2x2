//! The flows (check / fix host / flash / revert / backup), independent of the UI. Progress and
//! user instructions go out as `Event`s; the GUI shows them, the CLI prints them.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use nusb::DeviceInfo;

use crate::descriptor::{capture_terminal_state, DescriptorState};
use crate::dfu::{self, describe, find_devices, find_mode, DfuDevice, Mode};
use crate::image::Firmware;
use crate::platform::{self, HostReport};

#[derive(Clone, Debug)]
pub enum Event {
    Log(String),
    /// Current phase, one line.
    Step(String),
    /// Something the user has to do now (replug, reboot). None clears it.
    Attention(Option<String>),
    /// 0..1 while transferring, None otherwise.
    Progress(Option<f32>),
    Report(Box<Report>),
    Done {
        ok: bool,
        summary: String,
    },
}

pub type Sink = Arc<dyn Fn(Event) + Send + Sync>;

#[derive(Clone, Debug, Default)]
pub struct Report {
    pub runtime: Option<String>,
    pub dfu_mode: Option<String>,
    pub firmware_version: Option<u16>,
    pub descriptor: Option<DescriptorState>,
    pub descriptor_error: Option<String>,
    pub host: HostReport,
}

impl Report {
    pub fn device_present(&self) -> bool {
        self.runtime.is_some() || self.dfu_mode.is_some()
    }
}

const REPLUG_TIMEOUT: Duration = Duration::from_secs(180);

pub struct Engine {
    sink: Sink,
    cancel: Arc<AtomicBool>,
}

impl Engine {
    pub fn new(sink: Sink, cancel: Arc<AtomicBool>) -> Self {
        Self { sink, cancel }
    }

    fn emit(&self, e: Event) {
        (self.sink)(e);
    }
    pub fn log(&self, s: impl Into<String>) {
        self.emit(Event::Log(s.into()));
    }
    fn step(&self, s: impl Into<String>) {
        let s = s.into();
        self.log(format!("== {s}"));
        self.emit(Event::Step(s));
    }
    fn attention(&self, s: Option<String>) {
        self.emit(Event::Attention(s));
    }
    fn logger(&self) -> impl Fn(String) + '_ {
        move |s| self.log(s)
    }
    fn check_cancel(&self) -> Result<()> {
        if self.cancel.load(Ordering::Relaxed) {
            bail!("cancelled");
        }
        Ok(())
    }
    fn sleep(&self, d: Duration) -> Result<()> {
        let end = Instant::now() + d;
        while Instant::now() < end {
            self.check_cancel()?;
            std::thread::sleep(Duration::from_millis(100).min(end - Instant::now()));
        }
        Ok(())
    }

    // ------------------------------------------------------------ check

    pub fn check(&self) -> Report {
        let mut r = Report::default();
        let devs = find_devices();
        let runtime = devs.iter().find(|(_, m)| *m == Mode::Runtime).map(|(d, _)| d.clone());
        let dfu_dev = devs.iter().find(|(_, m)| *m == Mode::Dfu).map(|(d, _)| d.clone());
        if let Some(d) = &runtime {
            r.runtime = Some(describe(d, Mode::Runtime));
            r.firmware_version = Some(d.device_version());
            match platform::read_config_descriptor(d) {
                Ok(cfg) => r.descriptor = Some(capture_terminal_state(&cfg)),
                Err(e) => r.descriptor_error = Some(format!("{e:#}")),
            }
        }
        if let Some(d) = &dfu_dev {
            r.dfu_mode = Some(describe(d, Mode::Dfu));
            r.firmware_version = Some(d.device_version());
        }
        r.host = platform::host_report(runtime.as_ref(), dfu_dev.as_ref());
        self.emit(Event::Report(Box::new(r.clone())));
        r
    }

    // ------------------------------------------------------------ host fixes

    /// Remove the vendor driver (Windows) or set up USB access (bind WinUSB / udev rule).
    pub fn fix_host(&self) -> Result<String> {
        self.step("Checking the host");
        let host = platform::host_report(find_mode(Mode::Runtime).as_ref(), find_mode(Mode::Dfu).as_ref());
        if host.needs_privilege {
            bail!("this needs administrator/root rights; restart the tool elevated");
        }
        if host.vendor_driver {
            self.step("Removing the M-Audio driver package");
            if platform::remove_vendor_driver(&self.logger())? {
                self.attention(Some(
                    "REBOOT the computer now. After the reboot, unplug and replug the M-Track 2X2, start this \
                     tool again and run the check."
                        .into(),
                ));
                return Ok("M-Audio driver removed. Reboot, replug the interface, then run the check again.".into());
            }
            bail!("the vendor driver is bound but no package was found to remove; uninstall it from Settings > Apps, then reboot");
        }
        let mut changed = false;
        if find_mode(Mode::Runtime).is_some() {
            self.step("Setting up access to the DFU interface");
            changed |= platform::prepare_runtime_dfu_access(&self.logger())?;
        }
        if find_mode(Mode::Dfu).is_some() {
            self.step("Setting up access to the DFU-mode device");
            changed |= platform::prepare_dfu_mode_access(&self.logger())?;
        }
        if !changed && find_mode(Mode::Runtime).is_none() && find_mode(Mode::Dfu).is_none() {
            bail!("no M-Track 2X2 is connected");
        }
        Ok(if changed {
            "USB access set up.".into()
        } else {
            "Nothing to do: the host is already set up.".into()
        })
    }

    // ------------------------------------------------------------ device waits

    fn wait_for_mode(&self, mode: Mode, timeout: Duration) -> Result<DeviceInfo> {
        let end = Instant::now() + timeout;
        loop {
            if let Some(d) = find_mode(mode) {
                return Ok(d);
            }
            if Instant::now() >= end {
                bail!(
                    "timed out waiting for the device in {} mode",
                    if mode == Mode::Dfu { "DFU" } else { "audio" }
                );
            }
            self.sleep(Duration::from_millis(500))?;
        }
    }

    /// Ask for a power cycle and wait until the device has gone away and come back.
    fn prompt_replug(&self, reason: &str) -> Result<DeviceInfo> {
        self.attention(Some(format!(
            "ACTION NEEDED: {reason}\nUNPLUG the M-Track 2X2, wait 2 seconds, and PLUG IT BACK IN."
        )));
        self.log(format!(
            "Waiting up to {} s for a replug ({reason}) ...",
            REPLUG_TIMEOUT.as_secs()
        ));
        let end = Instant::now() + REPLUG_TIMEOUT;
        let mut seen_absent = find_devices().is_empty();
        let result = loop {
            let devs = find_devices();
            if devs.is_empty() {
                seen_absent = true;
            } else if seen_absent {
                self.sleep(Duration::from_secs(2))?; // let the OS finish enumerating
                let (d, m) = find_devices()
                    .into_iter()
                    .next()
                    .ok_or_else(|| anyhow!("device vanished again"))?;
                self.log(format!("Device is back: {}", describe(&d, m)));
                break Ok(d);
            }
            if Instant::now() >= end {
                break Err(anyhow!("timed out waiting for the device to be replugged"));
            }
            self.sleep(Duration::from_secs(1))?;
        };
        self.attention(None);
        result
    }

    /// Open + claim the DFU interface; if the OS side is not set up yet (WinUSB / udev), set it
    /// up and retry once.
    fn open_with_setup(&self, info: DeviceInfo, mode: Mode) -> Result<DfuDevice> {
        match DfuDevice::open(info.clone()) {
            Ok(d) => Ok(d),
            Err(e) => {
                self.log(format!("Cannot use the DFU interface yet: {e:#}"));
                if let Some(h) = platform::open_error_hint(&e) {
                    self.log(h);
                }
                let changed = match mode {
                    Mode::Runtime => platform::prepare_runtime_dfu_access(&self.logger())?,
                    Mode::Dfu => platform::prepare_dfu_mode_access(&self.logger())?,
                };
                if !changed {
                    return Err(e.context("DFU interface not accessible"));
                }
                let info = self.wait_for_mode(mode, Duration::from_secs(15))?;
                DfuDevice::open(info).context("DFU interface still not accessible after setup")
            }
        }
    }

    /// Get the device into DFU mode and return it with the DFU interface claimed.
    fn enter_dfu(&self, retry: bool) -> Result<DfuDevice> {
        if let Some(d) = find_mode(Mode::Dfu) {
            self.log(format!("Device is already in DFU mode: {}", describe(&d, Mode::Dfu)));
            return self.open_with_setup(d, Mode::Dfu);
        }
        let info = match find_mode(Mode::Runtime) {
            Some(d) => d,
            None => self.prompt_replug("the M-Track 2X2 is not connected (or it hung and dropped off the bus)")?,
        };
        self.log(format!("Found {}", describe(&info, Mode::Runtime)));
        let d = self.open_with_setup(info, Mode::Runtime)?;

        self.step("Switching to DFU mode");
        if let Err(e) = d.probe_handle() {
            self.log(format!("{e:#}"));
            if cfg!(windows) {
                self.attention(Some(
                    "The USB handle is in a broken state that only a REBOOT clears (every transfer fails \
                     instantly). Reboot Windows, replug the M-Track 2X2 and try again."
                        .into(),
                ));
                bail!("USB handle broken; reboot Windows and retry (no DFU request was sent)");
            }
            if !retry {
                bail!("USB handle broken even after a replug");
            }
            drop(d);
            self.prompt_replug("the USB handle is not responding")?;
            return self.enter_dfu(false);
        }
        self.log(format!(
            "Sending DFU_DETACH (window {} ms). This must be the first DFU request since power-up.",
            dfu::DETACH_WINDOW_MS
        ));
        d.detach().context("DFU_DETACH")?;
        let t_detach = Instant::now();
        let info = d.info.clone();
        let device = d.device.clone();
        drop(d); // release the interface before the reset
        platform::bus_reset(&info, &device, &self.logger())?;
        drop(device);

        let window = Duration::from_millis(dfu::DETACH_WINDOW_MS as u64).saturating_sub(t_detach.elapsed());
        let dfu_info = match self.wait_for_mode(Mode::Dfu, window.max(Duration::from_secs(10))) {
            Ok(i) => i,
            Err(e) => {
                self.log(format!("{e:#}"));
                // Windows: the node may be there without a driver, which nusb does not list as usable.
                if platform::prepare_dfu_mode_access(&self.logger()).unwrap_or(false) {
                    if let Ok(i) = self.wait_for_mode(Mode::Dfu, Duration::from_secs(10)) {
                        self.log(format!("Device is in DFU mode: {}", describe(&i, Mode::Dfu)));
                        return self.open_with_setup(i, Mode::Dfu);
                    }
                }
                if !retry {
                    return Err(e.context("the device did not enter DFU mode"));
                }
                self.prompt_replug("the device did not come back in DFU mode (firmware hung?); trying once more")?;
                return self.enter_dfu(false);
            }
        };
        self.log(format!("Device is in DFU mode: {}", describe(&dfu_info, Mode::Dfu)));
        let d = self.open_with_setup(dfu_info, Mode::Dfu)?;
        let (st, _, state) = d.get_status().context("GETSTATUS in DFU mode")?;
        self.log(format!(
            "DFU status {} / {}",
            dfu::status_name(st),
            dfu::state_name(state)
        ));
        Ok(d)
    }

    /// Reboot into audio mode and wait for the runtime device.
    fn leave_dfu(&self, d: DfuDevice) -> Result<DeviceInfo> {
        self.step("Returning to audio mode");
        if let Err(e) = d.reset_from_dfu() {
            self.log(format!(
                "  (RESETFROMDFU returned {e}; the device may have reset before the status stage)"
            ));
        }
        let info = d.info.clone();
        let device = d.device.clone();
        drop(d);
        if let Ok(i) = self.wait_for_mode(Mode::Runtime, Duration::from_secs(15)) {
            self.log(format!("Device is back in audio mode: {}", describe(&i, Mode::Runtime)));
            return Ok(i);
        }
        self.log("Still in DFU mode; trying a bus reset ...");
        let _ = platform::bus_reset(&info, &device, &self.logger());
        drop(device);
        if let Ok(i) = self.wait_for_mode(Mode::Runtime, Duration::from_secs(15)) {
            self.log(format!("Device is back in audio mode: {}", describe(&i, Mode::Runtime)));
            return Ok(i);
        }
        self.prompt_replug("the device did not return to audio mode by itself (a power cycle always does)")
    }

    fn read_existing_image(&self, d: &DfuDevice) -> Result<Vec<u8>> {
        self.step("Reading the current upgrade image");
        let img = d.read_image(|n| self.emit(Event::Log(format!("  {n} bytes ..."))))?;
        let _ = d.abort();
        if img.is_empty() {
            self.log("No upgrade image present: the device runs its factory firmware.");
        } else {
            self.log(format!("Upgrade image present: {} bytes.", img.len()));
        }
        Ok(img)
    }

    fn verify_descriptor(&self, info: &DeviceInfo) -> Result<DescriptorState> {
        self.sleep(Duration::from_secs(2))?;
        let cfg = platform::read_config_descriptor(info).context("read configuration descriptor")?;
        let st = capture_terminal_state(&cfg);
        self.log(format!("Descriptor check: {}", st.describe()));
        Ok(st)
    }

    // ------------------------------------------------------------ flash

    pub fn flash(&self, fw: &Firmware, force: bool) -> Result<String> {
        self.attention(None);
        self.step("Checking the host");
        let host = platform::host_report(find_mode(Mode::Runtime).as_ref(), find_mode(Mode::Dfu).as_ref());
        for i in &host.items {
            if i.ok == Some(false) {
                self.log(format!("  ! {}", i.text));
            }
        }
        if host.vendor_driver {
            bail!("the M-Audio driver is installed; remove it first (\"Remove M-Audio driver\"), reboot, replug");
        }
        if host.needs_privilege {
            bail!(
                "missing rights: {}",
                if cfg!(windows) {
                    "run as administrator"
                } else {
                    "run as root or install the udev rule"
                }
            );
        }
        self.log(format!(
            "Firmware: {} (version {}, {} bytes, sha256 {})",
            fw.name,
            fw.info.version,
            fw.info.length,
            &fw.sha256_logical[..16]
        ));

        let mut d = self.enter_dfu(true)?;
        let existing = self.read_existing_image(&d)?;
        if !existing.is_empty() {
            if existing.len() >= fw.chip_order.len() && existing[..fw.chip_order.len()] == fw.chip_order[..] {
                if !force {
                    self.log("The device already holds exactly this image.");
                    let info = self.leave_dfu(d)?;
                    let st = self.verify_descriptor(&info)?;
                    return Ok(format!(
                        "Nothing to do: this firmware is already installed. {}",
                        st.describe()
                    ));
                }
                self.log("The device already holds this image; re-flashing because 'force' was requested.");
            }
            self.step("Removing the old upgrade image (revert to factory)");
            d.revert_factory().context("XMOS_DFU_REVERTFACTORY")?;
            self.sleep(Duration::from_secs(3))?;
            self.leave_dfu(d)?;
            self.sleep(Duration::from_secs(2))?;
            d = self.enter_dfu(true)?;
            let again = self.read_existing_image(&d)?;
            if !again.is_empty() {
                let _ = self.leave_dfu(d);
                bail!("the upgrade image is still present after revert-factory; aborting");
            }
        }

        self.step("Writing the firmware");
        self.emit(Event::Progress(Some(0.0)));
        d.download(&fw.chip_order, |done, total| {
            self.emit(Event::Progress(Some(done as f32 / total as f32)));
            if done % 64 == 0 || done == total {
                self.emit(Event::Log(format!("  block {done}/{total}")));
            }
        })
        .context("DFU download")?;
        self.emit(Event::Progress(None));
        self.log("Download complete.");

        let info = self.leave_dfu(d)?;
        self.step("Verifying");
        let st = self.verify_descriptor(&info)?;
        match st {
            DescriptorState::Fixed => Ok(format!(
                "SUCCESS: the device now reports the fixed descriptor.{}",
                if cfg!(windows) {
                    " Windows should list a 'Microphone (M-Track 2X2)' recording device; replug once if it does not."
                } else {
                    ""
                }
            )),
            other => bail!(
                "the download finished but the device still reports: {}. The loader may have rejected the image (CRC/version) and booted the factory firmware.",
                other.describe()
            ),
        }
    }

    // ------------------------------------------------------------ revert

    pub fn revert(&self) -> Result<String> {
        self.attention(None);
        let d = self.enter_dfu(true)?;
        let existing = self.read_existing_image(&d)?;
        if existing.is_empty() {
            let _ = self.leave_dfu(d)?;
            return Ok("Nothing to do: the device already runs its factory firmware.".into());
        }
        self.step("Reverting to the factory firmware");
        d.revert_factory().context("XMOS_DFU_REVERTFACTORY")?;
        self.sleep(Duration::from_secs(3))?;
        let info = self.leave_dfu(d)?;
        self.step("Verifying");
        let st = self.verify_descriptor(&info)?;
        Ok(format!(
            "Reverted. The device runs its factory firmware again ({}).",
            st.describe()
        ))
    }

    // ------------------------------------------------------------ backup

    /// Save the current upgrade image (chip order) to a file. Empty file = factory only.
    pub fn backup(&self, path: &std::path::Path) -> Result<String> {
        self.attention(None);
        let d = self.enter_dfu(true)?;
        let existing = self.read_existing_image(&d)?;
        std::fs::write(path, &existing).with_context(|| format!("write {}", path.display()))?;
        let _ = self.leave_dfu(d)?;
        Ok(format!("Wrote {} bytes to {}", existing.len(), path.display()))
    }
}
