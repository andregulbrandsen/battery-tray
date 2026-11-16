//! Embeds the app icon, version info and manifest as Windows resources.
//!
//! The resource section is written directly as a COFF object and handed to the linker, so the build
//! needs no rc.exe or windres (the GNU toolchain doesn't ship one). Layout follows the PE spec:
//! a three-level directory (type -> id -> language) followed by data entries and the raw data.

use std::{env, fs, path::PathBuf};

const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;
const RT_VERSION: u16 = 16;
const RT_MANIFEST: u16 = 24;
const LANG_EN_US: u32 = 0x0409;

const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0"
        processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
    </dependentAssembly>
  </dependency>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
    </windowsSettings>
  </application>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
    </application>
  </compatibility>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
</assembly>
"#;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/battery-tray.ico");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let (machine, reloc_addr32nb) = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("x86_64") => (0x8664u16, 0x0003u16),
        Ok("x86") => (0x014c, 0x0007),
        Ok("aarch64") => (0xaa64, 0x0002),
        other => panic!("unsupported target architecture for resources: {other:?}"),
    };

    let ico = fs::read("assets/battery-tray.ico").expect("read assets/battery-tray.ico");
    let (images, group) = parse_ico(&ico);
    let mut res: Vec<(u16, u16, Vec<u8>)> = Vec::new();
    for (i, img) in images.into_iter().enumerate() {
        res.push((RT_ICON, (i + 1) as u16, img));
    }
    res.push((RT_GROUP_ICON, 1, group));
    res.push((RT_VERSION, 1, version_info()));
    res.push((RT_MANIFEST, 1, MANIFEST.as_bytes().to_vec()));
    res.sort_by_key(|r| (r.0, r.1));

    let obj = coff_object(&res, machine, reloc_addr32nb);
    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("resources.o");
    fs::write(&out, obj).expect("write resource object");
    println!("cargo:rustc-link-arg-bins={}", out.display());
}

fn u16_at(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([d[o], d[o + 1]])
}

