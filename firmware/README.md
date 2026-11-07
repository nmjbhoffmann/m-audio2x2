# Firmware images

Both files are XMOS `xflash` boot images for the M-Track 2X2's XS1-U8A controller, in **logical
byte order** (the order a normal file reader expects). The flash chip and the DFU interface use
bit-reversed bytes; see [docs/technical/firmware-format.md](../docs/technical/firmware-format.md).

| File | What it is | Version | Size | SHA-256 |
|---|---|---|---|---|
| `factory-image-v0.bin` | The unmodified factory image as read from the flash chip of the author's unit (device `bcdDevice 1.02`). Reference only. | 0 | 22440 bytes | `1fbed330ff3640f9d27165ba8a08f7338bb68dba6dff6f8946e77c519e9f8ed1` |
| `mtrack2x2-upgrade-v1-signed.bin` | **The fix.** Identical to the factory image except one descriptor byte (image offset `0x5545`, `0x02` to `0x01`), the version word (`0` to `1`) and the two recomputed CRC words. | 1 | 22440 bytes | `06bfed4f8ce3239127aafccf2750951f4a9b720ffa24b2a88f3a5cc6c781f0f5` |

## Which file to use

The tool has `mtrack2x2-upgrade-v1-signed.bin` **built in**; you do not need to select a file.
The same file is here (and in `tool/firmware/`, where the build picks it up) so you can verify the
checksum or pass it to the tool explicitly:

```
mtrack-fix verify firmware/mtrack2x2-upgrade-v1-signed.bin
mtrack-fix flash  firmware/mtrack2x2-upgrade-v1-signed.bin
```

The tool accepts the image in either bit order (it detects the order from the header) and
bit-reverses it before sending it over DFU.

Do not flash `factory-image-v0.bin`. It is version 0, and the loader only boots an upgrade image
whose version is higher than the factory image's. The tool refuses images with version 0.

## Version numbers

- The device reports USB `bcdDevice 1.02` in both states; that number lives in the firmware code,
  not in the image header, and the fix does not change it.
- The image header carries its own version word: factory `0`, fix `1`. The boot loader picks the
  upgrade image when it is present, has a valid CRC and a higher version than the factory image.
- Any future image in this project will use version 2, 3, and so on.

## Undoing it

`mtrack-fix revert` (or **Revert to factory** in the GUI) sends the XMOS "revert factory" DFU
request, which erases the upgrade image. The device then boots the factory firmware again, exactly
as before. The factory image is never written by the tool; DFU on this device cannot touch it.

## Legal

The files in this folder (and the copy embedded in `tool/firmware/`) are derived from the M-Track
2X2's own factory firmware, which is copyright M-Audio / inMusic Brands. They are not licensed under
the MIT license that covers the rest of this repository, and no rights to them are granted here. They
are provided solely for interoperability and for the repair of your own device, as-is, with no
warranty.

The patched image differs from the factory image by one descriptor byte, the version word and the
two CRC words, nothing else. Anyone can read the factory image from their own unit and apply the
one-byte change themselves; [docs/technical/firmware-format.md](../docs/technical/firmware-format.md)
describes the format, the CRC rule and the patch precisely enough to do that. The factory image stays
on the device untouched, so the change is reversible at any time with the tool's revert.
