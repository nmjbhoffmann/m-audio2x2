# DFU on the M-Track 2X2

How the device's firmware update interface behaves, the quirks that make it different from a
textbook DFU 1.1 device, the Windows-specific plumbing, and what the tool does step by step. All
of it was observed on the author's unit (firmware `1.02`) between 2026-11-04 and 2026-11-07, and
cross-checked against the XMOS USB Audio reference source (`sc_usb_audio` / `lib_xua` 1.x, which
this firmware is built from).

## The model

The firmware runs the XMOS reference DFU implementation on top of `libflash`:

- The SPI flash holds a boot loader, a **factory image** and room for one **upgrade image** (see
  [hardware.md](hardware.md) for the layout). The loader boots the upgrade image when it is present,
  its CRCs verify and its version is higher than the factory image's; otherwise the factory image.
- `DFU_DNLOAD` erases the upgrade area and writes the new image after the factory image, at the next
  4 KiB sector boundary (`0x7000` on this unit). The factory image is never written by DFU.
- `DFU_UPLOAD` returns the **upgrade image only**, padded to the 256-byte page. With only the factory
  image present it returns 0 bytes. The factory image cannot be read over USB.
- Two modes with different USB identities:

| Mode | VID:PID | Interfaces | DFU interface | How you get there |
|---|---|---|---|---|
| Runtime (audio) | `0763:400b` | audio 0 to 2, DFU 3 (protocol 1) | number 3 | power-up, or `RESETFROMDFU` |
| DFU | `0763:c00b` | one (protocol 2) | number 0 | `DFU_DETACH` + bus reset |

  M-Audio gave DFU mode its own PID (the XMOS default is to keep the same one). This is convenient
  on Windows: the audio driver binding on `400b` is never disturbed by what gets bound to `c00b`.
- Transfers are 64-byte blocks (`wTransferSize 64`), four per 256-byte flash page. The fixed image
  is 351 blocks.
- XMOS vendor extensions, sent as class requests to the DFU interface (`bmRequestType 0x21`):

| bRequest | Name | Effect |
|---|---|---|
| `0xF0` | `XMOS_DFU_RESETDEVICE` | reboot |
| `0xF1` | `XMOS_DFU_REVERTFACTORY` | erase all upgrade images; factory firmware boots |
| `0xF2` | `XMOS_DFU_RESETINTODFU` | reboot into DFU mode (RAM flag `0x11042011` survives the soft reset) |
| `0xF3` | `XMOS_DFU_RESETFROMDFU` | reboot into runtime mode |

A power cycle always returns the device to runtime mode (the DFU flag lives in RAM).

## The quirks

Each of these cost time to find. The tool is written around all of them.

1. **One DFU request per power-up in runtime mode, and it must be `DFU_DETACH`.** In runtime mode
   endpoint 0 tells the audio core to stop (`AUDIO_STOP_FOR_DFU`) before **every** DFU-interface
   request and waits for a handshake. The audio core handshakes once, then parks in a wait loop that
   never handshakes again. The second DFU request after a power-up therefore hangs endpoint 0 for good:
   the device stops answering, drops off the bus after a reset, and only a power cycle recovers it.
   Current `lib_xua` has a notify-once flag; this firmware predates it. So: no `GETSTATUS` first, no
   `GETSTATE`, no `0xF2`. Send `DFU_DETACH` and nothing else. In DFU mode the rule does not apply
   (the stop-audio path is skipped).
2. **`DFU_GETSTATE` hangs the device** in either mode. `GETSTATUS` works; `GETSTATE` does not.
   XMOS's own host tool never uses it. Standard DFU tools such as `dfu-util` do, so do not point them
   at this device.
3. **"Will Not Detach".** `DFU_DETACH` is accepted but the device does not reset itself; the host must
   perform a real USB bus reset afterwards. The device declares a 250 ms detach window; the tool asks
   for 30 s in `wValue` and the reset that follows the request within a second or two is honoured.
   A libusb/WinUSB "reset device" is not a bus reset on Windows (see below).
4. **Download is silently ignored while an upgrade image exists.** `flash_cmd_write_page` returns
   without writing. The blocks are acknowledged, the status is OK, and nothing changes. Send
   `REVERTFACTORY` first, then download.
5. **Upload right after a download returns 0 bytes.** The firmware decides whether an upgrade image
   exists when the DFU session opens (`flash_cmd_init`). To verify a write, open a new session (go
   back to runtime mode and detach again) or, simpler, re-read the live configuration descriptor after
   the reboot, which is what the tool does.
6. **Upload returns only the upgrade image.** A fresh unit uploads 0 bytes. Backups of the factory
   firmware need a flash programmer ([hardware.md](hardware.md)).
7. **A stray image outside the libflash slot makes DFU mode unreliable.** With an image at `0x10000`
   and the slot at `0x7000` empty, the device failed to enter DFU mode twice. Only ever place images
   where libflash puts them.
8. **Revert is the rollback.** `REVERTFACTORY` erases the upgrade image, and a failed or interrupted
   download leaves an image whose CRC fails, which the loader ignores. The factory image is the
   fallback at every boot.

## Windows specifics

Goal: no downloads, no third-party driver packages, in-box components only.

- **Driver binding.** Windows has no in-box DFU driver, so `MI_03` sits with problem code 28 after
  enumeration. The tool binds Windows' own `winusb.inf` ("WinUsb Device" model) to that node with
  SetupAPI (`SetupDiSetSelectedDriver` + `DiInstallDevice`, the same thing Device Manager's
  "Update driver > Let me pick" does). The node has no setup class, so the tool first gives it
  `winusb.inf`'s class, `USBDevice {88BAE032-5A81-49F0-BC3D-A4FF138216D6}`. The same is done for
  the DFU-mode node `USB\VID_0763&PID_C00B` the first time it appears. Bindings persist per USB port.
