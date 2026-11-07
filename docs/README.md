# Documentation index

Start with the top-level [README](../README.md) if you only want to apply the fix.

## General

| Document | What it covers |
|---|---|
| [the-problem.md](the-problem.md) | What is wrong with the device, why Windows shows no microphone, what the fix changes, risks and rollback |
| [using-the-tool.md](using-the-tool.md) | Step by step on Windows and Linux, what the prompts mean, troubleshooting |
| [building.md](building.md) | Building the tool from source on Windows, Ubuntu and Fedora |

## Technical

Deep dives for engineers. Each one stands on its own.

| Document | What it covers |
|---|---|
| [technical/hardware.md](technical/hardware.md) | The chips on the board, board photos, reading the flash chip with an SPI programmer, bit reversal, flash layout |
| [technical/usb-descriptors.md](technical/usb-descriptors.md) | Interface layout, audio topology, the three descriptor anomalies, endpoint math, clock facts |
| [technical/firmware-format.md](technical/firmware-format.md) | The xflash image header, the CRC rule, the LZ compression grammar, where the patch byte is, how to patch and re-sign |
| [technical/dfu.md](technical/dfu.md) | How XMOS DFU works on this device, its quirks, Windows specifics, what the tool does step by step |
| [technical/windows.md](technical/windows.md) | The Windows driver stack, why usbaudio2 builds no capture endpoint, facts about the vendor driver, why the in-box driver is the target |
| [technical/linux.md](technical/linux.md) | The Linux driver stack, the shared-clock caveat, recommended PipeWire configuration, summarized test results |
| [technical/reference/](technical/reference/) | Raw captures: `descriptors.hex`, `lsusb-v.txt`, `windows-descriptors.txt`, `windows-ks-topology.txt` |

## Elsewhere in the repository

| Location | What |
|---|---|
| [../firmware/README.md](../firmware/README.md) | The firmware images, checksums, version numbers |
| [../tool/README.md](../tool/README.md) | The tool: features, command line, source layout |
