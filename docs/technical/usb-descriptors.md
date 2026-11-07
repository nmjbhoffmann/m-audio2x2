# USB descriptors

What the M-Track 2X2 tells the host, and the three places where it is wrong. Raw material:
[reference/descriptors.hex](reference/descriptors.hex) (device + both configuration descriptors as
read from Linux sysfs, 506 bytes), [reference/lsusb-v.txt](reference/lsusb-v.txt) (decoded by
`lsusb -v`), [reference/windows-descriptors.txt](reference/windows-descriptors.txt) (the same bytes
read on Windows through the hub, plus all string descriptors). The bytes Windows and Linux read are
identical.

## Device descriptor

| Field | Value |
|---|---|
| bcdUSB | 2.00 |
| Class / SubClass / Protocol | `EF / 02 / 01` (Miscellaneous, Interface Association) |
| bMaxPacketSize0 | 64 |
| idVendor : idProduct | `0763 : 400b` |
| bcdDevice | `1.02` |
| iManufacturer / iProduct / iSerial | 1 ("M-Audio") / 3 ("M-Track 2X2") / 0 |
| bNumConfigurations | 2 (see Anomaly 3) |

## Interface layout (configuration 1, 244 bytes)

| If | Alt | Class | Purpose | Endpoints |
|---|---|---|---|---|
| 0 | 0 | Audio / Control (UAC2, protocol 0x20) | Topology and clock control | none (no interrupt endpoint) |
| 1 | 0 | Audio / Streaming | Playback, idle (zero bandwidth) | none |
| 1 | 1 | Audio / Streaming | **Playback** | `0x01` ISO OUT async, 200 B; `0x81` ISO IN feedback, 4 B |
| 2 | 0 | Audio / Streaming | Capture, idle | none |
| 2 | 1 | Audio / Streaming | **Capture** | `0x82` ISO IN async, 200 B |
| 3 | 0 | DFU 1.1 (class `FE/01`, protocol 1) | Firmware update | none |

Interfaces 0 to 2 sit under one Interface Association Descriptor: one audio function. Interface 3
is a separate function, which is why Windows enumerates the device as a composite device with two
children (`MI_00` audio, `MI_03` DFU).

Format on both streaming interfaces: 2 channels, 32-bit slots with 24 valid bits (`S32_LE`),
rates 44.1, 48, 88.2, 96, 176.4 and 192 kHz.

## Audio topology (as declared)

```
                     Clock Source 41  (internal, frequency R/W, validity R)
                            |
                     Clock Selector 40  (one input: 41)
                            |
          clocks every terminal below (bCSourceID = 40)

PLAYBACK   IT 2 (USB streaming) ------------------> OT 20 (Speaker)          correct
CAPTURE    IT 1 (Microphone)    --> nothing                                  orphan
           IT 2 (USB streaming) ------------------> OT 22 (USB streaming)    wrong source
```

Entity IDs (IT 1, IT 2, OT 20, OT 22, Clock Selector 40, Clock Source 41) are exactly the numbering
XMOS's USB Audio reference firmware uses. There are no Feature, Mixer or Selector units, so there
are no software volume, mute or gain controls. The only control a host can read is clock validity.

String descriptors worth knowing: 8 "M-Audio Clock Selector", 9 "M-Audio Internal Clock",
10 "M-Track 2X2 DFU", 11 to 14 "M-Track Channel 1/2" (channel names of IT 2 and IT 1), and
15 "iAP Interface", which nothing references. String 15 is a leftover from the XMOS reference
design's Apple accessory option and confirms the firmware's origin.

## Anomaly 1: the capture path is wired to the wrong source (the bug)

Output Terminal 22 is the capture stream to the host. Its descriptor is

```
0c 24 03 16 01 01 00 02 28 00 00 07
|  |  |  |  |     |  |  |  |     |
|  |  |  |  |     |  |  |  |     iTerminal (7)
|  |  |  |  |     |  |  |  bmControls (0)
|  |  |  |  |     |  |  bCSourceID = 40 (0x28)
|  |  |  |  |     |  bSourceID = 2        <-- should be 1
|  |  |  |  |     bAssocTerminal (0)
|  |  |  |  wTerminalType 0x0101 (USB streaming)
|  |  |  bTerminalID 22 (0x16)
|  |  OUTPUT_TERMINAL
|  CS_INTERFACE
bLength 12
```

