//! Draws tray icons in memory: a Segoe Fluent Icons glyph plus a level bar (or just the number).
//! Text is rendered with GDI into a grayscale mask, then composited into premultiplied ARGB pixels.

use crate::model::Kind;
use crate::util::from_wide;
use std::ffi::c_void;
use windows::Win32::Foundation::{BOOL, COLORREF, RECT};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, HICON, ICONINFO};

#[derive(Clone, Copy)]
pub struct Style {
    pub light_taskbar: bool,
    pub number_mode: bool,
}

pub struct IconSpec {
    pub kind: Kind,
    pub level: u8,
    pub charging: bool,
    pub connected: bool,
    /// Placeholder icon shown when there's no device yet (keeps the menu reachable).
    pub placeholder: bool,
}

fn glyph(kind: Kind) -> char {
    match kind {
        Kind::Mouse => '\u{E962}',
        Kind::Keyboard => '\u{E765}',
        Kind::Headset => '\u{E7F6}',
        Kind::Gamepad => '\u{E7FC}',
        Kind::Other => '\u{E772}',
    }
}
const BOLT: char = '\u{E945}';
const BATTERY: char = '\u{E83F}';

type Rgb = (u8, u8, u8);

fn colors(style: Style) -> (Rgb, Rgb, Rgb, Rgb) {
    // (foreground, amber, red, green), tuned for contrast on the taskbar.
    if style.light_taskbar {
        ((0x1a, 0x1a, 0x1a), (0xBA, 0x75, 0x17), (0xC4, 0x2B, 0x1C), (0x3B, 0x6D, 0x11))
    } else {
        ((0xff, 0xff, 0xff), (0xEF, 0x9F, 0x27), (0xFF, 0x6B, 0x6B), (0x97, 0xC4, 0x59))
    }
}

pub fn level_color(level: u8, style: Style) -> Rgb {
    let (fg, amber, red, _) = colors(style);
    if level < 15 {
        red
    } else if level <= 30 {
        amber
    } else {
        fg
    }
}

/// Grayscale coverage mask of `text` (0..=255 per pixel), drawn with its ink box centered in `area`
/// and scaled down until it fits.
unsafe fn text_mask(size: i32, text: &str, face: &str, height: i32, weight: i32, area: RECT) -> Vec<u8> {
    let mut h = height;
    loop {
        let (mask, bbox) = draw_text(size, text, face, h, weight, None);
        let Some((x0, y0, x1, y1)) = bbox else { return mask };
        let (bw, bh) = (x1 - x0 + 1, y1 - y0 + 1);
        let (aw, ah) = (area.right - area.left, area.bottom - area.top);
        if (bw > aw || bh > ah) && h > 6 {
            h -= 1;
            continue;
        }
        let dx = area.left + (aw - bw) / 2 - x0;
        let dy = area.top + (ah - bh) / 2 - y0;
        return draw_text(size, text, face, h, weight, Some((dx, dy))).0;
    }
}

type BBox = Option<(i32, i32, i32, i32)>;

