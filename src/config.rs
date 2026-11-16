//! Settings and last-known device levels, stored in %APPDATA%\BatteryTray\config.ini.

use crate::model::Kind;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Seen {
    pub name: String,
    pub kind: Kind,
    pub level: u8,
    pub approx: bool,
    pub when: String,
    pub source: String,
}

#[derive(Debug)]
pub struct Config {
    pub number_mode: bool,
    /// Low-battery alert threshold in percent; 0 turns alerts off.
    pub threshold: u8,
    pub names: BTreeMap<String, String>,
    pub icons: BTreeMap<String, Kind>,
    pub hidden: BTreeSet<String>,
    pub seen: BTreeMap<String, Seen>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            number_mode: false,
            threshold: 15,
            names: BTreeMap::new(),
            icons: BTreeMap::new(),
            hidden: BTreeSet::new(),
            seen: BTreeMap::new(),
        }
    }
}

fn path() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(base).join("BatteryTray").join("config.ini"))
}

fn clean(s: &str) -> String {
    s.replace(['\r', '\n', '|'], " ")
}

impl Config {
    pub fn load() -> Config {
        let mut c = Config::default();
        let Some(text) = path().and_then(|p| std::fs::read_to_string(p).ok()) else {
            return c;
        };
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            match k {
                "mode" => c.number_mode = v == "number",
                "threshold" => c.threshold = v.parse().unwrap_or(15),
                _ => {
                    if let Some(key) = k.strip_prefix("name.") {
                        c.names.insert(key.to_string(), v.to_string());
                    } else if let Some(key) = k.strip_prefix("icon.") {
                        if let Some(kind) = Kind::from_id(v) {
                            c.icons.insert(key.to_string(), kind);
                        }
                    } else if let Some(key) = k.strip_prefix("hidden.") {
                        if v == "1" {
                            c.hidden.insert(key.to_string());
                        }
                    } else if let Some(key) = k.strip_prefix("seen.") {
                        // level|approx|kind|when|source|name
                        let f: Vec<&str> = v.splitn(6, '|').collect();
                        if f.len() == 6 {
                            if let (Ok(level), Some(kind)) = (f[0].parse(), Kind::from_id(f[2])) {
                                c.seen.insert(
                                    key.to_string(),
                                    Seen {
                                        level,
                                        approx: f[1] == "1",
                                        kind,
                                        when: f[3].to_string(),
                                        source: f[4].to_string(),
                                        name: f[5].to_string(),
                                    },
                                );
                            }
                        }
                    }
                }
            }
        }
        c
    }

    pub fn save(&self) {
        let Some(p) = path() else { return };
        let mut out = String::from("# Battery Tray settings\n");
        out += &format!("mode={}\n", if self.number_mode { "number" } else { "bar" });
        out += &format!("threshold={}\n", self.threshold);
        for (k, v) in &self.names {
            out += &format!("name.{}={}\n", k, clean(v));
        }
        for (k, v) in &self.icons {
            out += &format!("icon.{}={}\n", k, v.id());
        }
        for k in &self.hidden {
            out += &format!("hidden.{}=1\n", k);
        }
        for (k, s) in &self.seen {
            out += &format!(
                "seen.{}={}|{}|{}|{}|{}|{}\n",
                k,
                s.level,
                if s.approx { 1 } else { 0 },
                s.kind.id(),
                clean(&s.when),
                clean(&s.source),
                clean(&s.name)
            );
        }
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, out);
    }
}