`bSourceID 2` is Input Terminal 2, the **playback** stream from the host. It should be 1, the
Microphone input terminal. As a result IT 1 is referenced by nothing, and read literally the
descriptors say the capture stream is a loopback of the playback stream.

Where the byte is: offset `0x68` of each 244-byte configuration descriptor (OT 22 starts at
`0x61`), i.e. offsets `0x7a` and `0x16e` in `descriptors.hex` (which starts with the 18-byte
device descriptor).

Impact:

- **Linux:** none. `snd-usb-audio` builds PCM streams from the streaming interfaces and does not
  need the terminal graph to be right. Capture works.
- **Windows (in-box `usbaudio2.sys`):** it builds a kernel-streaming topology from the terminal
  graph. The capture pin exists but the only thing upstream of it is the generic USB-streaming pin,
  and IT 1 gets no pin at all. With no physical input, Windows creates no Recording endpoint. Details
  in [windows.md](windows.md).

Most likely origin: in the XMOS reference design, OT 22 takes its input from a Feature Unit fed by
IT 1. The Feature Units were removed for this product and the source ID was not re-pointed.

The fix changes this one byte in the firmware, see [firmware-format.md](firmware-format.md). After
the fix, `lsusb -v` shows `bSourceID 1` for OT 22 and nothing else changes.

## Anomaly 2: `bmChannelConfig = 0`

Neither terminal nor streaming interface declares spatial channel positions. Linux assigns
front-left / front-right by default; Windows labels the channels generically. Cosmetic.

## Anomaly 3: two identical configurations

`bNumConfigurations = 2`, but configuration index 1 is byte-identical to index 0, **including
`bConfigurationValue = 1`**. Two configurations sharing one value is not valid per the USB
specification. The XMOS reference firmware often ships a second configuration as a USB Audio 1.0
fallback for full-speed hosts, so this looks like a stubbed fallback. Linux and Windows both pick
the first configuration; no practical impact has been seen, but a strict host stack or a USB
analyser may complain.

## Endpoint math

| Endpoint | Detail |
|---|---|
| `0x01` playback data | `bInterval 1`: one packet every 125 us microframe. `wMaxPacketSize 200` B = 25 frames of 8 B (2 ch x 4 B). At 192 kHz the nominal packet is 24 frames (192 B), so there is only one frame of headroom for clock adjustment at the top rate. Within spec, but tight. |
| `0x81` feedback | 4 B, `bInterval 4`: every 2^3 = 8 microframes = 1 ms. Linux detects the 16.16 fixed-point format, the correct one for a high-speed UAC2 device. |
| `0x82` capture data | Same sizing as playback. |
| Lock delay | `bLockDelayUnits 2` (decoded PCM samples), `wLockDelay 8`: the device says it needs 8 samples to lock after a rate change. |

Measured on Linux at 48 kHz, the feedback value alternated between 47999 Hz (`0x5.fff8`) and
48000 Hz (`0x6.0000`): a stable asynchronous clock within a few ppm of nominal. On Windows with the
vendor driver, which ignores the feedback endpoint, the device clock measured about 6.8 ppm slow
relative to the host.

## Clock facts

- **Clock Source 41:** frequency read/write (the host sets the sample rate here), validity read-only.
- **Clock Selector 40:** read/write but has one input, so it cannot select anything. The vendor
  driver writes `0` to it (selector inputs are 1-based, so that is invalid); the device accepts it.
- **One clock for both directions.** Playback and capture cannot run at different rates at the same
  time. See [linux.md](linux.md) for why that once caused clicks and what prevents it now.
- The first rate set after enumeration is slow: the vendor driver's `SET CUR SAM_FREQ 44100` took
  911 ms to complete; a later change took 70 ms. The device stalls the status stage while its clock
  locks.

## DFU interface

| Field | Value |
|---|---|
| Interface 3 | class `FE`, subclass `01`, protocol `01` (runtime), string 10 "M-Track 2X2 DFU" |
| DFU functional descriptor | `09 21 07 fa 00 40 00 10 01` |
| bmAttributes `0x07` | CanDownload, CanUpload, Manifestation Tolerant, **Will Not Detach** (the host must bus-reset the device after `DFU_DETACH`) |
| wDetachTimeout | 250 ms |
| wTransferSize | 64 bytes |
| bcdDFUVersion | 1.10 |

In DFU mode the device re-enumerates as `0763:c00b` with a single DFU interface (number 0,
protocol 2) and the same `bcdDevice`. See [dfu.md](dfu.md).
