//! The smallest correct PNG encoder: 8-bit RGBA, stored (uncompressed) deflate.
//!
//! The app icons it encodes are 32×32 — 4 KB of pixels — so compression would
//! save a few kilobytes of a `data:` URL that is cached per app. Not worth a
//! dependency; same reasoning as the hand-written base64 in `appicon`.

/// Encode `rgba` (row-major, `width * height * 4` bytes) as a PNG file.
pub fn encode_rgba(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    assert_eq!(rgba.len(), (width * height * 4) as usize, "pixel buffer size");

    // Scanlines, each prefixed with filter type 0 (None).
    let stride = width as usize * 4;
    let mut raw = Vec::with_capacity((stride + 1) * height as usize);
    for row in rgba.chunks(stride) {
        raw.push(0);
        raw.extend_from_slice(row);
    }

    let mut out = Vec::with_capacity(raw.len() + 128);
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    // Bit depth 8, colour type 6 (RGBA), deflate, adaptive filtering, no interlace.
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &zlib_stored(&raw));
    chunk(&mut out, b"IEND", &[]);
    out
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// A zlib stream of stored deflate blocks (at most 65535 bytes each).
fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = if data.is_empty() { vec![&[][..]] } else { data.chunks(65535).collect() };
    let last = blocks.len() - 1;
    for (i, block) in blocks.iter().enumerate() {
        out.push(u8::from(i == last));
        let len = block.len() as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(block);
    }
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &x in data {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksums_match_their_reference_values() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(adler32(b"Wikipedia"), 0x11e6_0398);
    }

    #[test]
    fn a_one_pixel_image_has_the_png_structure() {
        let png = encode_rgba(1, 1, &[255, 0, 0, 255]);
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(&png[png.len() - 8..png.len() - 4], b"IEND");
        // IHDR's CRC covers its type and data.
        let ihdr_crc = u32::from_be_bytes(png[29..33].try_into().unwrap());
        assert_eq!(ihdr_crc, crc32(&png[12..29]));
    }

    #[test]
    fn the_pixels_survive_inside_the_stored_block() {
        let px: Vec<u8> = (0..32 * 32 * 4).map(|i| (i % 251) as u8).collect();
        let png = encode_rgba(32, 32, &px);
        // zlib header, then one final stored block of 32 * (1 + 128) bytes.
        let idat = png.windows(4).position(|w| w == b"IDAT").unwrap() + 4;
        assert_eq!(&png[idat..idat + 3], &[0x78, 0x01, 0x01]);
        let len = u16::from_le_bytes([png[idat + 3], png[idat + 4]]) as usize;
        assert_eq!(len, 32 * 129);
        let body = &png[idat + 7..idat + 7 + len];
        assert_eq!(body[0], 0, "filter byte");
        assert_eq!(&body[1..129], &px[..128]);
    }
}
