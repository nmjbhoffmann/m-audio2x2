# Windows

How Windows drives the M-Track 2X2, why its built-in driver creates no microphone, what M-Audio's
driver does instead, and why this project targets the built-in driver. Captured on Windows 11 Pro
24H2 (build 26100, `usbaudio2.sys` 10.0.26100.1150) on 2026-11-03, with the fix verified on
2026-11-04 and 2026-11-07. Raw material: [reference/windows-ks-topology.txt](reference/windows-ks-topology.txt),
[reference/windows-descriptors.txt](reference/windows-descriptors.txt).

## Driver stack with the in-box drivers

```
USB\VID_0763&PID_400B            usbccgp     "USB Composite Device"
  MI_00  (interfaces 0-2, audio)  usbaudio2   "M-Track 2X2"
       SWD\MMDEVAPI\...           Speakers (M-Track 2X2)        <- the only endpoint before the fix
  MI_03  (interface 3, DFU)       (none)      "M-Track 2X2 DFU"  problem code 28
```

- `usbaudio2` binds through the class match `USB\Class_01&SubClass_00&Prot_20`. Stack:
  `ksthunk -> usbaudio2 -> usbccgp`.
- The "M-Track 2X2 DFU" unknown device is interface 3. Windows has no in-box DFU driver, so it shows
  problem code 28. It is harmless and unrelated to the missing microphone. The fix tool binds WinUSB
  to it when flashing; otherwise leave it alone.
- Windows reads exactly the same descriptor bytes as Linux (checked programmatically).

## Why usbaudio2 builds no capture endpoint

`usbaudio2` registers its wave filter for both `KSCATEGORY_RENDER` and `KSCATEGORY_CAPTURE`, so it
does intend to capture. The problem is the topology filter it builds from the UAC2 terminal graph:

```
msft_topo (topology filter)                     msft_wave (wave filter)
  pin 1  IN  bridge, generic (= IT 2, USB) --+--> pin 0 OUT bridge "Speakers" (KSNODETYPE_SPEAKER)
                                             +--> pin 2 OUT bridge, generic --> wave pin 3 -> ADC -> pin 2 (capture stream)
  (no Microphone / Line-in pin: IT 1 was never turned into a pin)
```

The audio endpoint builder creates a Recording endpoint by walking from the capture stream pin
backwards until it reaches a topology bridge pin with a **physical input** category (microphone,
line connector, and so on). Here the only thing upstream of capture is pin 1, the generic USB
streaming input that also feeds Speakers, because OT 22's `bSourceID` is 2. The Microphone terminal
(IT 1) is referenced by nothing, so it gets no pin at all. No physical source, no endpoint. Not even
a disabled or unplugged one appears in MMDevices.

With `bSourceID` corrected to 1, the topology filter gets a `KSNODETYPE_MICROPHONE` bridge pin
feeding the capture path, and Windows creates **Microphone (M-Track 2X2)**. Verified on the fixed
firmware.

Other things that were checked and ruled out: a custom INF for `usbaudio2.sys` (there is no
documented descriptor-override mechanism), and a USB filter driver that rewrites the byte in
descriptor responses (would work, but needs a signed kernel driver; a one-byte firmware change is
simpler and also helps on other hosts).

## The M-Audio driver (package 1.0.13, 2021)

The last package M-Audio published. Facts, from the installer contents and a USB trace:

| | |
|---|---|
| Installer | `InstallerLauncher.exe` 1.0.13.17066, 2021-03-11, inMusic Brands. Four MSIs (x86/x64 x legacy/Win10) differing only in the signed `.sys`/`.cat`, plus VC2005 runtimes |
| Kernel driver | `MAudioMTrack2X2.sys` 7.0.0.3512, x64, attestation-signed (WHQL publisher) |
| Driver model | **AVStream** minidriver (`ks.sys`, `USBD.SYS`; no `portcls`). Parses the configuration descriptor itself and ignores the UAC2 terminal graph, which is why capture works with it |
| Binding | `USB\VID_0763&PID_400B`, the **whole device**, replacing `usbccgp`. The DFU child disappears. Service `MTRACK2X2` |
| Endpoints | Hard-coded names: "Speakers" and "Line 1/2" (names exist for up to 16 channels; a shared inMusic code base with the M-Track Solo) |
| User mode | ASIO 1.0.8 (32/64-bit), a control panel with buffer size and sample rate only, an `AudioDevMon` service, a `DriverReplacer` that forces the INF onto the device |
| Firmware | None shipped. `AudioDevMon` has generic DFU hooks but no image or updater |
| Vendor USB requests | **None.** The trace shows only standard and UAC2 class requests: `GET_DESCRIPTOR`, `SET_CONFIGURATION`, `SET_INTERFACE`, `SET CUR` on the clock source (44100, then 48000) and on the clock selector (value 0, invalid but accepted). No GET requests: the driver has the device's capabilities hard-coded |
| Feedback | **Implicit.** Zero traffic on the explicit feedback endpoint `0x81`. Playback and capture run continuously from enumeration on (1000 URBs/s each) and the OUT packet sizes copy the IN packet sizes about 0.9 ms later. The capture stream paces playback |
| Verified | Installed on 24H2: "Speakers" and "Line 1/2" endpoints, recording works. Both endpoints came up disabled and had to be enabled under "Show disabled devices" |

The implicit-feedback design side-steps the shared-clock issue (both directions always run at one
rate), at the cost of streaming all the time. Linux uses the explicit feedback endpoint instead and
also works. Both are valid for this hardware.

Leftovers after an uninstall: `C:\Windows\System32\drivers\MAudioMTrack2X2.sys` stays behind
(harmless, not bound), stale "Speakers / Line 1/2 (M-Audio M-Track 2X2)" endpoints remain in the
MMDevices registry marked not present, and the package's `oem*.inf` can remain in the driver store,
in which case Windows re-binds the vendor driver on the next replug. The tool checks and removes all
of these.

## Decision: target the in-box driver

The end state this project aims for on Windows is **`usbccgp` + `usbaudio2`**, Windows' own USB
Audio 2.0 class driver, with the firmware fix making it create the microphone endpoint. Not the
M-Audio 1.0.13 driver. Reasons:

- The vendor driver is from 2021 and will not be updated. The class driver ships with Windows and is
  maintained.
- The vendor driver takes over the whole device, including the DFU interface, and keeps both audio
  streams running whenever the device is plugged in.
- With the class driver the device behaves like any other UAC2 interface: shared-mode audio, WASAPI
  exclusive mode, and ASIO through a wrapper such as FlexASIO or ASIO4ALL. Microsoft has been working
  on an improved in-box USB Audio 2.0 driver with low-latency and ASIO support; check its status if
  you need that.
- The fix lives in the device. Once applied, the unit works on any Windows machine with no software at
  all.

If you prefer the vendor driver's ASIO and control panel, it still works on the fixed firmware. The
descriptor change does not affect it (it never read that byte). But the tool requires it to be
removed while flashing, because it hides the DFU interface.

## Things not measured

ASIO stability and latency with the vendor driver at small buffers; a rate change from the vendor
control panel while capturing; what firmware version the vendor control panel reports.
