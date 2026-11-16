//! Bluetooth battery levels, read from the device property Windows fills in for paired devices
//! (the same value Settings > Bluetooth & devices shows). Needs no admin and no pairing access.

use crate::model::{Kind, Reading};
use crate::util::from_wide;
use std::collections::BTreeMap;
use windows::core::{GUID, PCWSTR};
use windows::Win32::Devices::DeviceAndDriverInstallation::*;
use windows::Win32::Devices::Properties::{DEVPROPKEY, DEVPROPTYPE};
use windows::Win32::Foundation::HWND;

const BATTERY: DEVPROPKEY = DEVPROPKEY {
    fmtid: GUID::from_u128(0x104ea319_6ee2_4701_bd47_8ddbf425bbe5),
    pid: 2,
};
const NAME: DEVPROPKEY = DEVPROPKEY {
    fmtid: GUID::from_u128(0xb725f130_47ef_101a_a5f1_02608c9ebac0),
    pid: 10,
};
const CLASS_OF_DEVICE: DEVPROPKEY = DEVPROPKEY {
    fmtid: GUID::from_u128(0x2bd67d8b_8beb_48d5_87e0_6cda3428040a),
    pid: 10,
};

unsafe fn prop(set: HDEVINFO, d: &SP_DEVINFO_DATA, key: &DEVPROPKEY) -> Option<Vec<u8>> {
    let mut t = DEVPROPTYPE::default();
    let mut buf = vec![0u8; 512];
    let mut size = 0u32;
    SetupDiGetDevicePropertyW(set, d, key, &mut t, Some(&mut buf), Some(&mut size), 0).ok()?;
    buf.truncate(size as usize);
    Some(buf)
}

/// The Bluetooth address in an instance id, ignoring service UUIDs in braces.
fn mac_of(id: &str) -> Option<String> {
    let mut s = String::new();
    let mut depth = 0;
    for c in id.chars() {
        match c {
            '{' => depth += 1,
            '}' => depth -= 1,
            _ if depth == 0 => s.push(c),
            _ => {}
        }
    }
    let b = s.as_bytes();
    let mut found = None;
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_hexdigit() {
            let start = i;
            while i < b.len() && b[i].is_ascii_hexdigit() {
                i += 1;
            }
            if i - start == 12 {
                found = Some(s[start..i].to_uppercase());
            }
        } else {
            i += 1;
        }
    }
    found
}

fn kind_from_cod(cod: u32) -> Option<Kind> {
    let major = (cod >> 8) & 0x1f;
    match major {
        0x04 => Some(Kind::Headset),
        0x05 => match (cod >> 6) & 0x3 {
            1 | 3 => Some(Kind::Keyboard),
            2 => Some(Kind::Mouse),
            _ if (cod >> 2) & 0xf == 2 => Some(Kind::Gamepad),
            _ => None,
        },
        _ => None,
    }
}

pub fn poll() -> Vec<Reading> {
    let mut levels: BTreeMap<String, u8> = BTreeMap::new();
    let mut names: BTreeMap<String, (String, Option<Kind>)> = BTreeMap::new();
    unsafe {
        let Ok(set) = SetupDiGetClassDevsW(None, PCWSTR::null(), HWND::default(), DIGCF_ALLCLASSES | DIGCF_PRESENT) else {
            return vec![];
        };
        let mut i = 0;
        loop {
            let mut d = SP_DEVINFO_DATA { cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32, ..Default::default() };
            if SetupDiEnumDeviceInfo(set, i, &mut d).is_err() {
                break;
            }
            i += 1;
            let mut idbuf = [0u16; 512];
            if SetupDiGetDeviceInstanceIdW(set, &d, Some(&mut idbuf), None).is_err() {
                continue;
            }
            let id = from_wide(&idbuf).to_uppercase();
            if !(id.starts_with("BTHENUM\\") || id.starts_with("BTHLE\\") || id.starts_with("BTHLEDEVICE\\")) {
                continue;
            }
            let Some(mac) = mac_of(&id) else { continue };
            if let Some(v) = prop(set, &d, &BATTERY) {
                if let Some(&level) = v.first() {
                    if level <= 100 {
                        levels.insert(mac.clone(), level);
                    }
                }
            }
            if id.contains("\\DEV_") {
                let name = prop(set, &d, &NAME)
                    .map(|v| {
                        let w: Vec<u16> = v.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                        from_wide(&w)
                    })
                    .unwrap_or_default();
                let kind = prop(set, &d, &CLASS_OF_DEVICE)
                    .filter(|v| v.len() >= 4)
                    .and_then(|v| kind_from_cod(u32::from_le_bytes([v[0], v[1], v[2], v[3]])));
                names.insert(mac, (name, kind));
            }
        }
        let _ = SetupDiDestroyDeviceInfoList(set);
    }
    levels
        .into_iter()
        .map(|(mac, level)| {
            let (name, kind) = names.remove(&mac).unwrap_or_default();
            let name = if name.is_empty() { format!("Bluetooth device {mac}") } else { name };
            let kind = kind.unwrap_or_else(|| Kind::from_name(&name));
            Reading { key: format!("bt-{mac}"), name, kind, level, approx: false, charging: false, source: "Bluetooth" }
        })
        .collect()
}
