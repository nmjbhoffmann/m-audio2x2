//! Windows: PnP state of the M-Track 2X2, the vendor driver package, binding the in-box WinUSB
//! driver to the DFU interface (what Device Manager's "Update driver > Let me pick > WinUsb
//! Device" does), and a real bus reset through the upstream hub (IOCTL_USB_HUB_CYCLE_PORT).
//!
//! Everything here uses in-box Windows components; nothing is downloaded.

use super::{HostReport, ReportItem};
use anyhow::{anyhow, bail, Context, Result};
use nusb::{Device, DeviceInfo};
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::ptr::{null, null_mut};
use std::time::Duration;
use windows_sys::Win32::Devices::DeviceAndDriverInstallation::*;
use windows_sys::Win32::Devices::Properties::*;
use windows_sys::Win32::Devices::Usb::{IOCTL_USB_HUB_CYCLE_PORT, USB_CYCLE_PORT_PARAMS};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
use windows_sys::Win32::Storage::FileSystem::*;
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows_sys::Win32::System::IO::DeviceIoControl;

pub const OS_NAME: &str = "Windows";

const RUNTIME_PARENT: &str = "USB\\VID_0763&PID_400B\\";
const RUNTIME_AUDIO: &str = "USB\\VID_0763&PID_400B&MI_00\\";
const RUNTIME_DFU: &str = "USB\\VID_0763&PID_400B&MI_03\\";
const DFU_MODE: &str = "USB\\VID_0763&PID_C00B\\";
const VENDOR_SERVICE: &str = "MTRACK2X2";
const VENDOR_INF_MARKER: &str = "maudiomtrack2x2";
/// Arbitrary, project-specific device interface GUID registered for the WinUSB nodes so that
/// user-mode code (nusb/libusb) can open them. Stock winusb.inf registers none.
const INTERFACE_GUID: &str = "{6C5F8B6A-3B1B-4B7E-9C19-2A0C8D3E4F10}";
/// USBDevice setup class from winusb.inf.
const USBDEVICE_CLASS_GUID: &str = "{88BAE032-5A81-49F0-BC3D-A4FF138216D6}";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn utf16_units(buf: &[u8]) -> Vec<u16> {
    buf.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect()
}

fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    OsString::from_wide(&buf[..end]).to_string_lossy().into_owned()
}

fn last_error() -> u32 {
    // SAFETY: trivial FFI call.
    unsafe { GetLastError() }
}

// ---------------------------------------------------------------- privileges

pub fn is_elevated() -> bool {
    // SAFETY: standard token query with properly sized out-params.
    unsafe {
        let mut token: HANDLE = null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut te = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            &mut te as *mut _ as *mut _,
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        );
        CloseHandle(token);
        ok != 0 && te.TokenIsElevated != 0
    }
}

// ---------------------------------------------------------------- PnP nodes

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct PnpNode {
    pub instance_id: String,
    pub service: String,
    pub problem: u32,
    pub name: String,
}

fn devnode_prop_raw(devinst: u32, key: &DEVPROPKEY) -> Option<(u32, Vec<u8>)> {
    let mut ty: DEVPROPTYPE = 0;
    let mut buf = vec![0u8; 4096];
    let mut size = buf.len() as u32;
    // SAFETY: buffer and size are consistent.
    let cr = unsafe { CM_Get_DevNode_PropertyW(devinst, key, &mut ty, buf.as_mut_ptr(), &mut size, 0) };
    if cr != CR_SUCCESS {
        return None;
    }
    buf.truncate(size as usize);
    Some((ty, buf))
}

fn devnode_prop_string(devinst: u32, key: &DEVPROPKEY) -> Option<String> {
    let (ty, buf) = devnode_prop_raw(devinst, key)?;
    if ty != DEVPROP_TYPE_STRING {
        return None;
    }
    let w = utf16_units(&buf);
    Some(from_wide(&w))
}

fn devnode_prop_u32(devinst: u32, key: &DEVPROPKEY) -> Option<u32> {
    let (ty, buf) = devnode_prop_raw(devinst, key)?;
    if ty != DEVPROP_TYPE_UINT32 || buf.len() < 4 {
        return None;
    }
    Some(u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]))
}