unsafe fn draw_text(size: i32, text: &str, face: &str, height: i32, weight: i32, offset: Option<(i32, i32)>) -> (Vec<u8>, BBox) {
    // Draw on a 2x canvas so ink that starts outside the icon can be measured and re-centered.
    let canvas = size * 2;
    let dc = CreateCompatibleDC(HDC::default());
    let mut bmi = BITMAPINFO::default();
    bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = canvas;
    bmi.bmiHeader.biHeight = -canvas;
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    let mut bits: *mut c_void = std::ptr::null_mut();
    let Ok(bmp) = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, None, 0) else {
        let _ = DeleteDC(dc);
        return (vec![0; (size * size) as usize], None);
    };
    let old_bmp = SelectObject(dc, bmp);

    let mut lf = LOGFONTW { lfHeight: -height, lfWeight: weight, lfCharSet: DEFAULT_CHARSET, lfQuality: ANTIALIASED_QUALITY, ..Default::default() };
    crate::util::copy_wide(&mut lf.lfFaceName, face);
    let font = CreateFontIndirectW(&lf);
    let old_font = SelectObject(dc, font);
    SetTextColor(dc, COLORREF(0x00ffffff));
    SetBkMode(dc, TRANSPARENT);

    let (ox, oy) = offset.unwrap_or((size / 2, size / 2));
    let mut rect = RECT { left: ox, top: oy, right: ox + size, bottom: oy + size };
    let mut w: Vec<u16> = text.encode_utf16().collect();
    DrawTextW(dc, &mut w, &mut rect, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_NOCLIP);
    let _ = GdiFlush();

    let px = std::slice::from_raw_parts(bits as *const u32, (canvas * canvas) as usize);
    let mut bbox: BBox = None;
    let mut mask = vec![0u8; (size * size) as usize];
    for y in 0..canvas {
        for x in 0..canvas {
            let v = ((px[(y * canvas + x) as usize] >> 8) & 0xff) as u8;
            if v == 0 {
                continue;
            }
            // Measured in icon coordinates (canvas offset by size/2 when measuring).
            let (ix, iy) = if offset.is_some() { (x, y) } else { (x - size / 2, y - size / 2) };
            bbox = Some(match bbox {
                None => (ix, iy, ix, iy),
                Some((a, b, c, d)) => (a.min(ix), b.min(iy), c.max(ix), d.max(iy)),
            });
            if (0..size).contains(&ix) && (0..size).contains(&iy) {
                mask[(iy * size + ix) as usize] = v;
            }
        }
    }
    // When measuring, the text was drawn at (size/2, size/2) so its "natural" origin is (0, 0).
    SelectObject(dc, old_font);
    SelectObject(dc, old_bmp);
    let _ = DeleteObject(font);
    let _ = DeleteObject(bmp);
    let _ = DeleteDC(dc);
    (mask, bbox)
}

/// Icon font: Segoe Fluent Icons on Windows 11, Segoe MDL2 Assets as fallback.
pub fn icon_font() -> &'static str {
    use std::sync::OnceLock;
    static FONT: OnceLock<&'static str> = OnceLock::new();
    FONT.get_or_init(|| unsafe {
        let dc = CreateCompatibleDC(HDC::default());
        let mut lf = LOGFONTW { lfHeight: -16, lfCharSet: DEFAULT_CHARSET, ..Default::default() };
        crate::util::copy_wide(&mut lf.lfFaceName, "Segoe Fluent Icons");
        let f = CreateFontIndirectW(&lf);
        let old = SelectObject(dc, f);
        let mut name = [0u16; 64];
        GetTextFaceW(dc, Some(&mut name));
        SelectObject(dc, old);
        let _ = DeleteObject(f);
        let _ = DeleteDC(dc);
        if from_wide(&name).eq_ignore_ascii_case("Segoe Fluent Icons") {
            "Segoe Fluent Icons"
        } else {
            "Segoe MDL2 Assets"
        }
    })
}

/// Premultiplied "over" of a solid color with coverage `a` (0..1) onto a BGRA pixel.
fn blend(px: &mut u32, (r, g, b): Rgb, a: f32) {
    if a <= 0.0 {
        return;
    }
    let a = a.min(1.0);
    let d = *px;
    let mix = |src: u8, dst: u32| (src as f32 * a + dst as f32 * (1.0 - a)).round() as u32;
    let da = (d >> 24) & 0xff;
    let oa = (255.0 * a + da as f32 * (1.0 - a)).round() as u32;
    let or = mix(r, (d >> 16) & 0xff);
    let og = mix(g, (d >> 8) & 0xff);
    let ob = mix(b, d & 0xff);
    *px = (oa << 24) | (or << 16) | (og << 8) | ob;
}

fn apply_mask(px: &mut [u32], mask: &[u8], color: Rgb, alpha: f32) {
    for (p, &m) in px.iter_mut().zip(mask) {
        blend(p, color, m as f32 / 255.0 * alpha);
    }
}

