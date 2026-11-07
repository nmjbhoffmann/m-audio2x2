# Linux

The M-Track 2X2 works on Linux without this project's fix. This page records how the driver stack
sees it, the one caveat that explains the clicks people reported on older systems, a recommended
PipeWire configuration, and a summary of the tests. Measured on Fedora (kernel 7.1.3, PipeWire 1.6.9,
WirePlumber 0.5.18) on 2026-11-03.

## Driver stack

| Layer | What |
|---|---|
| Kernel driver | `snd-usb-audio`, generic UAC2 path. No quirk entry exists for `0763:400b` and none is needed. `quirks: 0x0`. |
| What it builds | Card `M2X2`. Playback: interface 1 alt 1, EP `0x01` async with explicit feedback on `0x81`. Capture: interface 2 alt 1, EP `0x82` async. Format `S32_LE` (24 valid bits), 2 channels, 44.1 to 192 kHz. One mixer control: "M-Audio Internal Clock Validity" (read-only). |
| Interface 3 (DFU) | Left unclaimed. The fix tool opens it directly through `/dev/bus/usb`. |
| Power management | Runtime autosuspend is disabled while `snd-usb-audio` is bound. |
| Sound server | PipeWire. Card `alsa_card.usb-M-Audio_M-Track_2X2-00`, profile "Analog Stereo Duplex", sink `alsa_output.usb-M-Audio_M-Track_2X2-00.analog-stereo`, source `alsa_input.usb-M-Audio_M-Track_2X2-00.analog-stereo`. The IEC958 (S/PDIF) profiles PipeWire also offers are bogus; the device has no digital I/O. |

The descriptor bug does not matter here: `snd-usb-audio` builds its PCM streams from the streaming
interfaces and does not require the terminal graph to be right. Explicit feedback is detected and
used; no implicit-feedback quirk is needed.

## The shared-clock caveat

The device has **one clock** (Clock Source 41) for playback and capture. It cannot play at 48 kHz
while recording at 44.1 kHz.

What this caused on older systems (reported on Linux Mint 21, kernel 5.15, PulseAudio): PulseAudio
opened the source and the sink as independent ALSA devices and switched an idle sink's rate to suit a
new stream. When it retuned the sink (say 44.1 to 48 kHz) while the source was open, the older kernel
driver simply reprogrammed the shared clock. The capture stream then ran at the wrong rate, the
feedback loop and buffer accounting drifted, and the result was clicks, underruns and timing errors.
Two applications at different rates was exactly the situation that triggered it.

What prevents it now:

- **Kernel:** newer `snd-usb-audio` tracks endpoints that share a clock and constrains a second
  opener to the rate already in use instead of retuning under it (believed to have arrived around
  kernel 6.1; verified here: with capture at 48 kHz, a 96 kHz playback request was held at 48 kHz by
  the kernel and PipeWire resampled, with zero errors).
- **Sound server:** PipeWire runs the graph at one rate and resamples everything else. With
  `clock.allowed-rates = [ 48000 ]` (the default on this system) the device never changes rate at all.

## Recommended configuration

Keep a single fixed graph rate. Create `~/.config/pipewire/pipewire.conf.d/10-rates.conf`:

```
context.properties = {
    default.clock.rate          = 48000
    default.clock.allowed-rates = [ 48000 ]
}
```

With a fixed rate the device never has to re-lock, and every stream gets high-quality resampling. If
you want bit-exact 44.1 kHz playback for music, `[ 44100 48000 ]` is safe on a current kernel; expect
a brief mute or a possible pop at each switch (the device needs 8 samples plus a stream restart to
lock).

Low latency for a DAW: tested clean down to a 32-sample quantum (0.67 ms).

```
pw-metadata -n settings 0 clock.force-quantum 64    # 1.3 ms at 48 kHz
pw-metadata -n settings 0 clock.force-quantum 0     # back to default
```

Per application: `PIPEWIRE_QUANTUM=64/48000 <app>`, or `pw-jack <app>` for JACK applications. The
"Pro Audio" card profile exposes the raw device without channel-map guesswork.

Mono microphone sources: PipeWire presents the input as one stereo source, so a single microphone on
input 1 lands in the left channel only. A `libpipewire-module-loopback` instance per input with
`audio.position = [ FL ]` (or `[ FR ]`), `stream.dont-remix = true` and a playback side of
`media.class = "Audio/Source"`, `audio.position = [ MONO ]` gives proper mono sources that
applications such as Discord or OBS can pick.

Power: the device is bus powered, declares 500 mA and generates +48 V phantom power from the bus.
Plug it straight into a rear port. Avoid unpowered hubs and front-panel ports if you see crackle with
phantom power on or under loud playback.

## Test results (summary)

All tests ran through PipeWire on kernel 7.1.3 with digital silence as the playback signal (the USB
isochronous stream and feedback loop behave the same as with audio). "Errors" is PipeWire's per-node
xrun counter.

| Test | Result |
|---|---|
| Duplex streaming at the default quantum, then at 256, 64 and 32 samples (0.67 ms), 30 to 60 s each | 0 errors at every size. Driver wake-up latency 2 to 33 us against a 667 us period at the smallest size. |
| Sequential rate switches 44.1 to 48 to 96 to 192 to 44.1 kHz, twice; 20 rapid 44.1/48 switches | Device locked to every rate; feedback values within 1 to 2 Hz of nominal; no kernel or PipeWire errors. |
| Three simultaneous streams at 44.1, 48 and 96 kHz | Graph stays at the first rate, the others are resampled, 0 errors. |
| Capture at 48 kHz, then playback asks for 96 kHz | Kernel holds both directions at 48 kHz, PipeWire resamples, 0 errors (the shared-clock protection). |
| Real microphone on input 1, 12 s at 48 kHz | Clean signal, peak -4.7 dBFS, no clipping. Unused input 2 noise floor about -99 dBFS. Negligible DC offset. |
| Kernel log throughout | Empty: no resets, disconnects, babble or ISO errors. |

Not tested: long listening sessions with real audio, phantom power with a condenser microphone while
playing and recording, and audible pops at the moment of a rate switch (silence cannot reveal those).

## Useful commands

```
cat /proc/asound/M2X2/stream0                 # live stream state, momentary feedback rate
cat /proc/asound/M2X2/pcm0p/sub0/hw_params    # actual rate/format/period, playback
cat /proc/asound/M2X2/pcm0c/sub0/hw_params    # same, capture
pw-top                                        # per-node quantum, rate, error counter
wpctl status                                  # sinks, sources, defaults
pw-metadata -n settings                       # graph clock settings
lsusb -v -d 0763:400b                         # descriptors (sudo for strings)
journalctl -k | grep -i usb                   # kernel USB messages
```
