use windows::Win32::System::SystemInformation::GetLocalTime;

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn from_wide(buf: &[u16]) -> String {
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

/// Copy a string into a fixed-size UTF-16 buffer (NUL-terminated, truncated if needed).
pub fn copy_wide(dst: &mut [u16], s: &str) {
    let mut i = 0;
    for c in s.encode_utf16() {
        if i + 1 >= dst.len() {
            break;
        }
        dst[i] = c;
        i += 1;
    }
    dst[i] = 0;
}

/// Local time like "13 Nov 14:32".
pub fn now_label() -> String {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let t = unsafe { GetLocalTime() };
    let m = MONTHS.get((t.wMonth as usize).wrapping_sub(1)).unwrap_or(&"");
    format!("{} {} {:02}:{:02}", t.wDay, m, t.wHour, t.wMinute)
}

/// 32-bit FNV-1a, used for stable tray icon ids.
pub fn hash32(s: &str) -> u32 {
    let mut h: u32 = 0x811c9dc5;
    for b in s.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x01000193);
    }
    h | 0x100 // never collide with the fallback icon id (1)
}
