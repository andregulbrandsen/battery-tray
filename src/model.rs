#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Mouse,
    Keyboard,
    Headset,
    Gamepad,
    Other,
}

impl Kind {
    pub const ALL: [Kind; 5] = [Kind::Mouse, Kind::Keyboard, Kind::Headset, Kind::Gamepad, Kind::Other];

    pub fn id(self) -> &'static str {
        match self {
            Kind::Mouse => "mouse",
            Kind::Keyboard => "keyboard",
            Kind::Headset => "headset",
            Kind::Gamepad => "gamepad",
            Kind::Other => "other",
        }
    }

    pub fn from_id(s: &str) -> Option<Kind> {
        Kind::ALL.iter().copied().find(|k| k.id() == s)
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Mouse => "Mouse",
            Kind::Keyboard => "Keyboard",
            Kind::Headset => "Headphones",
            Kind::Gamepad => "Controller",
            Kind::Other => "Other",
        }
    }

    /// Guess from a device name when the device doesn't say what it is.
    pub fn from_name(name: &str) -> Kind {
        let n = name.to_lowercase();
        let has = |words: &[&str]| words.iter().any(|w| n.contains(w));
        if has(&["mouse", "mx master", "mx anywhere", "trackball"]) {
            Kind::Mouse
        } else if has(&["keyboard", "keys", "k380", "k810"]) {
            Kind::Keyboard
        } else if has(&["headset", "headphone", "buds", "airpods", "earbuds", "wh-", "wf-", "jabra", "bose"]) {
            Kind::Headset
        } else if has(&["controller", "gamepad", "xbox", "dualsense", "joy-con"]) {
            Kind::Gamepad
        } else {
            Kind::Other
        }
    }
}

/// One battery reading from a provider.
#[derive(Clone, Debug)]
pub struct Reading {
    /// Stable id, e.g. "bt-C824789F3649" or "lg-C539-1".
    pub key: String,
    pub name: String,
    pub kind: Kind,
    pub level: u8,
    /// True when the percentage is estimated from battery voltage.
    pub approx: bool,
    pub charging: bool,
    pub source: &'static str,
}
