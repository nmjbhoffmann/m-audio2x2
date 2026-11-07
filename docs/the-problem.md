# The problem

## Symptom

On Windows, with no M-Audio driver installed, the M-Track 2X2 shows up as **Speakers (M-Track 2X2)**
only. There is no recording device, not even a disabled one. Sound output works, the microphone
inputs do not.

On Linux the same unit works completely: playback, recording, every sample rate, no errors.

## Why

Every USB device hands the computer a description of itself when it is plugged in. For an audio
device this description includes a small wiring diagram: which inputs feed which outputs. The
operating system builds its audio devices from that diagram.

The M-Track 2X2's diagram has one wrong entry. The block that sends recorded audio to the computer
("Output Terminal 22") says its source is the **playback stream from the computer** instead of the
**microphone inputs**. The microphone block is listed, but nothing points at it.

| | What the device says | What it should say |
|---|---|---|
| Recording stream source | USB playback (terminal 2) | Microphone (terminal 1) |

The two operating systems react differently:

- **Linux** builds its playback and recording streams from the USB streaming interfaces and mostly
  ignores the wiring diagram. Recording works.
- **Windows' built-in USB Audio 2.0 driver (`usbaudio2.sys`)** follows the diagram. It builds a
  recording path, but the only thing feeding it is the generic "data from the computer" block, which
  is not a physical input. Windows only creates a Recording device when it can trace the path back to
  something physical like a microphone or line input. It cannot, so it creates nothing.

This is a bug in the device's firmware, not in Windows. It looks like a leftover from how the
firmware was adapted from XMOS's reference design: a volume control block was removed from the
recording path and the source number was never re-pointed at the microphone.

M-Audio's own Windows driver (version 1.0.13 from 2021) does not use the diagram at all; it has the
device's layout hard-coded. That is why recording works with it. The downside is that it is an old
driver that takes over the whole device, keeps it streaming all the time and will not be updated.

## What the fix changes

Exactly **one byte** in the firmware: the source number of Output Terminal 22, from `2` to `1`.
Nothing else about the device changes. Sample rates, latency, the knobs, the headphone output and
the Linux behaviour are all the same as before.

The byte lives inside the firmware image stored on a small flash chip in the device. The fix is a
copy of the original firmware with that byte changed, the version number raised from 0 to 1 and the
image checksums recomputed so the device's boot loader accepts it.

## How it gets onto the device

The device has a built-in firmware update interface (USB DFU, the standard way XMOS-based devices are
updated). The flash chip holds the **factory image** in one area and has room for an **upgrade
image** in another. At power-up the boot loader uses the upgrade image if it is present and valid,
and otherwise the factory image.

The tool writes the fix as an upgrade image. The factory image is never touched; the update
interface on this device physically cannot write to it.

After the update, Windows' built-in driver sees the corrected diagram and creates
**Microphone (M-Track 2X2)** next to Speakers. Verified on Windows 11 24H2 on 2026-11-04 and again
with the finished tool on 2026-11-07.

## Risks and rollback

- **Bricking:** very unlikely by design. If the upgrade image is incomplete or corrupt, its checksum
  fails and the device simply boots the factory image. The boot loader and the factory image are
  outside the area the update interface can write.
- **A hung device:** the firmware's update interface has a few quirks (it tolerates only one update
  request per power-up in normal mode, and one particular request freezes it). The tool is written
  around them. If the device ever stops responding, unplugging it and plugging it back in always
  restores it, because the device boots into normal audio mode after every power cycle.
- **Rollback:** **Revert to factory** in the tool (or `mtrack-fix revert`) erases the upgrade image.
  The device is then exactly as it left the factory.
- **Other units:** the fix was built from and tested on one unit with firmware `1.02`. The tool checks
  the live USB description before it does anything and tells you whether your unit has the bug. If
  your unit reports a different firmware version, the image in this project might not match it. Ask
  before flashing; the factory-image fallback still protects you, but there is no point in writing an
  image that the loader will reject.
- **Warranty:** this modifies the device's firmware. Whatever your warranty says about that applies.

## Does it matter on Linux?

No. Linux worked before and works after. The only visible change on Linux is that `lsusb -v` now
shows `bSourceID 1` for Output Terminal 22. There is a separate, unrelated Linux caveat about the
device's single shared clock, covered in [technical/linux.md](technical/linux.md).
