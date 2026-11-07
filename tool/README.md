# mtrack-fix: M-Audio M-Track 2X2 firmware fix tool

A single-file, no-dependencies desktop tool (Windows and Linux) that flashes the fixed firmware onto an
M-Track 2X2 over its built-in DFU interface, and walks the user through the host-side steps.

The bug: the factory firmware's USB descriptor wires the capture terminal (Output Terminal 22) to USB
playback instead of the microphone. Windows' in-box USB Audio 2.0 driver therefore never creates a
microphone/recording device. The fix is a one-byte change in the firmware, shipped here as a signed
*upgrade* image. The factory image stays on the chip and remains the automatic fallback.

## What the tool does

| Feature | How |
|---|---|
| Detect the bug | Reads the configuration descriptor (Windows: via the USB hub, works with any driver; Linux: via sysfs) and inspects OT22's `bSourceID`. |
| Detect the M-Audio driver | Windows: MSI uninstall entries, the `MTRACK2X2` service bound to the device, and leftover `oem*.inf` packages in the driver store. Offers one-click removal (`msiexec /X`, `pnputil /delete-driver`), then asks for a reboot. |
| DFU access without downloads | Windows: binds the in-box `winusb.inf` "WinUsb Device" model to the DFU interface node with SetupAPI (the same thing Device Manager's "Let me pick" does), registers a `DeviceInterfaceGUIDs` value, restarts the node. Linux: installs a udev rule. |
| Flash | `DFU_DETACH` (the one runtime request the firmware tolerates), real bus reset (Windows: hub port cycle IOCTL; Linux: `USBDEVFS_RESET`), wait for the DFU-mode device (`0763:C00B`), read the current upgrade image, revert to factory if one exists, `DFU_DNLOAD` in 64-byte blocks, `RESETFROMDFU`, re-read the descriptor to verify. |
| Revert | `XMOS_DFU_REVERTFACTORY`: deletes the upgrade image, the device boots the factory firmware again. |
| User prompts | Replug / reboot instructions appear in the amber banner (GUI) or on the console (CLI); the tool waits for the device to go away and come back. |
| Custom images | Any xflash image can be selected (path or drag-and-drop). It is validated (tag, loader CRCs, version > 0) in either bit order before anything is sent. |

Firmware quirks the flow is built around are documented in `src/dfu.rs` and in `firmware/README.md`
of the research folder (one DFU request per power-up in audio mode, `GETSTATE` hangs the device,
DFU mode has its own PID, a broken WinUSB handle needs a Windows reboot).

## Usage

GUI: run `mtrack-fix` (Windows asks for administrator rights; the release build requires them because
driver binding and the hub port cycle need them).

CLI (same binary):

```
mtrack-fix check                      device, descriptor and host/driver state
mtrack-fix fix-host                   remove the M-Audio driver / set up DFU access
mtrack-fix flash [image] [--force]    flash the built-in fixed firmware (or a given image)
mtrack-fix revert                     back to the factory firmware
mtrack-fix backup <out.bin>           save the current upgrade image (0 bytes = factory only)
mtrack-fix verify <image>             check an xflash image's CRCs
```

Linux: run with `sudo` once (`sudo ./mtrack-fix fix-host` installs `/etc/udev/rules.d/70-mtrack2x2-dfu.rules`),
then replug the interface and run the tool as a normal user.

### Windows user flow

1. `check`. If the M-Audio package is installed: **Remove M-Audio driver**, reboot, replug.
2. **Flash firmware** (it binds WinUSB to the DFU interface by itself, in audio mode and again for the
   DFU-mode device the first time each appears on a USB port).
3. The tool reboots the device and verifies the descriptor. Windows now lists
   *Microphone (M-Track 2X2)*.

If the tool reports that the USB handle is broken, reboot Windows and retry; this is a host-side
state (every transfer fails instantly) that only a reboot clears, and the tool detects it before
spending the single DFU request.

## Building

```
cargo build --release
```

- Rust stable. Windows: MSVC toolchain. Linux: `libxkbcommon-dev libwayland-dev libxcb1-dev libxrandr-dev libxi-dev libgl1-mesa-dev libudev-dev` (build-time only; the binary loads X11/Wayland/GL at runtime).
- Pure-Rust USB stack (`nusb`), no libusb. GUI is `egui`/`eframe`.
- `build.rs` embeds a Windows manifest: `requireAdministrator` for release builds, `asInvoker` for debug/test builds.
- `.github/workflows/build.yml` builds, lints and tests both targets; a `v*` tag publishes a GitHub release with the two binaries and the firmware image.

## Layout

```
src/main.rs        CLI entry / GUI launch
src/gui.rs         egui front end
src/engine.rs      the flows (check, fix-host, flash, revert, backup) emitting events
src/dfu.rs         DFU + XMOS vendor requests over nusb
src/image.rs       xflash image parse/verify/bit-reverse; built-in signed image
src/descriptor.rs  OT22 bSourceID check
src/platform/      windows.rs (PnP, SetupAPI WinUSB bind, hub port cycle, vendor package),
                   linux.rs (udev, sysfs descriptors), other.rs (fallback)
firmware/          mtrack2x2-upgrade-v1-signed.bin (logical byte order; embedded at build time)
```

## License

Code and docs: MIT. The firmware image in `firmware/` is not covered: it is derived from M-Audio's
factory firmware. See the repository's `LICENSE` and `firmware/README.md`.
