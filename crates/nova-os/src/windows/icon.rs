//! An executable's icon as a 32px PNG.
//!
//! The icon resource is extracted at exactly the size the UI wants (no
//! rescaling, so no blur), drawn out of its `HICON` as 32-bit BGRA, and
//! encoded with [`super::png`]. Every step may fail; the caller falls back to
//! the generic glyph, exactly as on macOS.

use std::mem::size_of;

use windows_sys::Win32::Graphics::Gdi::{
    CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS, HBITMAP,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, PrivateExtractIconsW, HICON, ICONINFO};

use super::{png, wide};

/// PNG bytes for the first icon in `exe`, at `px`×`px`.
pub fn png_for(exe: &str, px: u32) -> Option<Vec<u8>> {
    let path = wide(exe);
    let mut icon: HICON = std::ptr::null_mut();
    let mut id = 0u32;
    // SAFETY: one HICON slot, one id slot; the icon is destroyed below.
    let n = unsafe {
        PrivateExtractIconsW(path.as_ptr(), 0, px as i32, px as i32, &mut icon, &mut id, 1, 0)
    };
    // 0 = no icons in the file; 0xFFFFFFFF = file not found or not a PE.
    if n == 0 || n == u32::MAX || icon.is_null() {
        return None;
    }
    let rgba = unsafe { rgba_of(icon, px) };
    unsafe { DestroyIcon(icon) };
    rgba.map(|px_data| png::encode_rgba(px, px, &px_data))
}

/// Read an icon's pixels as straight RGBA.
///
/// # Safety
/// `icon` must be a live icon handle of `px`×`px`.
unsafe fn rgba_of(icon: HICON, px: u32) -> Option<Vec<u8>> {
    let mut info: ICONINFO = std::mem::zeroed();
    if GetIconInfo(icon, &mut info) == 0 {
        return None;
    }
    let dc = CreateCompatibleDC(std::ptr::null_mut());
    let color = if info.hbmColor.is_null() { None } else { bits(dc, info.hbmColor, px) };
    let mask = bits(dc, info.hbmMask, px);
    DeleteDC(dc);
    if !info.hbmColor.is_null() {
        DeleteObject(info.hbmColor);
    }
    DeleteObject(info.hbmMask);

    // Monochrome icons (no colour bitmap) are vanishingly rare in apps that
    // make network requests; the glyph is a fine answer for them.
    let mut px_data = color?;
    // Old-style icons carry no alpha channel and rely on the AND mask for
    // transparency: a white mask pixel is transparent.
    let has_alpha = px_data.chunks(4).any(|p| p[3] != 0);
    for (i, p) in px_data.chunks_mut(4).enumerate() {
        p.swap(0, 2); // BGRA → RGBA
        if !has_alpha {
            let transparent = mask.as_ref().is_some_and(|m| m[i * 4] != 0);
            p[3] = if transparent { 0 } else { 255 };
        }
    }
    Some(px_data)
}

/// A bitmap's pixels as top-down 32-bit BGRA.
unsafe fn bits(dc: windows_sys::Win32::Graphics::Gdi::HDC, bmp: HBITMAP, px: u32) -> Option<Vec<u8>> {
    let mut bmi: BITMAPINFO = std::mem::zeroed();
    bmi.bmiHeader = BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: px as i32,
        // Negative: top-down rows, which is the order PNG wants.
        biHeight: -(px as i32),
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB,
        ..std::mem::zeroed()
    };
    let mut out = vec![0u8; (px * px * 4) as usize];
    let lines = GetDIBits(dc, bmp, 0, px, out.as_mut_ptr().cast(), &mut bmi, DIB_RGB_COLORS);
    (lines == px as i32).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_system_executable_yields_a_png_of_the_requested_size() {
        let explorer = r"C:\Windows\explorer.exe";
        let png = png_for(explorer, 32).expect("explorer.exe has an icon");
        assert_eq!(&png[..4], &[0x89, b'P', b'N', b'G']);
        assert_eq!(u32::from_be_bytes(png[16..20].try_into().unwrap()), 32, "width");
    }

    #[test]
    fn a_missing_file_yields_nothing() {
        assert!(png_for(r"C:\no\such\novaproxy.exe", 32).is_none());
    }
}