/// Present PnP device instances whose ID contains `needle` (case-insensitive).
pub fn present_nodes(needle: &str) -> Vec<PnpNode> {
    let mut out = Vec::new();
    // SAFETY: two-step size/list query with matching buffer length.
    unsafe {
        let mut len = 0u32;
        if CM_Get_Device_ID_List_SizeW(&mut len, null(), CM_GETIDLIST_FILTER_PRESENT) != CR_SUCCESS {
            return out;
        }
        let mut buf = vec![0u16; len as usize + 1];
        if CM_Get_Device_ID_ListW(null(), buf.as_mut_ptr(), len, CM_GETIDLIST_FILTER_PRESENT) != CR_SUCCESS {
            return out;
        }
        let needle = needle.to_uppercase();
        for id in buf.split(|&c| c == 0).filter(|s| !s.is_empty()) {
            let id_s = from_wide(id);
            if !id_s.to_uppercase().contains(&needle) {
                continue;
            }
            let mut devinst = 0u32;
            let w = wide(&id_s);
            if CM_Locate_DevNodeW(&mut devinst, w.as_ptr(), CM_LOCATE_DEVNODE_NORMAL) != CR_SUCCESS {
                continue;
            }
            out.push(PnpNode {
                instance_id: id_s,
                service: devnode_prop_string(devinst, &DEVPKEY_Device_Service).unwrap_or_default(),
                problem: devnode_prop_u32(devinst, &DEVPKEY_Device_ProblemCode).unwrap_or(0),
                name: devnode_prop_string(devinst, &DEVPKEY_NAME)
                    .or_else(|| devnode_prop_string(devinst, &DEVPKEY_Device_DeviceDesc))
                    .unwrap_or_default(),
            });
        }
    }
    out
}

fn find_node(prefix: &str) -> Option<PnpNode> {
    present_nodes("VID_0763")
        .into_iter()
        .find(|n| n.instance_id.to_uppercase().starts_with(&prefix.to_uppercase()))
}

// ---------------------------------------------------------------- registry

struct RegKey(HKEY);
impl Drop for RegKey {
    fn drop(&mut self) {
        // SAFETY: handle was opened by us.
        unsafe { RegCloseKey(self.0) };
    }
}

fn reg_open(root: HKEY, path: &str, sam: u32) -> Option<RegKey> {
    let mut h: HKEY = null_mut();
    let w = wide(path);
    // SAFETY: valid wide string and out-param.
    let r = unsafe { RegOpenKeyExW(root, w.as_ptr(), 0, sam, &mut h) };
    (r == ERROR_SUCCESS).then_some(RegKey(h))
}

fn reg_create(root: HKEY, path: &str, sam: u32) -> Option<RegKey> {
    let mut h: HKEY = null_mut();
    let w = wide(path);
    // SAFETY: valid wide string and out-param.
    let r = unsafe {
        RegCreateKeyExW(
            root,
            w.as_ptr(),
            0,
            null(),
            REG_OPTION_NON_VOLATILE,
            sam,
            null(),
            &mut h,
            null_mut(),
        )
    };
    (r == ERROR_SUCCESS).then_some(RegKey(h))
}

fn reg_value_raw(key: &RegKey, name: &str) -> Option<(u32, Vec<u8>)> {
    let w = wide(name);
    let mut ty = 0u32;
    let mut size = 0u32;
    // SAFETY: size query then fill, as documented.
    unsafe {
        if RegQueryValueExW(key.0, w.as_ptr(), null(), &mut ty, null_mut(), &mut size) != ERROR_SUCCESS {
            return None;
        }
        let mut buf = vec![0u8; size as usize + 2];
        if RegQueryValueExW(key.0, w.as_ptr(), null(), &mut ty, buf.as_mut_ptr(), &mut size) != ERROR_SUCCESS {
            return None;
        }
        buf.truncate(size as usize);
        Some((ty, buf))
    }
}

fn reg_string(key: &RegKey, name: &str) -> Option<String> {
    let (ty, buf) = reg_value_raw(key, name)?;
    if ty != REG_SZ && ty != REG_EXPAND_SZ {
        return None;
    }
    let w = utf16_units(&buf);
    Some(from_wide(&w))
}

fn reg_multi_string(key: &RegKey, name: &str) -> Option<Vec<String>> {
    let (ty, buf) = reg_value_raw(key, name)?;
    if ty != REG_MULTI_SZ {
        return None;
    }
    let w = utf16_units(&buf);
    Some(w.split(|&c| c == 0).filter(|s| !s.is_empty()).map(from_wide).collect())
}

