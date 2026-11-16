//! Logitech wireless devices (Lightspeed, Bolt, Unifying receivers and direct USB devices) over HID++ 2.0.
//! Uses the receiver's vendor HID collection, which normal users can open. Read-only queries.

use crate::model::{Kind, Reading};
use std::time::{Duration, Instant};
use windows::core::PCWSTR;
use windows::Win32::Devices::DeviceAndDriverInstallation::*;
use windows::Win32::Devices::HumanInterfaceDevice::*;
use windows::Win32::Foundation::*;
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::Threading::{CreateEventW, ResetEvent, WaitForSingleObject};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};

/// Our HID++ software id, so replies meant for G HUB or Options+ are ignored.
const SW: u8 = 0x0A;
const REPLY_TIMEOUT: Duration = Duration::from_millis(300);

const LIGHTSPEED: &[u16] = &[0xC539, 0xC53A, 0xC53D, 0xC53F, 0xC541, 0xC545, 0xC547, 0xC54D];
const MULTI_SLOT: &[u16] = &[0xC52B, 0xC532, 0xC548];

struct Hid {
    h: HANDLE,
    ev: HANDLE,
}

impl Drop for Hid {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.h);
            let _ = CloseHandle(self.ev);
        }
    }
}

enum Io {
    Write,
    Read,
}

impl Hid {
    unsafe fn io(&self, op: Io, buf: &mut [u8; 20], timeout_ms: u32) -> bool {
        let mut ov = OVERLAPPED { hEvent: self.ev, ..Default::default() };
        let _ = ResetEvent(self.ev);
        let r = match op {
            Io::Write => WriteFile(self.h, Some(&buf[..]), None, Some(&mut ov)),
            Io::Read => ReadFile(self.h, Some(&mut buf[..]), None, Some(&mut ov)),
        };
        if let Err(e) = r {
            if e.code() != ERROR_IO_PENDING.to_hresult() {
                return false;
            }
        }
        let mut n = 0u32;
        if WaitForSingleObject(self.ev, timeout_ms) != WAIT_OBJECT_0 {
            let _ = CancelIoEx(self.h, Some(&ov));
            let _ = GetOverlappedResult(self.h, &ov, &mut n, true);
            return false;
        }
        GetOverlappedResult(self.h, &ov, &mut n, false).is_ok() && n > 0
    }

    /// Sends a long HID++ request and returns the 16 parameter bytes of the matching reply.
    fn request(&self, dev: u8, feat: u8, func: u8, params: &[u8]) -> Option<[u8; 16]> {
        let mut msg = [0u8; 20];
        msg[0] = 0x11;
        msg[1] = dev;
        msg[2] = feat;
        msg[3] = (func << 4) | SW;
        msg[4..4 + params.len()].copy_from_slice(params);
        unsafe {
            if !self.io(Io::Write, &mut msg.clone(), 500) {
                return None;
            }
            let deadline = Instant::now() + REPLY_TIMEOUT;
            loop {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return None;
                }
                let mut r = [0u8; 20];
                if !self.io(Io::Read, &mut r, left.as_millis().max(1) as u32) {
                    return None;
                }
                if r[0] != 0x11 || r[1] != dev {
                    continue;
                }
                if r[2] == feat && r[3] == msg[3] {
                    let mut out = [0u8; 16];
                    out.copy_from_slice(&r[4..20]);
                    return Some(out);
                }
                if r[2] == 0xFF && r[3] == feat && r[4] == msg[3] {
                    return None; // HID++ error reply
                }
            }
        }
    }

    fn feature(&self, dev: u8, id: u16) -> Option<u8> {
        let r = self.request(dev, 0, 0, &[(id >> 8) as u8, id as u8])?;
        (r[0] != 0).then_some(r[0])
    }
}

struct Battery {
    level: u8,
    approx: bool,
    charging: bool,
}

/// Rough Li-ion discharge curve for devices that only report voltage.
fn voltage_to_percent(mv: u16) -> u8 {
    const CURVE: [(u16, u8); 11] = [
        (4186, 100), (4067, 90), (3989, 80), (3922, 70), (3859, 60), (3811, 50),
        (3778, 40), (3751, 30), (3717, 20), (3671, 10), (3500, 0),
    ];
    if mv >= CURVE[0].0 {
        return 100;
    }
    for w in CURVE.windows(2) {
        let (hi, lo) = (w[0], w[1]);
        if mv >= lo.0 {
            let t = (mv - lo.0) as f32 / (hi.0 - lo.0) as f32;
            return (lo.1 as f32 + t * (hi.1 - lo.1) as f32).round() as u8;
        }
    }
    0
}

fn read_battery(hid: &Hid, dev: u8) -> Option<Battery> {
    if let Some(fi) = hid.feature(dev, 0x1004) {
        // Unified battery: percent when supported, otherwise a coarse level.
        if let Some(s) = hid.request(dev, fi, 1, &[]) {
            let level = if s[0] > 0 {
                s[0]
            } else if s[1] & 8 != 0 {
                100
            } else if s[1] & 4 != 0 {
                60
            } else if s[1] & 2 != 0 {
                20
            } else {
                5
            };
            return Some(Battery { level, approx: s[0] == 0, charging: s[2] == 1 || s[2] == 2 });
        }
    }
    if let Some(fi) = hid.feature(dev, 0x1000) {
        if let Some(s) = hid.request(dev, fi, 0, &[]) {
            return Some(Battery { level: s[0].min(100), approx: false, charging: s[2] == 1 || s[2] == 2 });
        }
    }
    if let Some(fi) = hid.feature(dev, 0x1001) {
        if let Some(s) = hid.request(dev, fi, 0, &[]) {
            let mv = u16::from_be_bytes([s[0], s[1]]);
            if mv > 0 {
                return Some(Battery { level: voltage_to_percent(mv), approx: true, charging: s[2] & 0x80 != 0 });
            }
        }
    }
    None
}

