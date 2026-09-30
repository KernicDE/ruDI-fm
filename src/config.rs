use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

pub const APP_ID: &str = "rudi-fm";

pub fn config_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join(APP_ID)
}

pub fn state_dir() -> PathBuf {
    dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(APP_ID)
}

pub fn cache_dir() -> PathBuf {
    dirs::cache_dir().unwrap_or_else(|| PathBuf::from(".")).join(APP_ID)
}

/// Output folder for "now playing" files (OBS text/image sources read these).
pub fn nowplaying_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(APP_ID)
        .join("nowplaying")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// DI.FM Premium listen key – kommt aus .env (DIFM_LISTEN_KEY), nie aus config.toml.
    #[serde(skip)]
    pub listen_key: String,
    /// Stream quality: premium_high (MP3 320k), premium (AAC 128k), premium_medium (AAC 64k).
    pub quality: String,
    /// Discord application ID for Rich Presence. Empty = disabled.
    pub discord_app_id: String,
    /// Port of the local OBS overlay web server. 0 = disabled.
    pub overlay_port: u16,
    /// Start playing the last channel on launch.
    pub autoplay: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen_key: String::new(),
            quality: "premium_high".into(),
            discord_app_id: String::new(),
            overlay_port: 8765,
            autoplay: false,
        }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        config_dir().join("config.toml")
    }

    pub fn load() -> Result<Self> {
        let path = Self::path();
        if !path.exists() {
            fs::create_dir_all(config_dir())?;
            fs::write(&path, DEFAULT_CONFIG)?;
        }
        let text = fs::read_to_string(&path).with_context(|| format!("{} lesen", path.display()))?;
        let mut cfg: Config = toml::from_str(&text).with_context(|| format!("{} parsen", path.display()))?;
        cfg.listen_key = listen_key_from_env().unwrap_or_default();
        if cfg.listen_key.is_empty() {
            bail!(
                "DIFM_LISTEN_KEY fehlt.\nIn {} oder ./.env eintragen: DIFM_LISTEN_KEY=<key>",
                config_dir().join(".env").display()
            );
        }
        Ok(cfg)
    }
}

/// Lookup order: process env, ~/.config/rudi-fm/.env, ./.env
fn listen_key_from_env() -> Option<String> {
    const VAR: &str = "DIFM_LISTEN_KEY";
    if let Ok(v) = std::env::var(VAR)
        && !v.trim().is_empty()
    {
        return Some(v.trim().to_string());
    }
    [config_dir().join(".env"), PathBuf::from(".env")]
        .iter()
        .filter_map(|p| fs::read_to_string(p).ok())
        .find_map(|text| {
            text.lines().find_map(|line| {
                let (k, v) = line.trim().strip_prefix("export ").unwrap_or(line.trim()).split_once('=')?;
                (k.trim() == VAR).then(|| v.trim().trim_matches(['"', '\'']).to_string())
            })
        })
        .filter(|v| !v.is_empty())
}

const DEFAULT_CONFIG: &str = r#"# ruDI.FM Konfiguration

# Der DI.FM Listen-Key steht NICHT hier, sondern in ~/.config/rudi-fm/.env
# (DIFM_LISTEN_KEY=...) bzw. als Umgebungsvariable.

# premium_high = MP3 320k, premium = AAC 128k, premium_medium = AAC 64k
quality = "premium_high"

# Discord Rich Presence: Application-ID von https://discord.com/developers/applications
# (App z.B. "DI.FM" nennen – der Name erscheint als "Hört DI.FM"). Leer = aus.
discord_app_id = ""

# Lokaler Webserver für OBS-Browserquelle (http://127.0.0.1:<port>/). 0 = aus.
overlay_port = 8765

# Beim Start den zuletzt gehörten Kanal abspielen
autoplay = false
"#;

/// Persistent UI state (favorites, last channel, volume).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub favorites: BTreeSet<String>,
    pub last_channel: Option<String>,
    pub volume: i64,
}

impl Default for State {
    fn default() -> Self {
        Self { favorites: BTreeSet::new(), last_channel: None, volume: 70 }
    }
}

impl State {
    fn path() -> PathBuf {
        state_dir().join("state.json")
    }

    pub fn load() -> Self {
        fs::read_to_string(Self::path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let _ = fs::create_dir_all(state_dir());
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = fs::write(Self::path(), json);
        }
    }
}