fn reg_subkeys(key: &RegKey) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    loop {
        let mut name = vec![0u16; 256];
        let mut len = name.len() as u32;
        // SAFETY: buffer/len consistent.
        let r = unsafe {
            RegEnumKeyExW(
                key.0,
                i,
                name.as_mut_ptr(),
                &mut len,
                null(),
                null_mut(),
                null_mut(),
                null_mut(),
            )
        };
        if r != ERROR_SUCCESS {
            break;
        }
        out.push(from_wide(&name[..len as usize]));
        i += 1;
    }
    out
}

fn device_parameters_path(instance_id: &str) -> String {
    format!("SYSTEM\\CurrentControlSet\\Enum\\{instance_id}\\Device Parameters")
}

fn interface_guid_set(instance_id: &str) -> bool {
    reg_open(HKEY_LOCAL_MACHINE, &device_parameters_path(instance_id), KEY_READ)
        .and_then(|k| reg_multi_string(&k, "DeviceInterfaceGUIDs"))
        .map(|v| v.iter().any(|g| g.eq_ignore_ascii_case(INTERFACE_GUID)))
        .unwrap_or(false)
}

fn set_interface_guid(instance_id: &str) -> Result<()> {
    let key = reg_create(
        HKEY_LOCAL_MACHINE,
        &device_parameters_path(instance_id),
        KEY_READ | KEY_SET_VALUE,
    )
    .ok_or_else(|| anyhow!("cannot open Device Parameters key for {instance_id}"))?;
    let mut data: Vec<u16> = INTERFACE_GUID.encode_utf16().collect();
    data.push(0);
    data.push(0);
    let bytes: Vec<u8> = data.iter().flat_map(|c| c.to_le_bytes()).collect();
    let name = wide("DeviceInterfaceGUIDs");
    // SAFETY: byte buffer with matching length.
    let r = unsafe {
        RegSetValueExW(
            key.0,
            name.as_ptr(),
            0,
            REG_MULTI_SZ,
            bytes.as_ptr(),
            bytes.len() as u32,
        )
    };
    if r != ERROR_SUCCESS {
        bail!("RegSetValueEx(DeviceInterfaceGUIDs) failed: error {r}");
    }
    Ok(())
}

// ---------------------------------------------------------------- vendor package

#[derive(Clone, Debug)]
pub struct VendorPackage {
    pub name: String,
    pub version: String,
    pub product_code: String,
}

pub fn vendor_packages() -> Vec<VendorPackage> {
    let mut out = Vec::new();
    for path in [
        "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
        "SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
    ] {
        let Some(root) = reg_open(HKEY_LOCAL_MACHINE, path, KEY_READ) else {
            continue;
        };
        for sub in reg_subkeys(&root) {
            let Some(k) = reg_open(HKEY_LOCAL_MACHINE, &format!("{path}\\{sub}"), KEY_READ) else {
                continue;
            };
            let Some(name) = reg_string(&k, "DisplayName") else {
                continue;
            };
            let n = name.to_lowercase();
            if n.contains("m-track 2x2") || (n.contains("m-audio") && n.contains("2x2")) {
                out.push(VendorPackage {
                    name,
                    version: reg_string(&k, "DisplayVersion").unwrap_or_default(),
                    product_code: sub,
                });
            }
        }
    }
    out
}

fn inf_dir() -> PathBuf {
    PathBuf::from(std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into())).join("INF")
}

/// oemNN.inf files in the driver store that belong to the M-Audio driver.
pub fn leftover_vendor_infs() -> Vec<String> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(inf_dir()) else {
        return out;
    };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_lowercase();
        if !(name.starts_with("oem") && name.ends_with(".inf")) {
            continue;
        }
        let Ok(bytes) = std::fs::read(e.path()) else { continue };
        let text = if bytes.starts_with(&[0xFF, 0xFE]) {
            let w = utf16_units(&bytes[2..]);
            String::from_utf16_lossy(&w)
        } else {
            String::from_utf8_lossy(&bytes).into_owned()
        };
        if text.to_lowercase().contains(VENDOR_INF_MARKER) {
            out.push(e.file_name().to_string_lossy().into_owned());
        }
    }
    out
}

