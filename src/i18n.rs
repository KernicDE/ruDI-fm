//! UI language (German/English): `language` in config.toml, "auto" follows the locale.

use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

static EN: AtomicBool = AtomicBool::new(false);

pub fn init(setting: &str) {
    let en = match setting {
        "de" => false,
        "en" => true,
        _ => ["LC_ALL", "LC_MESSAGES", "LANG"]
            .iter()
            .find_map(|v| std::env::var(v).ok().filter(|s| !s.is_empty()))
            .is_none_or(|l| !l.starts_with("de")),
    };
    EN.store(en, Relaxed);
}

pub fn en() -> bool {
    EN.load(Relaxed)
}

pub fn tr(de: &'static str, en_: &'static str) -> &'static str {
    if en() { en_ } else { de }
}
