# Using the tool

`mtrack-fix` is one program with a window (double-click it) and a command line (run it with an
argument). Both do the same things. The fixed firmware is built in.

## Before you start

- Confirm you have an **M-Track 2X2** (USB-C model, USB ID `0763:400b`). The tool refuses other
  devices.
- Plug the interface directly into the computer, not through a hub if you can avoid it.
- Close anything that is using the interface for audio.

## Windows, step by step

### 1. Remove the M-Audio driver (only if you installed it)

The M-Audio driver package takes over the whole device, including the update interface, so it has
to go first. If you never installed it, skip to step 2.

1. Start `mtrack-fix.exe`. Windows asks for administrator rights; say yes.
2. The status list shows "M-Audio driver package installed". Click **Remove M-Audio driver**.
   The tool uninstalls the package and deletes its leftover driver files from Windows' driver store
   so that Windows cannot silently re-attach it later.
3. **Reboot.** This is required; the Windows USB stack is in a half-configured state after a driver
   uninstall, and that state is known to break the next step.
4. After the reboot, unplug the interface, wait two seconds, and plug it back in. Windows now uses its
   own drivers: "USB Composite Device" and "USB Audio 2.0".

### 2. Check

Start `mtrack-fix.exe` again. The status list should say:

- the M-Track 2X2 is connected and the firmware has the bug ("Firmware: BUG PRESENT ...");
- running as administrator;
- no M-Audio driver package installed;
- the device uses the in-box USB Composite Device driver and the audio function uses `usbaudio2`;
- the DFU interface has no driver yet (the "!!" line). That is expected; the tool handles it.

If it already says "Firmware: FIXED", there is nothing to do.

### 3. Flash

Click **Flash firmware**. The log shows each step:

1. **Setting up access to the DFU interface.** The tool attaches Windows' own WinUSB driver to the
   device's update interface. Nothing is downloaded. This happens automatically the first time on each
   USB port; you may see the device blink in Device Manager.
2. **Switching to DFU mode.** The tool sends one "detach" request and then asks the USB hub to reset
   the port. The device comes back as a different USB device ("M-Track 2X2 DFU", ID `0763:c00b`).
   The first time, the tool attaches WinUSB to that device too.
3. **Reading the current upgrade image.** On an untouched unit this is empty. If a fix is already
   there, the tool first removes it (the device ignores new images while one is present).
4. **Writing the firmware.** 351 blocks of 64 bytes. A few seconds.
5. **Returning to audio mode.** The device reboots itself into normal mode.
6. **Verifying.** The tool reads the USB description again and reports "FIXED".

### 4. Verify in Windows

Open Settings > System > Sound (or the classic Sound control panel). Under input there is now
**Microphone (M-Track 2X2)**. If it is not there yet, unplug and replug the interface once.

Both inputs arrive as one stereo recording device: input 1 on the left channel, input 2 on the right.

## Linux, step by step

Linux does not need the fix for audio. You might still apply it so the unit works on a Windows machine
later, or to run the tool's checks.

