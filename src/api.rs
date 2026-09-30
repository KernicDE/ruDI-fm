//! AudioAddict / DI.FM public API.

use std::collections::HashMap;
use std::fs;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::cache_dir;

const BASE: &str = "https://api.audioaddict.com/v1/di";

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .user_agent(concat!("ruDI.FM/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

fn get_json(url: &str) -> Result<Value> {
    let mut resp = agent().get(url).call().with_context(|| format!("GET {url}"))?;
    Ok(resp.body_mut().read_json::<Value>()?)
}

pub fn get_bytes(url: &str) -> Result<Vec<u8>> {
    let mut resp = agent().get(url).call()?;
    Ok(resp.body_mut().with_config().limit(10 * 1024 * 1024).read_to_vec()?)
}

/// AudioAddict image URLs look like `//cdn-images…/x.png{?size,height,…}`.
pub fn image_url(raw: &str, size: u32) -> String {
    let base = raw.split('{').next().unwrap_or(raw);
    let base = if base.starts_with("//") { format!("https:{base}") } else { base.to_string() };
    format!("{base}?size={size}x{size}")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Channel {
    pub id: u64,
    pub key: String,
    pub name: String,
    pub description: String,
    pub artists: Vec<String>,
    pub image: Option<String>,
    /// Genre filters this channel belongs to (Trance, House, …).
    pub filters: Vec<String>,
    pub similar: Vec<u64>,
}

/// Active channels = members of the "All" filter. Cached for offline start.
pub fn fetch_channels() -> Result<Vec<Channel>> {
    let cache = cache_dir().join("channels.json");
    match fetch_channels_online() {
        Ok(list) => {
            let _ = fs::create_dir_all(cache_dir());
            let _ = fs::write(&cache, serde_json::to_vec(&list)?);
            Ok(list)
        }
        Err(e) => {
            let text = fs::read_to_string(&cache).map_err(|_| e)?;
            Ok(serde_json::from_str(&text)?)
        }
    }
}

fn fetch_channels_online() -> Result<Vec<Channel>> {
    let filters = get_json(&format!("{BASE}/channel_filters"))?;
    let filters = filters.as_array().context("channel_filters: kein Array")?;

    let mut membership: HashMap<u64, Vec<String>> = HashMap::new();
    for f in filters {
        let name = f["name"].as_str().unwrap_or_default();
        if matches!(name, "All" | "Popular" | "New") {
            continue;
        }
        for c in f["channels"].as_array().into_iter().flatten() {
            if let Some(id) = c["id"].as_u64() {
                membership.entry(id).or_default().push(name.to_string());
            }
        }
    }

    let all = filters
        .iter()
        .find(|f| f["name"] == "All")
        .and_then(|f| f["channels"].as_array())
        .context("Filter 'All' fehlt")?;

    Ok(all
        .iter()
        .filter_map(|c| {
            let id = c["id"].as_u64()?;
            Some(Channel {
                id,
                key: c["key"].as_str()?.to_string(),
                name: c["name"].as_str()?.trim().to_string(),
                description: c["description_short"].as_str().unwrap_or_default().trim().to_string(),
                artists: c["artists"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|a| a["name"].as_str().map(str::to_string))
                    .take(6)
                    .collect(),
                image: c["images"]["square"]
                    .as_str()
                    .or_else(|| c["images"]["default"].as_str())
                    .map(|s| image_url(s, 500)),
                filters: membership.remove(&id).unwrap_or_default(),
                similar: c["similar_channels"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|s| s["similar_channel_id"].as_u64())
                    .collect(),
            })
        })
        .collect())
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Track {
    pub artist: String,
    pub title: String,
    /// Unix seconds.
    pub started: i64,
    /// Seconds, 0 = unknown.
    pub duration: i64,
    pub art_url: Option<String>,
    pub votes_up: i64,
    pub votes_down: i64,
}

impl Track {
    fn from_json(v: &Value) -> Option<Self> {
        if v["type"].as_str() == Some("advertisement") {
            return None;
        }
        Some(Track {
            artist: v["display_artist"].as_str().or(v["artist"].as_str()).unwrap_or_default().to_string(),
            title: v["display_title"].as_str().or(v["title"].as_str()).unwrap_or_default().to_string(),
            started: v["started"].as_i64().unwrap_or(0),
            duration: v["duration"].as_i64().or(v["length"].as_i64()).unwrap_or(0),
            art_url: v["art_url"].as_str().filter(|s| !s.is_empty()).map(|s| image_url(s, 500)),
            votes_up: v["votes"]["up"].as_i64().unwrap_or(0),
            votes_down: v["votes"]["down"].as_i64().unwrap_or(0),
        })
    }

    pub fn display(&self) -> String {
        match (self.artist.is_empty(), self.title.is_empty()) {
            (false, false) => format!("{} – {}", self.artist, self.title),
            (true, false) => self.title.clone(),
            _ => self.artist.clone(),
        }
    }
}

/// Current track of every channel, keyed by channel id.
pub fn fetch_now_playing_all() -> Result<HashMap<u64, Track>> {
    let v = get_json(&format!("{BASE}/track_history"))?;
    let obj = v.as_object().context("track_history: kein Objekt")?;
    Ok(obj
        .iter()
        .filter_map(|(k, t)| Some((k.parse().ok()?, Track::from_json(t)?)))
        .collect())
}

/// Recent tracks of one channel, newest first.
pub fn fetch_history(channel_id: u64) -> Result<Vec<Track>> {
    let v = get_json(&format!("{BASE}/track_history/channel/{channel_id}"))?;
    Ok(v.as_array().into_iter().flatten().filter_map(Track::from_json).collect())
}

/// Stream URLs (mirrors) for a channel.
pub fn fetch_stream_urls(quality: &str, key: &str, listen_key: &str) -> Result<Vec<String>> {
    let v = get_json(&format!("{BASE}/listen/{quality}/{key}?listen_key={listen_key}"))?;
    let urls: Vec<String> = v.as_array().into_iter().flatten().filter_map(|u| u.as_str().map(String::from)).collect();
    anyhow::ensure!(!urls.is_empty(), "Keine Stream-URLs (listen_key/quality prüfen)");
    Ok(urls)
}