fn run_hidden(program: &str, args: &[&str], log: &dyn Fn(String)) -> Result<i32> {
    log(format!("> {program} {}", args.join(" ")));
    let out = Command::new(program)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .with_context(|| format!("run {program}"))?;
    for line in String::from_utf8_lossy(&out.stdout)
        .lines()
        .chain(String::from_utf8_lossy(&out.stderr).lines())
    {
        let l = line.trim();
        if !l.is_empty() {
            log(format!("  {l}"));
        }
    }
    Ok(out.status.code().unwrap_or(-1))
}

/// Uninstall the M-Audio package (MSI) and purge its INF from the driver store.
/// Returns true when something was removed; the caller must then ask for a reboot.
pub fn remove_vendor_driver(log: &dyn Fn(String)) -> Result<bool> {
    if !is_elevated() {
        bail!("removing the M-Audio driver needs administrator rights");
    }
    let mut changed = false;
    let pkgs = vendor_packages();
    if !pkgs.is_empty() {
        let _ = run_hidden("sc.exe", &["stop", "MTrack2X2AudioDevMon"], log);
    }
    for p in pkgs {
        log(format!(
            "Uninstalling '{}' {} ({}) ...",
            p.name, p.version, p.product_code
        ));
        let code = run_hidden("msiexec.exe", &["/X", &p.product_code, "/qn", "/norestart"], log)?;
        match code {
            0 | 3010 | 1641 => {
                log(format!(
                    "  msiexec exit {code} (removed{})",
                    if code == 0 { "" } else { ", reboot wanted" }
                ));
                changed = true;
            }
            _ => bail!("msiexec returned {code} while removing '{}'", p.name),
        }
    }
    for inf in leftover_vendor_infs() {
        log(format!("Deleting driver package {inf} from the driver store ..."));
        let code = run_hidden("pnputil.exe", &["/delete-driver", &inf, "/uninstall", "/force"], log)?;
        if code != 0 && code != 3010 {
            log(format!(
                "  pnputil returned {code}; the package may still be in use until a reboot"
            ));
        }
        changed = true;
    }
    if changed {
        let _ = run_hidden("pnputil.exe", &["/scan-devices"], log);
    }
    Ok(changed)
}

// ---------------------------------------------------------------- WinUSB binding (SetupAPI)

struct DevInfoSet(HDEVINFO);
impl Drop for DevInfoSet {
    fn drop(&mut self) {
        // SAFETY: set was created by SetupDiCreateDeviceInfoList.
        unsafe { SetupDiDestroyDeviceInfoList(self.0) };
    }
}

fn open_devinfo(instance_id: &str) -> Result<(DevInfoSet, SP_DEVINFO_DATA)> {
    // SAFETY: standard SetupAPI usage; structs zero-initialised with cbSize set.
    unsafe {
        let set = SetupDiCreateDeviceInfoList(null(), null_mut());
        if set as isize == INVALID_HANDLE_VALUE as isize {
            bail!("SetupDiCreateDeviceInfoList failed: {}", last_error());
        }
        let set = DevInfoSet(set);
        let mut data: SP_DEVINFO_DATA = std::mem::zeroed();
        data.cbSize = std::mem::size_of::<SP_DEVINFO_DATA>() as u32;
        let w = wide(instance_id);
        if SetupDiOpenDeviceInfoW(set.0, w.as_ptr(), null_mut(), 0, &mut data) == 0 {
            bail!("device {instance_id} not found: error {}", last_error());
        }
        Ok((set, data))
    }
}

