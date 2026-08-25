//! The icon of the app a flow came from.
//!
//! The flow table names the originating process (`Flow::process`); this turns
//! that name into the picture a person actually recognises. It is a *nicety* by
//! construction: every step is allowed to fail and the caller falls back to a
//! generic glyph, so an unusual bundle layout costs a picture and nothing else.
//!
//! Deliberately no new dependency. macOS ships `sips`, which reads `.icns` and
//! writes PNG, and the bundle layout is stable enough to find the icon by
//! looking rather than by parsing `Info.plist` — a binary plist parser is a lot
//! of surface to add for a 32px image. The trade is stated so the next reader
//! knows it was a choice: an app whose icon lives somewhere unusual gets the
//! glyph.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Size of the PNG we hand the UI, in points. The table draws it at 13px, so a
/// 32px raster covers a 2× display without being a needless payload.
const PX: u32 = 32;

/// A `data:` URL for the app's icon, ready for an `<img src>`.
///
/// `bundle` is a path ending in `.app` — the outermost bundle, which is what
/// `procinfo` already rolls helper processes up to.
pub fn icon_data_url(bundle: &str) -> Option<String> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let icns = find_icns(Path::new(bundle))?;
    let png = to_png(&icns)?;
    Some(format!("data:image/png;base64,{}", b64(&png)))
}

/// The most promising `.icns` in a bundle's `Resources`.
///
/// Biggest file wins when there are several: apps ship document-type icons
/// beside the app icon, and the app icon is nearly always the largest — a
/// heuristic, but one that fails to *another icon of the same app*, which is a
/// far better failure than parsing the wrong plist key.
fn find_icns(bundle: &Path) -> Option<PathBuf> {
    let dir = bundle.join("Contents/Resources");
    let mut best: Option<(u64, PathBuf)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("icns") {
            continue;
        }
        let size = entry.metadata().ok()?.len();
        if best.as_ref().is_none_or(|(b, _)| size > *b) {
            best = Some((size, path));
        }
    }
    best.map(|(_, p)| p)
}

/// `sips` writes to a file, not to stdout, so this goes through a temp path.
fn to_png(icns: &Path) -> Option<Vec<u8>> {
    let out = std::env::temp_dir().join(format!("novaproxy-icon-{}.png", std::process::id()));
    let status = Command::new("sips")
        .arg("-s")
        .arg("format")
        .arg("png")
        .arg("-Z")
        .arg(PX.to_string())
        .arg(icns)
        .arg("--out")
        .arg(&out)
        .output()
        .ok()?;
    if !status.status.success() {
        return None;
    }
    let bytes = std::fs::read(&out).ok();
    // Best-effort: a leftover temp file is not worth failing the icon over.
    let _ = std::fs::remove_file(&out);
    bytes
}

/// Base64, written out rather than pulled in: this is the only caller in the
/// workspace and the alphabet is not going to change.
fn b64(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((bytes.len() + 2) / 3 * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(A[(n >> 18) as usize & 63] as char);
        out.push(A[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { A[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { A[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_rfc_examples() {
        assert_eq!(b64(b""), "");
        assert_eq!(b64(b"f"), "Zg==");
        assert_eq!(b64(b"fo"), "Zm8=");
        assert_eq!(b64(b"foo"), "Zm9v");
        assert_eq!(b64(b"foob"), "Zm9vYg==");
        assert_eq!(b64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn base64_handles_bytes_that_are_not_text() {
        // PNG magic — the actual payload, and the case a hand-written encoder
        // gets wrong by assuming ASCII.
        assert_eq!(b64(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]), "iVBORw0KGgo=");
    }

    #[test]
    fn a_bundle_with_no_resources_yields_nothing_rather_than_failing() {
        let missing = std::env::temp_dir().join("novaproxy-no-such.app");
        assert!(find_icns(&missing).is_none());
    }

    #[test]
    fn the_largest_icns_wins() {
        let dir = std::env::temp_dir().join(format!("novaproxy-icontest-{}.app", std::process::id()));
        let res = dir.join("Contents/Resources");
        std::fs::create_dir_all(&res).unwrap();
        std::fs::write(res.join("doc.icns"), vec![0u8; 10]).unwrap();
        std::fs::write(res.join("app.icns"), vec![0u8; 100]).unwrap();
        std::fs::write(res.join("readme.txt"), vec![0u8; 1000]).unwrap();
        assert_eq!(find_icns(&dir).unwrap().file_name().unwrap(), "app.icns");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