fn fill_rect(px: &mut [u32], size: i32, x0: f32, y0: i32, x1: f32, y1: i32, color: Rgb, alpha: f32) {
    for y in y0.max(0)..y1.min(size) {
        for x in 0..size {
            // Horizontal anti-aliasing so the bar end moves smoothly with the level.
            let cover = ((x as f32 + 1.0).min(x1) - (x as f32).max(x0)).clamp(0.0, 1.0);
            blend(&mut px[(y * size + x) as usize], color, cover * alpha);
        }
    }
}

/// Renders an icon to premultiplied BGRA pixels (row-major, top-down).
pub fn render(size: i32, spec: &IconSpec, style: Style) -> Vec<u32> {
    let (fg, _, red, green) = colors(style);
    let lc = level_color(spec.level, style);
    let mut px = vec![0u32; (size * size) as usize];
    let font = icon_font();
    unsafe {
        if spec.placeholder {
            let m = text_mask(size, &BATTERY.to_string(), font, size, 400, RECT { left: 0, top: 0, right: size, bottom: size });
            apply_mask(&mut px, &m, fg, 1.0);
        } else if style.number_mode {
            let text = spec.level.to_string();
            let m = text_mask(size, &text, "Segoe UI", size, 600, RECT { left: 0, top: 0, right: size, bottom: size });
            // No room for a bolt next to two digits, so charging shows as green digits.
            apply_mask(&mut px, &m, if spec.charging { green } else { lc }, 1.0);
        } else {
            let bar_h = (size / 8).max(2);
            let gap = (size / 16).max(1);
            let glyph_area = RECT { left: 0, top: 0, right: size, bottom: size - bar_h - gap };
            let gcolor = if spec.level < 15 { red } else { fg };
            let m = text_mask(size, &glyph(spec.kind).to_string(), font, size, 400, glyph_area);
            apply_mask(&mut px, &m, gcolor, 1.0);
            // Level bar along the bottom edge: dim track, colored fill.
            let margin = (size / 16).max(1) as f32;
            let (x0, x1) = (margin, size as f32 - margin);
            let y0 = size - bar_h;
            fill_rect(&mut px, size, x0, y0, x1, size, fg, 0.3);
            let fill_end = x0 + (x1 - x0) * spec.level.min(100) as f32 / 100.0;
            fill_rect(&mut px, size, x0, y0, fill_end.max(x0 + 1.0), size, if spec.charging { green } else { lc }, 1.0);
        }
        if spec.charging && !spec.placeholder && !style.number_mode {
            let s = size * 9 / 16;
            let m = text_mask(size, &BOLT.to_string(), font, s, 700, RECT { left: size - s, top: 0, right: size, bottom: s });
            // Knock out a 1px halo around the bolt so it stays readable on top of the glyph.
            let r = (size / 16).max(1);
            for y in 0..size {
                for x in 0..size {
                    let mut halo = 0u8;
                    for dy in -r..=r {
                        for dx in -r..=r {
                            let (nx, ny) = (x + dx, y + dy);
                            if (0..size).contains(&nx) && (0..size).contains(&ny) {
                                halo = halo.max(m[(ny * size + nx) as usize]);
                            }
                        }
                    }
                    let keep = 1.0 - halo as f32 / 255.0;
                    let p = &mut px[(y * size + x) as usize];
                    let sc = |v: u32| ((v as f32) * keep).round() as u32;
                    *p = (sc((*p >> 24) & 0xff) << 24) | (sc((*p >> 16) & 0xff) << 16) | (sc((*p >> 8) & 0xff) << 8) | sc(*p & 0xff);
                }
            }
            apply_mask(&mut px, &m, green, 1.0);
        }
    }
    if !spec.connected && !spec.placeholder {
        for p in px.iter_mut() {
            let scale = |v: u32| ((v as f32) * 0.45).round() as u32;
            let (a, r, g, b) = ((*p >> 24) & 0xff, (*p >> 16) & 0xff, (*p >> 8) & 0xff, *p & 0xff);
            *p = (scale(a) << 24) | (scale(r) << 16) | (scale(g) << 8) | scale(b);
        }
    }
    px
}