- **`DeviceInterfaceGUIDs`.** Stock `winusb.inf` registers no device interface, and user-mode USB
  libraries (nusb, libusb) find WinUSB devices through one. The tool writes a `REG_MULTI_SZ`
  `DeviceInterfaceGUIDs` = `{6C5F8B6A-3B1B-4B7E-9C19-2A0C8D3E4F10}` (a project-specific GUID) under
  the node's `Device Parameters` key and restarts the node.
- **Bus reset.** WinUSB cannot reset a device, and `pnputil /restart-device` refuses on this node
  ("reboot needed"). What works is asking the upstream hub to cycle the port: open the hub's device
  interface (`\\?\<parent instance id with # instead of \>#{f18a0e88-c30c-11d0-8815-00a0c906bed8}`)
  and send `IOCTL_USB_HUB_CYCLE_PORT` with `USB_CYCLE_PORT_PARAMS.ConnectionIndex` = the device's port
  number. The device re-enumerates; after `DFU_DETACH` it comes back as `c00b`. Needs administrator
  rights.
- **The broken-handle state.** After driver changes (the vendor-package uninstall in particular, and
  sometimes after an interrupted run) every control transfer through the WinUSB handle fails instantly
  with Windows error 31 (`ERROR_GEN_FAILURE`), even a standard `GET_DESCRIPTOR`. The firmware never
  sees the request. Only a Windows reboot clears it; a replug does not. Because of quirk 1 the tool
  must not spend the one DFU request on a dead handle, so before `DFU_DETACH` it probes the handle with
  a standard `GET_DESCRIPTOR(device)` (a standard request, not a DFU-interface request, so it does not
  count) and tells the user to reboot if that fails.
- **Reading the descriptor without a driver.** `nusb` reads the configuration descriptor through the
  hub (`IOCTL_USB_GET_DESCRIPTOR_FROM_NODE_CONNECTION`), so the descriptor check works whatever driver,
  or no driver, is bound.
- **Vendor package.** The M-Audio driver binds the whole device (`USB\VID_0763&PID_400B`, replacing
  `usbccgp`), so no `MI_03` child exists while it is installed. The tool finds it through its MSI
  uninstall entries, the `MTRACK2X2` service and leftover `oem*.inf` packages, removes all of them
  (`msiexec /X`, `pnputil /delete-driver`), and asks for a reboot. Without the reboot the device stack
  stays half-configured and the handle state above follows.

## Linux specifics

- `snd-usb-audio` claims interfaces 0 to 2 only; interface 3 is free. No driver work.
- Permission: a udev rule (`/etc/udev/rules.d/70-mtrack2x2-dfu.rules`, `MODE="0666"`,
  `TAG+="uaccess"` for both PIDs), installed by `sudo mtrack-fix fix-host`. Or run as root.
- Bus reset: `USBDEVFS_RESET` through the device node, which is a real port reset on Linux.
- The one-request rule and the `c00b` PID apply exactly as on Windows.

## What the tool does, step by step

`mtrack-fix flash` (the GUI's **Flash firmware** runs the same code):

1. **Host check.** Windows: elevated? vendor package present (abort if so)? `usbccgp` on the parent,
   `usbaudio2` on `MI_00`, what is on `MI_03`? Linux: udev rule or root?
2. **Image check.** Parse the image (built-in or given), verify tag and both CRCs, require
   `version > 0`, detect bit order.
3. **Find the device.** `0763:400b` (runtime) or `0763:c00b` (already in DFU mode, in which case
   steps 4 to 6 are skipped).
4. **DFU access.** Windows: bind WinUSB + GUID to `MI_03` if not already done. Open the interface.
5. **Handle probe.** Standard `GET_DESCRIPTOR(device)`. Failure means "reboot Windows"; no DFU request
   has been sent.
6. **Detach and reset.** `DFU_DETACH` (the single runtime request), then the hub port cycle (Windows)
   or `USBDEVFS_RESET` (Linux). Wait for `0763:c00b`. Windows: bind WinUSB + GUID to the DFU-mode
   node the first time. If the device does not come back, prompt for a replug and try once more.
7. **Status.** `DFU_GETSTATUS` in DFU mode (safe there), expect `dfuIDLE`.
8. **Read the current upgrade image.** `DFU_UPLOAD` until a short block. If it equals the image to be
   written, stop ("nothing to do") unless forced. If any image is present, `REVERTFACTORY`, then read
   again and require 0 bytes.
9. **Download.** `DFU_DNLOAD` 64-byte blocks with `GETSTATUS` after each, then a zero-length
   `DFU_DNLOAD` to finish, then `GETSTATUS` through manifestation.
10. **Back to runtime mode.** `RESETFROMDFU`. Wait for `0763:400b`. If it does not appear, try a bus
    reset, then prompt for a replug (a power cycle always works).
11. **Verify.** Read the live configuration descriptor and check OT 22's `bSourceID`. Report
    "FIXED" or explain that the loader rejected the image.

`mtrack-fix revert` runs steps 1 and 3 to 7, sends `REVERTFACTORY`, then steps 10 and 11
(expecting "BUG PRESENT"). `mtrack-fix backup` runs up to step 8 and writes what `DFU_UPLOAD`
returned.

Timeouts: 5 s per control transfer, up to 30 s for the device to appear in DFU mode, 15 s for it to return to audio mode, 180 s for a replug prompt.