fn u32_at(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

/// Splits an .ico file into RT_ICON images and the RT_GROUP_ICON directory that references them.
fn parse_ico(d: &[u8]) -> (Vec<Vec<u8>>, Vec<u8>) {
    assert!(d.len() > 6 && u16_at(d, 2) == 1, "not an .ico file");
    let count = u16_at(d, 4) as usize;
    let mut images = Vec::new();
    let mut group = Vec::new();
    group.extend_from_slice(&d[0..6]);
    for i in 0..count {
        let e = 6 + i * 16;
        let size = u32_at(d, e + 8) as usize;
        let off = u32_at(d, e + 12) as usize;
        images.push(d[off..off + size].to_vec());
        group.extend_from_slice(&d[e..e + 12]); // width, height, colors, reserved, planes, bit count, bytes
        group.extend_from_slice(&((i + 1) as u16).to_le_bytes()); // RT_ICON id instead of file offset
    }
    (images, group)
}

fn pad4(b: &mut Vec<u8>) {
    while b.len() % 4 != 0 {
        b.push(0);
    }
}

fn utf16z(s: &str) -> Vec<u8> {
    s.encode_utf16().chain([0]).flat_map(|c| c.to_le_bytes()).collect()
}

/// One node of the VS_VERSIONINFO tree.
fn block(key: &str, text: bool, value: &[u8], value_len: u16, children: &[Vec<u8>]) -> Vec<u8> {
    let mut b = vec![0, 0];
    b.extend_from_slice(&value_len.to_le_bytes());
    b.extend_from_slice(&(text as u16).to_le_bytes());
    b.extend(utf16z(key));
    pad4(&mut b);
    b.extend_from_slice(value);
    for c in children {
        pad4(&mut b);
        b.extend_from_slice(c);
    }
    let len = b.len() as u16;
    b[0..2].copy_from_slice(&len.to_le_bytes());
    b
}

fn string(key: &str, value: &str) -> Vec<u8> {
    let v = utf16z(value);
    block(key, true, &v, (v.len() / 2) as u16, &[])
}

fn version_info() -> Vec<u8> {
    let part = |k: &str| env::var(k).unwrap().parse::<u32>().unwrap_or(0);
    let (major, minor, patch) = (part("CARGO_PKG_VERSION_MAJOR"), part("CARGO_PKG_VERSION_MINOR"), part("CARGO_PKG_VERSION_PATCH"));
    let version = env::var("CARGO_PKG_VERSION").unwrap();
    let ms = (major << 16) | minor;
    let ls = patch << 16;
    let fixed: Vec<u8> = [
        0xFEEF04BD, // signature
        0x00010000, // structure version
        ms, ls,     // file version
        ms, ls,     // product version
        0x3F,       // flags mask
        0,          // flags
        0x00040004, // VOS_NT_WINDOWS32
        1,          // VFT_APP
        0, 0, 0,    // subtype, date
    ]
    .iter()
    .flat_map(|v: &u32| v.to_le_bytes())
    .collect();

    let strings = [
        string("CompanyName", ""),
        string("FileDescription", "Battery Tray"),
        string("FileVersion", &version),
        string("InternalName", "battery-tray"),
        string("LegalCopyright", ""),
        string("OriginalFilename", "battery-tray.exe"),
        string("ProductName", "Battery Tray"),
        string("ProductVersion", &version),
    ];
    let table = block("040904B0", true, &[], 0, &strings);
    let sfi = block("StringFileInfo", true, &[], 0, &[table]);
    let translation = block("Translation", false, &[0x09, 0x04, 0xB0, 0x04], 4, &[]);
    let vfi = block("VarFileInfo", true, &[], 0, &[translation]);
    block("VS_VERSION_INFO", false, &fixed, fixed.len() as u16, &[sfi, vfi])
}

/// Builds a COFF object with a single .rsrc section holding the resource tree.
fn coff_object(res: &[(u16, u16, Vec<u8>)], machine: u16, reloc_addr32nb: u16) -> Vec<u8> {
    let mut types: Vec<u16> = res.iter().map(|r| r.0).collect();
    types.dedup();
    let n = res.len();
    let count_of = |t: u16| res.iter().filter(|r| r.0 == t).count();

    // Offsets within the section.
    let root = 0usize;
    let mut type_dir = Vec::new();
    let mut off = 16 + 8 * types.len();
    for &t in &types {
        type_dir.push(off);
        off += 16 + 8 * count_of(t);
    }
    let lang_dir = |k: usize, base: usize| base + k * 24;
    let lang_base = off;
    off += n * 24;
    let entry_base = off;
    off += n * 16;
    let mut blob = Vec::new();
    for r in res {
        off = (off + 7) & !7;
        blob.push(off);
        off += r.2.len();
    }
    let total = (off + 7) & !7;
    let mut s = vec![0u8; total];

    let put16 = |s: &mut Vec<u8>, o: usize, v: u16| s[o..o + 2].copy_from_slice(&v.to_le_bytes());
    let put32 = |s: &mut Vec<u8>, o: usize, v: u32| s[o..o + 4].copy_from_slice(&v.to_le_bytes());
    let dir = |s: &mut Vec<u8>, o: usize, ids: usize| put16(s, o + 14, ids as u16);
    const SUBDIR: u32 = 0x8000_0000;

    dir(&mut s, root, types.len());
    let mut k = 0;
    for (ti, &t) in types.iter().enumerate() {
        put32(&mut s, root + 16 + ti * 8, t as u32);
        put32(&mut s, root + 16 + ti * 8 + 4, type_dir[ti] as u32 | SUBDIR);
        dir(&mut s, type_dir[ti], count_of(t));
        for (ri, r) in res.iter().filter(|r| r.0 == t).enumerate() {
            let e = type_dir[ti] + 16 + ri * 8;
            put32(&mut s, e, r.1 as u32);
            put32(&mut s, e + 4, lang_dir(k, lang_base) as u32 | SUBDIR);
            let ld = lang_dir(k, lang_base);
            dir(&mut s, ld, 1);
            put32(&mut s, ld + 16, LANG_EN_US);
            put32(&mut s, ld + 20, (entry_base + k * 16) as u32);
            k += 1;
        }
    }
    for (k, r) in res.iter().enumerate() {
        let e = entry_base + k * 16;
        put32(&mut s, e, blob[k] as u32); // section offset; the ADDR32NB relocation turns it into an RVA
        put32(&mut s, e + 4, r.2.len() as u32);
        s[blob[k]..blob[k] + r.2.len()].copy_from_slice(&r.2);
    }

    let raw_ptr = 20 + 40;
    let reloc_ptr = raw_ptr + s.len();
    let sym_ptr = reloc_ptr + 10 * n;
    let mut o = Vec::new();
    // File header.
    o.extend_from_slice(&machine.to_le_bytes());
    o.extend_from_slice(&1u16.to_le_bytes()); // sections
    o.extend_from_slice(&0u32.to_le_bytes()); // timestamp
    o.extend_from_slice(&(sym_ptr as u32).to_le_bytes());
    o.extend_from_slice(&2u32.to_le_bytes()); // symbols (section symbol + its aux record)
    o.extend_from_slice(&0u16.to_le_bytes()); // optional header size
    o.extend_from_slice(&0u16.to_le_bytes()); // characteristics
    // Section header.
    o.extend_from_slice(b".rsrc\0\0\0");
    o.extend_from_slice(&0u32.to_le_bytes()); // virtual size
    o.extend_from_slice(&0u32.to_le_bytes()); // virtual address
    o.extend_from_slice(&(s.len() as u32).to_le_bytes());
    o.extend_from_slice(&(raw_ptr as u32).to_le_bytes());
    o.extend_from_slice(&(reloc_ptr as u32).to_le_bytes());
    o.extend_from_slice(&0u32.to_le_bytes()); // line numbers
    o.extend_from_slice(&(n as u16).to_le_bytes());
    o.extend_from_slice(&0u16.to_le_bytes());
    o.extend_from_slice(&0xC000_0040u32.to_le_bytes()); // initialized data, read, write
    o.extend_from_slice(&s);
    // Relocations: one per data entry, against the section symbol.
    for k in 0..n {
        o.extend_from_slice(&((entry_base + k * 16) as u32).to_le_bytes());
        o.extend_from_slice(&0u32.to_le_bytes());
        o.extend_from_slice(&reloc_addr32nb.to_le_bytes());
    }
    // Symbol table: ".rsrc" section symbol with its auxiliary section-definition record.
    o.extend_from_slice(b".rsrc\0\0\0");
    o.extend_from_slice(&0u32.to_le_bytes()); // value
    o.extend_from_slice(&1i16.to_le_bytes()); // section number
    o.extend_from_slice(&0u16.to_le_bytes()); // type
    o.push(3); // IMAGE_SYM_CLASS_STATIC
    o.push(1); // aux records
    o.extend_from_slice(&(s.len() as u32).to_le_bytes());
    o.extend_from_slice(&(n as u16).to_le_bytes());
    o.extend_from_slice(&0u16.to_le_bytes());
    o.extend_from_slice(&0u32.to_le_bytes()); // checksum
    o.extend_from_slice(&0u16.to_le_bytes()); // number
    o.push(0); // selection
    o.extend_from_slice(&[0, 0, 0]);
    o.extend_from_slice(&4u32.to_le_bytes()); // empty string table
    o
}