pub fn to_hicon(size: i32, px: &[u32]) -> HICON {
    unsafe {
        let mut bmi = BITMAPINFO::default();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = size;
        bmi.bmiHeader.biHeight = -size;
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        let mut bits: *mut c_void = std::ptr::null_mut();
        let Ok(color) = CreateDIBSection(HDC::default(), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) else {
            return HICON::default();
        };
        std::ptr::copy_nonoverlapping(px.as_ptr(), bits as *mut u32, px.len());
        let mask = CreateBitmap(size, size, 1, 1, None);
        let info = ICONINFO { fIcon: BOOL(1), xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&info).unwrap_or_default();
        let _ = DeleteObject(mask);
        let _ = DeleteObject(color);
        icon
    }
}

/// Writes a preview sheet (both taskbar themes, both modes, several levels) as a 32-bit BMP.
pub fn write_preview(path: &str) -> std::io::Result<()> {
    let samples = [
        IconSpec { kind: Kind::Mouse, level: 82, charging: false, connected: true, placeholder: false },
        IconSpec { kind: Kind::Keyboard, level: 24, charging: false, connected: true, placeholder: false },
        IconSpec { kind: Kind::Headset, level: 9, charging: false, connected: true, placeholder: false },
        IconSpec { kind: Kind::Gamepad, level: 60, charging: true, connected: true, placeholder: false },
        IconSpec { kind: Kind::Mouse, level: 63, charging: false, connected: false, placeholder: false },
        IconSpec { kind: Kind::Other, level: 100, charging: false, connected: true, placeholder: true },
    ];
    let sizes = [16, 24, 32];
    let zoom = 4;
    let cell = 32 * zoom + 8;
    let rows: Vec<(bool, bool, i32)> = [false, true]
        .iter()
        .flat_map(|&light| [false, true].into_iter().flat_map(move |num| sizes.into_iter().map(move |s| (light, num, s))))
        .collect();
    let (w, h) = (cell * samples.len() as i32, cell * rows.len() as i32);
    let mut img = vec![0u32; (w * h) as usize];
    for (ri, &(light, num, size)) in rows.iter().enumerate() {
        let bg: u32 = if light { 0xffeeeeee } else { 0xff1f1f1f };
        for (ci, spec) in samples.iter().enumerate() {
            let px = render(size, spec, Style { light_taskbar: light, number_mode: num });
            let (ox, oy) = (ci as i32 * cell, ri as i32 * cell);
            for y in 0..cell {
                for x in 0..cell {
                    img[((oy + y) * w + ox + x) as usize] = bg;
                }
            }
            for y in 0..size * zoom {
                for x in 0..size * zoom {
                    let p = px[((y / zoom) * size + x / zoom) as usize];
                    let a = (p >> 24) & 0xff;
                    let over = |s: u32, d: u32| s + d * (255 - a) / 255;
                    let d = bg;
                    let c = 0xff000000
                        | (over((p >> 16) & 0xff, (d >> 16) & 0xff) << 16)
                        | (over((p >> 8) & 0xff, (d >> 8) & 0xff) << 8)
                        | over(p & 0xff, d & 0xff);
                    img[((oy + 4 + y) * w + ox + 4 + x) as usize] = c;
                }
            }
        }
    }
    let mut out = Vec::with_capacity(54 + img.len() * 4);
    let data_len = (img.len() * 4) as u32;
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(54 + data_len).to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&w.to_le_bytes());
    out.extend_from_slice(&(-h).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&[0; 24]);
    for p in img {
        out.extend_from_slice(&p.to_le_bytes());
    }
    std::fs::write(path, out)
}