/// Bind the in-box "WinUsb Device" model from winusb.inf to one device instance.
pub fn bind_winusb(instance_id: &str, log: &dyn Fn(String)) -> Result<()> {
    if !is_elevated() {
        bail!("binding WinUSB needs administrator rights");
    }
    let inf = inf_dir().join("winusb.inf");
    if !inf.exists() {
        bail!("{} is missing", inf.display());
    }
    log(format!("Binding WinUSB (winusb.inf) to {instance_id} ..."));
    // SAFETY: SetupAPI sequence mirroring Device Manager's "Have Disk" install; all structs
    // are zeroed with cbSize set and all pointers outlive the calls.
    unsafe {
        let (mut set, mut data) = open_devinfo(instance_id)?;
        let g = data.ClassGuid;
        if g.data1 == 0 && g.data2 == 0 && g.data3 == 0 && g.data4 == [0; 8] {
            // A device with no setup class gets no class-driver list: give it winusb.inf's class.
            log("  device has no setup class yet; assigning the USBDevice class".into());
            let mut g: Vec<u16> = wide(USBDEVICE_CLASS_GUID);
            let bytes: Vec<u8> = g.drain(..).flat_map(|c| c.to_le_bytes()).collect();
            if SetupDiSetDeviceRegistryPropertyW(set.0, &mut data, SPDRP_CLASSGUID, bytes.as_ptr(), bytes.len() as u32)
                == 0
            {
                bail!("SetupDiSetDeviceRegistryProperty(CLASSGUID) failed: {}", last_error());
            }
            let (s2, d2) = open_devinfo(instance_id)?;
            set = s2;
            data = d2;
        }

        let mut params: SP_DEVINSTALL_PARAMS_W = std::mem::zeroed();
        params.cbSize = std::mem::size_of::<SP_DEVINSTALL_PARAMS_W>() as u32;
        if SetupDiGetDeviceInstallParamsW(set.0, &data, &mut params) == 0 {
            bail!("SetupDiGetDeviceInstallParams failed: {}", last_error());
        }
        params.Flags |= DI_ENUMSINGLEINF;
        params.FlagsEx |= DI_FLAGSEX_ALLOWEXCLUDEDDRVS;
        let p = wide(&inf.to_string_lossy());
        if p.len() > params.DriverPath.len() {
            bail!("INF path too long");
        }
        params.DriverPath[..p.len()].copy_from_slice(&p);
        if SetupDiSetDeviceInstallParamsW(set.0, &data, &params) == 0 {
            bail!("SetupDiSetDeviceInstallParams failed: {}", last_error());
        }
        if SetupDiBuildDriverInfoList(set.0, &mut data, SPDIT_CLASSDRIVER) == 0 {
            bail!("SetupDiBuildDriverInfoList failed: {}", last_error());
        }
        let mut found: Option<SP_DRVINFO_DATA_V2_W> = None;
        let mut i = 0;
        loop {
            let mut drv: SP_DRVINFO_DATA_V2_W = std::mem::zeroed();
            drv.cbSize = std::mem::size_of::<SP_DRVINFO_DATA_V2_W>() as u32;
            if SetupDiEnumDriverInfoW(set.0, &data, SPDIT_CLASSDRIVER, i, &mut drv) == 0 {
                break;
            }
            let desc = from_wide(&drv.Description);
            log(format!("  [{i}] {desc} ({})", from_wide(&drv.ProviderName)));
            if found.is_none() && desc.to_lowercase().contains("winusb device") {
                found = Some(drv);
            }
            i += 1;
        }
        let Some(mut drv) = found else {
            SetupDiDestroyDriverInfoList(set.0, &data, SPDIT_CLASSDRIVER);
            bail!("winusb.inf offers no 'WinUsb Device' model for this device ({i} models listed)");
        };
        if SetupDiSetSelectedDriverW(set.0, &mut data, &mut drv) == 0 {
            let e = last_error();
            SetupDiDestroyDriverInfoList(set.0, &data, SPDIT_CLASSDRIVER);
            bail!("SetupDiSetSelectedDriver failed: {e}");
        }
        let mut reboot: windows_sys::core::BOOL = 0;
        let ok = DiInstallDevice(null_mut(), set.0, &data, &drv, 0, &mut reboot);
        let e = last_error();
        SetupDiDestroyDriverInfoList(set.0, &data, SPDIT_CLASSDRIVER);
        if ok == 0 {
            bail!("DiInstallDevice failed: error {e:#x}");
        }
        log(format!(
            "  WinUSB installed{}",
            if reboot != 0 {
                " (Windows asked for a reboot)"
            } else {
                ""
            }
        ));
    }
    Ok(())
}

fn restart_node(instance_id: &str, log: &dyn Fn(String)) -> Result<()> {
    let code = run_hidden("pnputil.exe", &["/restart-device", instance_id], log)?;
    if code != 0 {
        log(format!("  pnputil /restart-device returned {code}"));
    }
    std::thread::sleep(Duration::from_secs(3));
    Ok(())
}