1. Download `mtrack-fix-linux-x64` from the
   [releases page](https://github.com/nmjbhoffmann/m-audio2x2/releases), or build it
   (see [building.md](building.md)). Make it executable:

   ```
   chmod +x mtrack-fix-linux-x64
   mv mtrack-fix-linux-x64 mtrack-fix
   ```

2. Start `./mtrack-fix` as your normal user (not with `sudo`). The window opens.
3. The first time, the status list says there is no udev rule. Click **Set up USB access** and enter
   your password when asked. This installs `/etc/udev/rules.d/70-mtrack2x2-dfu.rules`, which lets
   normal users talk to the device's update interface. It applies at once; no replug needed.
   Flashing without the rule asks for the password the same way.
4. Click **Flash firmware**. The steps are the same as on Windows.
5. `lsusb -v -d 0763:400b | grep -A3 "OUTPUT_TERMINAL.*22"` now shows `bSourceID 1`.

From a terminal, `./mtrack-fix check` and `./mtrack-fix flash` do the same. Without a desktop (for
example over SSH) there is no password prompt; install the rule with `sudo ./mtrack-fix fix-host`
once instead.

No kernel driver is replaced on Linux. The sound driver only claims the audio interfaces; the update
interface is free.

## Command line

```
mtrack-fix check                   device, descriptor and host/driver state
mtrack-fix fix-host                remove the M-Audio driver (Windows) / install the udev rule (Linux)
mtrack-fix flash [image] [--force] flash the built-in fixed firmware (or a given image)
mtrack-fix revert                  back to the factory firmware
mtrack-fix backup <out.bin>        save the current upgrade image (0 bytes = factory only)
mtrack-fix verify <image>          check an xflash image's header and checksums
```

On Windows the command line works from an existing console window (`cmd` or PowerShell) started
as administrator.

## What the prompts mean

| Message | Meaning | What to do |
|---|---|---|
| "REBOOT the computer now" | The M-Audio package was removed. Windows must restart before the device can be used again. | Reboot, replug, start the tool again. |
| "UNPLUG the M-Track 2X2, wait 2 seconds, and PLUG IT BACK IN" | The device dropped off the bus or did not come back in the expected mode. A power cycle always returns it to normal audio mode. | Do exactly that. The tool waits up to a few minutes and continues by itself. |
| "The USB handle is in a broken state that only a REBOOT clears" | Windows is refusing every USB request to the update interface (a known host-side state after driver changes). The tool detects this **before** sending anything to the device. | Reboot Windows, replug, try again. |
| "This must be the first DFU request since power-up" | Informational. The device only tolerates one update request per power-up in audio mode. | Nothing. If a previous attempt failed after this point, replug before retrying. |
| "Nothing to do: this firmware is already installed" | The device already holds exactly this image. | Nothing. Use `--force` or the "Re-flash" checkbox if you really want to rewrite it. |
| "the M-Audio driver is installed; remove it first" | The vendor package is present. | Step 1 above. |
| "this needs administrator/root rights" | Driver binding and the hub port reset need elevation. | Windows: run as administrator. |
| "the password prompt was cancelled" (Linux) | The udev rule was not installed. | Click **Set up USB access** again and enter your password. |
| "no polkit password agent is running" (Linux) | No desktop session to ask for the password, for example over SSH. | `sudo ./mtrack-fix fix-host` once. |

## Troubleshooting

**Every USB request fails instantly, or the tool says the handle is broken.** This is a Windows
state, not a device fault. It happens after driver changes (for example the M-Audio uninstall) and
sometimes after an interrupted attempt. Reboot Windows. A replug alone is usually not enough.

**The device disappeared and does not come back.** The firmware hung. This happens if a second
update request reaches the device in audio mode (for example from another program, or from a
retry without a replug in between). Unplug the device, wait two seconds, plug it back in. It always
boots the normal firmware after a power cycle.

**"The device did not enter DFU mode".** The hub port reset did not produce a re-enumeration. Replug
and try again. If it happens repeatedly, try another USB port, directly on the computer.

**One update request per power-up.** In audio mode the device accepts exactly one request on its
update interface; a second one freezes it until the next power cycle. The tool sends only that one
request (`DFU_DETACH`) and does everything else in update mode, where the limit does not apply. Do
not run other DFU tools (such as `dfu-util`) against the device in audio mode; querying its status
counts as the one request.

**The tool says "Windows reports a pending reboot".** Reboot first. Device configuration is
unreliable until then.

**After flashing, Windows still shows only Speakers.** Replug once. If it still does not appear,
run `mtrack-fix check`: if it says "FIXED", the firmware is right and the issue is on the Windows
side (check Sound settings > "Show disabled devices"). If it says "BUG PRESENT", the loader rejected the
image; please report the output of `check` and your firmware version.

**The M-Audio driver came back after a replug.** The package was removed but its driver files were
still in Windows' driver store. Run **Remove M-Audio driver** again; it also deletes leftover
`oem*.inf` packages. Reboot.

**Linux: "permission denied" or the device is not found.** Click **Set up USB access** (or run
`sudo ./mtrack-fix fix-host` once). If it still fails, replug the interface.

**Going back.** **Revert to factory** / `mtrack-fix revert` at any time. It follows the same
detach-and-reset dance, erases the upgrade image, and the device boots the factory firmware.
