//! XMOS xflash boot-image handling: parse, verify the loader CRCs, bit-reverse.
//!
//! Header (13 little-endian words):
//!   0 tag 0x0FF51DE  1 pageCRC  2 imageCRC  3 features  4 size  5 version  6 length
//!   7 numCores  8 bootImgAddr  9 overlayAddr  10 overlayOff  11 switchSetupOff  12 coreEntryOff
//!
//! CRC rule (reverse-engineered from the stage-1 loader, verified on the factory image):
//!   standard CRC-32 (zlib) over one zero word followed by the covered words.
//!   pageCRC  covers words 3..63 (the rest of the first 256-byte page).
//!   imageCRC covers words 3..length/4-1.
//!
//! The flash chip stores every byte bit-reversed (the XS1 boot ROM clocks SPI LSB-first).
//! DFU_DNLOAD writes the file verbatim, so the bytes that go over DFU are the "chip order"
//! (bit-reversed) image. This module keeps both orders.

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};

pub const TAG: u32 = 0x00FF_51DE;

/// The signed, patched upgrade image (version 1, OT22 bSourceID 2 -> 1), logical byte order.
pub const BUILTIN_IMAGE: &[u8] = include_bytes!("../firmware/mtrack2x2-upgrade-v1-signed.bin");
pub const BUILTIN_NAME: &str = "mtrack2x2-upgrade-v1-signed.bin (built in)";

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct ImageInfo {
    pub version: u32,
    pub length: u32,
    pub features: u32,
    pub cores: u32,
    pub page_crc_ok: bool,
    pub image_crc_ok: bool,
}

#[derive(Clone)]
#[allow(dead_code)]
pub struct Firmware {
    pub name: String,
    /// Logical byte order (what xflash_image.py works on).
    pub logical: Vec<u8>,
    /// Bit-reversed per byte: what DFU_DNLOAD must send.
    pub chip_order: Vec<u8>,
    pub info: ImageInfo,
    pub sha256_logical: String,
}

pub fn bitrev(data: &[u8]) -> Vec<u8> {
    data.iter().map(|b| b.reverse_bits()).collect()
}

fn word(img: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([img[i * 4], img[i * 4 + 1], img[i * 4 + 2], img[i * 4 + 3]])
}

/// CRC of a byte range as the XMOS loader computes it (one zero word prepended).
fn loader_crc(bytes: &[u8]) -> u32 {
    let mut h = crc32fast::Hasher::new();
    h.update(&[0, 0, 0, 0]);
    h.update(bytes);
    h.finalize()
}

pub fn parse(img: &[u8]) -> Result<ImageInfo> {
    if img.len() < 256 {
        bail!("image is too short ({} bytes)", img.len());
    }
    if word(img, 0) != TAG {
        bail!("bad image tag {:#x} (expected {:#x})", word(img, 0), TAG);
    }
    let length = word(img, 6);
    if length as usize > img.len() || !length.is_multiple_of(4) || length < 256 {
        bail!(
            "image header length {} does not fit the file ({} bytes)",
            length,
            img.len()
        );
    }
    let page_crc = loader_crc(&img[12..256]);
    let image_crc = loader_crc(&img[12..length as usize]);
    Ok(ImageInfo {
        version: word(img, 5),
        length,
        features: word(img, 3),
        cores: word(img, 7),
        page_crc_ok: page_crc == word(img, 1),
        image_crc_ok: image_crc == word(img, 2),
    })
}

/// Load an image file in either byte order; the tag tells which one it is.
pub fn load(bytes: &[u8], name: &str) -> Result<Firmware> {
    if bytes.len() < 256 {
        bail!("{name}: too short to be an xflash image");
    }
    let (logical, chip_order) = if word(bytes, 0) == TAG {
        (bytes.to_vec(), bitrev(bytes))
    } else {
        let rev = bitrev(bytes);
        if word(&rev, 0) != TAG {
            bail!(
                "{name}: not an xflash boot image (tag {:#x} not found in either bit order)",
                TAG
            );
        }
        (rev, bytes.to_vec())
    };
    let info = parse(&logical).with_context(|| name.to_string())?;
    if !(info.page_crc_ok && info.image_crc_ok) {
        bail!(
            "{name}: CRC mismatch (page {}, image {}); the loader would reject it and boot the factory image",
            if info.page_crc_ok { "ok" } else { "BAD" },
            if info.image_crc_ok { "ok" } else { "BAD" }
        );
    }
    if info.version == 0 {
        bail!("{name}: version 0 is the factory image's version; an upgrade image must be > 0");
    }
    let sha256_logical = hex(&Sha256::digest(&logical));
    Ok(Firmware {
        name: name.to_string(),
        logical,
        chip_order,
        info,
        sha256_logical,
    })
}

pub fn builtin() -> Firmware {
    load(BUILTIN_IMAGE, BUILTIN_NAME).expect("built-in image is valid")
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_verifies() {
        let fw = builtin();
        assert_eq!(fw.info.version, 1);
        assert_eq!(fw.info.length, 22440);
        assert!(fw.info.page_crc_ok && fw.info.image_crc_ok);
        assert_eq!(
            fw.sha256_logical,
            "06bfed4f8ce3239127aafccf2750951f4a9b720ffa24b2a88f3a5cc6c781f0f5"
        );
        // chip order is what went over DFU in the acceptance run
        assert_eq!(
            hex(&Sha256::digest(&fw.chip_order)),
            "f955164a8b008c7bde666a5200884740d56011ec477e5ffadcab1161febc29b7"
        );
    }

    #[test]
    fn load_accepts_chip_order() {
        let fw = load(&bitrev(BUILTIN_IMAGE), "rev").unwrap();
        assert_eq!(fw.logical, BUILTIN_IMAGE);
    }

    #[test]
    fn corrupt_crc_rejected() {
        let mut img = BUILTIN_IMAGE.to_vec();
        img[0x5545] ^= 0xff;
        assert!(load(&img, "bad").is_err());
    }
}