/// Make sure `prefix` (a device-instance prefix) is bound to WinUSB with our interface GUID.
/// Returns true when something was changed (the node was restarted).
fn ensure_winusb(prefix: &str, what: &str, log: &dyn Fn(String)) -> Result<bool> {
    let node = find_node(prefix).ok_or_else(|| anyhow!("{what} is not present"))?;
    let mut changed = false;
    if !node.service.eq_ignore_ascii_case("WINUSB") {
        bind_winusb(&node.instance_id, log)?;
        changed = true;
    } else {
        log(format!("{what}: WinUSB already bound."));
    }
    if !interface_guid_set(&node.instance_id) {
        set_interface_guid(&node.instance_id)?;
        log(format!("{what}: DeviceInterfaceGUIDs registered."));
        changed = true;
    }
    if changed {
        restart_node(&node.instance_id, log)?;
    }
    Ok(changed)
}

pub fn prepare_runtime_dfu_access(log: &dyn Fn(String)) -> Result<bool> {
    ensure_winusb(RUNTIME_DFU, "audio-mode DFU interface (MI_03)", log)
}

pub fn prepare_dfu_mode_access(log: &dyn Fn(String)) -> Result<bool> {
    ensure_winusb(DFU_MODE, "DFU-mode device (PID C00B)", log)
}

// ---------------------------------------------------------------- bus reset

/// Ask the upstream hub to cycle the device's port: a real USB reset + re-enumeration, which
/// is what DFU_DETACH needs ("will not detach" device). WinUSB itself cannot reset a device.
pub fn bus_reset(info: &DeviceInfo, _dev: &Device, log: &dyn Fn(String)) -> Result<()> {
    let parent = info.parent_instance_id().to_string_lossy().into_owned();
    let port = info.port_number();
    if parent.is_empty() || port == 0 {
        bail!("cannot determine the upstream hub/port of the device");
    }
    let path = format!(
        "\\\\?\\{}#{{f18a0e88-c30c-11d0-8815-00a0c906bed8}}",
        parent.replace('\\', "#")
    );
    log(format!("Cycling port {port} of hub {parent} ..."));
    // SAFETY: documented hub IOCTL with a correctly sized in/out struct.
    unsafe {
        let w = wide(&path);
        let h = CreateFileW(
            w.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            null(),
            OPEN_EXISTING,
            0,
            null_mut(),
        );
        if h == INVALID_HANDLE_VALUE {
            bail!("cannot open hub {path}: error {}", last_error());
        }
        let mut p = USB_CYCLE_PORT_PARAMS {
            ConnectionIndex: port,
            StatusReturned: 0,
        };
        let mut ret = 0u32;
        let ok = DeviceIoControl(
            h,
            IOCTL_USB_HUB_CYCLE_PORT,
            &p as *const _ as *const _,
            std::mem::size_of::<USB_CYCLE_PORT_PARAMS>() as u32,
            &mut p as *mut _ as *mut _,
            std::mem::size_of::<USB_CYCLE_PORT_PARAMS>() as u32,
            &mut ret,
            null_mut(),
        );
        let e = last_error();
        CloseHandle(h);
        if ok == 0 {
            bail!("IOCTL_USB_HUB_CYCLE_PORT failed: error {e}");
        }
        let status = p.StatusReturned;
        log(format!("  port cycled (status {status:#x})"));
    }
    Ok(())
}

// ---------------------------------------------------------------- descriptors

/// nusb reads the configuration descriptor through the hub on Windows, so this works with any
/// (or no) driver bound to the device.
pub fn read_config_descriptor(info: &DeviceInfo) -> Result<Vec<u8>> {
    use nusb::MaybeFuture;
    let dev = info.open().wait().context("open device (hub descriptor read)")?;
    let cfg = dev.configurations().next().map(|c| c.as_bytes().to_vec());
    cfg.ok_or_else(|| anyhow!("device reports no configuration descriptor"))
}

// ---------------------------------------------------------------- report