fn read_name(hid: &Hid, dev: u8) -> (String, Option<Kind>) {
    let Some(fi) = hid.feature(dev, 0x0005) else { return (String::new(), None) };
    let mut name = Vec::new();
    if let Some(c) = hid.request(dev, fi, 0, &[]) {
        let len = c[0] as usize;
        while name.len() < len {
            let Some(part) = hid.request(dev, fi, 1, &[name.len() as u8]) else { break };
            let take = (len - name.len()).min(16);
            name.extend_from_slice(&part[..take]);
        }
    }
    let kind = hid.request(dev, fi, 2, &[]).and_then(|t| match t[0] {
        0 | 2 => Some(Kind::Keyboard),
        3 | 4 | 5 => Some(Kind::Mouse),
        8 => Some(Kind::Headset),
        11 | 12 => Some(Kind::Gamepad),
        _ => None,
    });
    let name = String::from_utf8_lossy(&name).trim_end_matches('\0').trim().to_string();
    (name, kind)
}

fn probe(hid: &Hid, dev: u8, pid: u16, out: &mut Vec<Reading>) -> bool {
    // Ping (root feature, getProtocolVersion); asleep or empty slots don't answer.
    if hid.request(dev, 0, 1, &[0, 0, 0x5A]).is_none() {
        return false;
    }
    let Some(b) = read_battery(hid, dev) else { return true };
    let (name, kind) = read_name(hid, dev);
    let name = if name.is_empty() { format!("Logitech device {pid:04X}") } else { name };
    let kind = kind.unwrap_or_else(|| Kind::from_name(&name));
    let slot = if dev == 0xFF { "d".to_string() } else { dev.to_string() };
    let source = if dev == 0xFF { "Logitech USB" } else if LIGHTSPEED.contains(&pid) { "Logitech Lightspeed" } else { "Logitech receiver" };
    out.push(Reading {
        key: format!("lg-{pid:04X}-{slot}"),
        name,
        kind,
        level: b.level,
        approx: b.approx,
        charging: b.charging,
        source,
    });
    true
}

unsafe fn hid_paths() -> Vec<Vec<u16>> {
    let mut out = Vec::new();
    let guid = HidD_GetHidGuid();
    let Ok(set) = SetupDiGetClassDevsW(Some(&guid), PCWSTR::null(), HWND::default(), DIGCF_PRESENT | DIGCF_DEVICEINTERFACE) else {
        return out;
    };
    let mut i = 0;
    loop {
        let mut d = SP_DEVICE_INTERFACE_DATA { cbSize: std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32, ..Default::default() };
        if SetupDiEnumDeviceInterfaces(set, None, &guid, i, &mut d).is_err() {
            break;
        }
        i += 1;
        let mut req = 0u32;
        let _ = SetupDiGetDeviceInterfaceDetailW(set, &d, None, 0, Some(&mut req), None);
        if req < 8 {
            continue;
        }
        let mut buf = vec![0u64; (req as usize + 7) / 8];
        let p = buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
        (*p).cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
        if SetupDiGetDeviceInterfaceDetailW(set, &d, Some(p), req, None, None).is_ok() {
            let s = (p as *const u8).add(4) as *const u16;
            let mut path = Vec::new();
            let mut k = 0;
            while *s.add(k) != 0 {
                path.push(*s.add(k));
                k += 1;
            }
            path.push(0);
            out.push(path);
        }
    }
    let _ = SetupDiDestroyDeviceInfoList(set);
    out
}

pub fn poll() -> Vec<Reading> {
    let mut out = Vec::new();
    unsafe {
        for path in hid_paths() {
            let lower = String::from_utf16_lossy(&path).to_lowercase();
            if !lower.contains("vid_046d") {
                continue;
            }
            let Ok(h) = CreateFileW(
                PCWSTR(path.as_ptr()),
                (GENERIC_READ | GENERIC_WRITE).0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                HANDLE::default(),
            ) else {
                continue;
            };
            let Ok(ev) = CreateEventW(None, true, false, PCWSTR::null()) else {
                let _ = CloseHandle(h);
                continue;
            };
            let hid = Hid { h, ev };
            let mut pd = PHIDP_PREPARSED_DATA::default();
            if !HidD_GetPreparsedData(hid.h, &mut pd).as_bool() {
                continue;
            }
            let mut caps = HIDP_CAPS::default();
            let _ = HidP_GetCaps(pd, &mut caps);
            let _ = HidD_FreePreparsedData(pd);
            // Only the HID++ long-report collection (vendor page, 20-byte reports).
            if caps.UsagePage < 0xFF00 || caps.InputReportByteLength != 20 || caps.OutputReportByteLength != 20 {
                continue;
            }
            let mut attr = HIDD_ATTRIBUTES { Size: std::mem::size_of::<HIDD_ATTRIBUTES>() as u32, ..Default::default() };
            let _ = HidD_GetAttributes(hid.h, &mut attr);
            let pid = attr.ProductID;
            if LIGHTSPEED.contains(&pid) {
                probe(&hid, 1, pid, &mut out);
            } else if MULTI_SLOT.contains(&pid) {
                for dev in 1..=6 {
                    probe(&hid, dev, pid, &mut out);
                }
            } else {
                // Device plugged in directly (wired or with its own dongle); wired ones have no battery feature.
                probe(&hid, 0xFF, pid, &mut out);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn curve() {
        assert_eq!(super::voltage_to_percent(4300), 100);
        assert_eq!(super::voltage_to_percent(3875), 63);
        assert_eq!(super::voltage_to_percent(3400), 0);
    }
}
