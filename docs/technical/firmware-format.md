# Firmware image format

How the M-Track 2X2's firmware image is laid out, how its checksums are computed, how its program
is compressed, and how the fix was made. Everything here was reverse-engineered from the flash dump
of the author's unit and verified byte-exact (the decompressed image contains the live USB
descriptors, and the recomputed CRCs match the factory values). It is enough to re-implement the
tools from scratch.

The images are `xflash` boot images from the XMOS tools 13/14 era (the XS1 generation). Newer XMOS
tools and the xCORE-200 / xcore.ai generations use a different layout.

## Two byte orders

The XS1 boot ROM shifts SPI data **LSB first**, so `xflash` writes every byte bit-reversed and the
flash chip holds the mirror image of each byte. Three places, two orders:

| Where | Order |
|---|---|
| A dump read with a normal (MSB-first) programmer | chip order (bit-reversed) |
| Bytes sent or received over DFU (the firmware's flash code moves them verbatim) | chip order |
| The images in `firmware/`, the tool's embedded image, and everything in this document | logical order |

Conversion is per byte: reverse the eight bits. The tool detects the order of a given file from the
header tag and converts as needed.

## Image header

13 little-endian 32-bit words at the start of the image:

| Word | Offset | Name | Factory value | Fixed image |
|---|---|---|---|---|
| 0 | `0x00` | tag | `0x0FF51DE` | same |
| 1 | `0x04` | pageCRC | `0x07E7604D` | `0xC333FC74` |
| 2 | `0x08` | imageCRC | `0x29537E35` | `0x029215B5` |
| 3 | `0x0C` | features | `0x40` | same |
| 4 | `0x10` | size | `0x57A8` (22440) | same |
| 5 | `0x14` | version | 0 | **1** |
| 6 | `0x18` | length | `0x57A8` (22440) | same |
| 7 | `0x1C` | numCores | 1 | same |
| 8 | `0x20` | bootImgAddr | 0 | same |
| 9 | `0x24` | overlayAddr | 0 | same |
| 10 | `0x28` | overlayOff | 0 | same |
| 11 | `0x2C` | switchSetupOff | `0x34` | same |
| 12 | `0x30` | coreEntryOff | `0x3C` | same |

`size` and `length` are both the total image length in bytes, header included. The loader boots the
upgrade image when its tag matches, both CRCs verify and its `version` is greater than the factory
image's. Equal or lower versions are ignored.

After the header come per-core entry data, then the compressed program stream, then the
decompressor stub (see below).

## The CRC rule

Both CRC fields are standard **CRC-32** (reflected polynomial `0xEDB88320`, initial value
`0xFFFFFFFF`, final inversion; exactly `zlib.crc32` or `crc32fast`) computed over **one zero word
followed by the covered words**, all little-endian:

```
crc(words) = crc32( b"\x00\x00\x00\x00" + pack("<%dI" % len(words), *words) )

pageCRC  = crc(words[3 : 64])             # the rest of the first 256-byte page
imageCRC = crc(words[3 : length // 4])    # everything after the two CRC words, to the end
```

Word indices are into the image seen as an array of 32-bit little-endian words; word 3 is
`features`. The leading zero word is the quirk: the XS1 loader's CRC routine starts from a register
state equal to CRC-32 after one zero word (`0xDEBB20E3`) rather than from `0xFFFFFFFF`. Feeding a
zero word to a standard CRC-32 produces the same state.

The first-stage loader at the start of the flash uses the same rule: its CRC word (chip offset
`0x08AC`) is `crc(words[0 : 555])` over the length word plus the 554 loader words.

Two things that do **not** work on this unit, so you do not have to rediscover them:

- The "no CRC check" sentinel `0x0D15AB1E` documented for xflash images. The loader does compare
  against it, but an upgrade image carrying the sentinel in both fields was never booted.
- Any recipe that feeds the stored CRC into the computation and compares the result with a
  constant. The fields are plain CRCs as above.

## The compression

The program inside the image is compressed with an LZ77-style scheme. The image carries its own
decompressor stub in its last 120 bytes (image offsets `0x572A` to `0x57A2`), and the first-stage
loader has a near-identical copy for its own payload. The grammar was recovered by disassembling the
stub (XS1 instruction set).

Stream layout in the factory image:

| Image offset | Content |
|---|---|
| `0x0000` | header and entry data |
| `0x05E6` to `0x572A` | compressed stream |
| `0x572A` to `0x57A2` | decompressor stub (XS1 code) |

The stub decompresses the stream to RAM at `0x10000`, producing 27817 bytes, in place (input is
read from higher addresses than the output is written to). In the decompressed program the USB
device descriptor is at `+0x6554` and the configuration descriptor at `+0x6568`.

### Grammar

Three marker bytes A, B and C are chosen per image (the stub loads them with `ldc` instructions, so
they are part of the stub, not the stream). The M-Track image uses **A = `0xDF`, B = `0xE5`,
C = `0x57`**, apparently the three rarest byte values in the program. Reading the stream byte by
byte:

| Input | Output |
|---|---|
| `b`, where b is not a marker | literal `b` |
| `M 0x00`, where M is a marker | literal `M` (escaped marker) |
| `A p` | copy `(p >> 4) + 3` bytes from distance `(p & 15) * 2` |
| `B p` | copy `(p >> 6) + 3` bytes from distance `((p & 63) + 15) * 2` |
| `C p q` | copy `(p >> 4) + 3` bytes from distance `((((p & 15) << 8) \| q) + 15) * 2` |

"Copy n bytes from distance d" appends, one byte at a time, the byte that is d bytes behind the
current end of the output. Copies may overlap (d < n), the classic LZ77 run. Every distance is even,
and the three token types share the `+15` and `*2` because their code paths fall through into each
other in the stub.

Ranges that follow from the grammar: `A` covers distances 2 to 30 with lengths 3 to 18; `B` covers
distances 30 to 156 with lengths 3 to 6; `C` covers distances 30 to 8220 with lengths 3 to 18. A
token whose second byte would be `0x00` is not representable (it would read as an escaped marker),
so an encoder must emit a literal instead in that case. A greedy encoder with these rules reproduces
a valid stream; the original encoder's exact choices do not matter, only that the decoded bytes match.

The first-stage loader's copy of the stub uses the same grammar with one difference: in its `C`
token the length is `(p >> 3) + 3` and the distance uses `p & 7`.

## Where the bug is in the image

The faulty `bSourceID` of Output Terminal 22 is stored as a **literal** byte in the compressed
stream at image offset **`0x5545`** (chip offset `0x5DF5` for the factory image at `0x08B0`). The
configuration descriptor template starts at image offset `0x54E7`.

Changing a literal in place is safe when no later token copies through it. The pair `02 28` (the old
`bSourceID` followed by `bCSourceID 40`) occurs nowhere after that point in the program, so no
back-reference reads the changed byte, and the output differs in exactly one byte. This was checked
by decompressing the patched image and comparing with the factory program: the only difference is
`+0x65D0` (`0x6568 + 0x68`), the one descriptor byte.

If a future image stores the byte inside a copy token instead, the fix would be to decompress,
patch the program, re-compress with the grammar above and re-sign.

## How the fix was made

Starting from the factory image in logical order (`firmware/factory-image-v0.bin`):

1. Set byte `0x5545` from `0x02` to `0x01`.
2. Set word 5 (`version`) to 1.
3. Recompute word 1 (`pageCRC`) over words 3 to 63 and word 2 (`imageCRC`) over words 3 to
   `length/4 - 1`, with the zero-word rule above.
4. Bit-reverse every byte to get the file that goes over DFU (or into a programmer at `0x7000`).

Result: `firmware/mtrack2x2-upgrade-v1-signed.bin` (logical order), which differs from the factory
image in byte `0x5545`, the version word and the two CRC words only. The tool embeds the logical
image and performs step 4 itself before sending.

To verify an image: check the tag, recompute both CRCs and compare. `mtrack-fix verify <file>` does
this in either byte order.

## Why an upgrade image and not a patched factory image

The DFU interface can only write the upgrade slot, and the loader always keeps the factory image as
a fallback when the upgrade image fails its CRC. Writing the fix as a version-1 upgrade image means
no soldering for users, an automatic rollback on a bad write, and an explicit rollback through the
"revert factory" request. A patched factory image written with a programmer at `0x08B0` would also
work (the CRC rule is the same) but offers none of that safety.