pub fn host_report(_runtime: Option<&DeviceInfo>, _dfu: Option<&DeviceInfo>) -> HostReport {
    let mut r = HostReport::default();
    if is_elevated() {
        r.items.push(ReportItem::good("Running as administrator."));
    } else {
        r.items.push(ReportItem::bad(
            "Not running as administrator: driver binding and port cycling will fail.",
        ));
        r.needs_privilege = true;
    }

    let pkgs = vendor_packages();
    for p in &pkgs {
        r.items.push(ReportItem::bad(format!(
            "M-Audio driver package installed: '{}' {}. It takes over the whole device and must be removed.",
            p.name, p.version
        )));
        r.vendor_driver = true;
    }
    let infs = leftover_vendor_infs();
    if !infs.is_empty() {
        r.items.push(ReportItem::bad(format!(
            "M-Audio driver still in the driver store ({}); Windows would bind it again on replug.",
            infs.join(", ")
        )));
        r.vendor_driver = true;
    }
    if pkgs.is_empty() && infs.is_empty() {
        r.items.push(ReportItem::good("No M-Audio driver package installed."));
    }

    let nodes = present_nodes("VID_0763");
    let up = |n: &PnpNode, p: &str| n.instance_id.to_uppercase().starts_with(p);
    if let Some(n) = nodes.iter().find(|n| up(n, RUNTIME_PARENT)) {
        if n.service.eq_ignore_ascii_case(VENDOR_SERVICE) {
            r.items.push(ReportItem::bad(
                "The M-Audio kernel driver (MTRACK2X2) is bound to the device.",
            ));
            r.vendor_driver = true;
        } else if n.service.eq_ignore_ascii_case("usbccgp") {
            r.items
                .push(ReportItem::good("Device uses the in-box USB Composite Device driver."));
        } else {
            r.items.push(ReportItem::info(format!(
                "Device node driver: '{}' (problem code {}).",
                if n.service.is_empty() { "none" } else { &n.service },
                n.problem
            )));
        }
        if let Some(a) = nodes.iter().find(|n| up(n, RUNTIME_AUDIO)) {
            if a.service.eq_ignore_ascii_case("usbaudio2") {
                r.items.push(ReportItem::good(
                    "Audio function uses the in-box USB Audio 2.0 driver (usbaudio2).",
                ));
            } else {
                r.items.push(ReportItem::info(format!(
                    "Audio function driver: '{}' (problem code {}).",
                    if a.service.is_empty() { "none" } else { &a.service },
                    a.problem
                )));
            }
        }
        match nodes.iter().find(|n| up(n, RUNTIME_DFU)) {
            Some(d) if d.service.eq_ignore_ascii_case("WINUSB") && interface_guid_set(&d.instance_id) => {
                r.items.push(ReportItem::good(
                    "DFU interface is bound to WinUSB (ready for flashing).",
                ));
            }
            Some(d) => {
                r.items.push(ReportItem::bad(format!(
                    "DFU interface has no usable driver yet ('{}', problem code {}). The tool binds the in-box WinUSB driver when you flash.",
                    if d.service.is_empty() { "none" } else { &d.service },
                    d.problem
                )));
                r.needs_setup = true;
            }
            None => {
                if !r.vendor_driver {
                    r.items.push(ReportItem::info(
                        "No separate DFU interface node (device not enumerated as composite yet).",
                    ));
                }
            }
        }
    }
    if let Some(n) = nodes.iter().find(|n| up(n, DFU_MODE)) {
        if n.service.eq_ignore_ascii_case("WINUSB") && interface_guid_set(&n.instance_id) {
            r.items.push(ReportItem::good(
                "DFU-mode device (PID C00B) is present and bound to WinUSB.",
            ));
        } else {
            r.items.push(ReportItem::info(
                "DFU-mode device (PID C00B) is present; WinUSB will be bound to it when needed.",
            ));
            r.needs_setup = true;
        }
    }
    let sys = PathBuf::from(std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into()))
        .join("System32\\drivers\\MAudioMTrack2X2.sys");
    if Path::new(&sys).exists() && pkgs.is_empty() && infs.is_empty() {
        r.items.push(ReportItem::info(
            "Leftover MAudioMTrack2X2.sys in System32\\drivers (harmless, not bound).",
        ));
    }
    if reg_open(
        HKEY_LOCAL_MACHINE,
        "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Component Based Servicing\\RebootPending",
        KEY_READ,
    )
    .is_some()
    {
        r.items.push(ReportItem::info(
            "Windows reports a pending reboot; device configuration may be unreliable until then.",
        ));
        r.needs_reboot = true;
    }
    r
}

pub fn open_error_hint(e: &anyhow::Error) -> Option<String> {
    let s = e.to_string().to_lowercase();
    if s.contains("winusb") || s.contains("incompatible driver") || s.contains("deviceinterfaceguid") {
        Some(
            "The DFU interface is not bound to WinUSB yet; the tool will bind it (administrator rights needed).".into(),
        )
    } else {
        None
    }
}
