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

/// Recent tracks of one channel, newest first.
pub fn fetch_history(channel_id: u64) -> Result<Vec<Track>> {
    let v = get_json(&format!("{BASE}/track_history/channel/{channel_id}"))?;
    Ok(v.as_array().into_iter().flatten().filter_map(Track::from_json).collect())
}

/// Stream URLs (mirrors) for a channel.
pub fn fetch_stream_urls(quality: &str, key: &str, listen_key: &str) -> Result<Vec<String>> {
    let v = get_json(&format!("{BASE}/listen/{quality}/{key}?listen_key={listen_key}"))?;
    let urls: Vec<String> = v.as_array().into_iter().flatten().filter_map(|u| u.as_str().map(String::from)).collect();
    anyhow::ensure!(!urls.is_empty(), crate::i18n::tr("Keine Stream-URLs (listen_key/quality prüfen)", "No stream URLs (check listen_key/quality)"));
    Ok(urls)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Show {
    #[serde(default)]
    pub slug: String,
    pub name: String,
    /// e.g. "Every Thursday".
    #[serde(default)]
    pub schedule: String,
    #[serde(default)]
    pub episodes: u64,
    pub tagline: String,
    pub channel_id: u64,
    /// Running right now (the API then already reports next week's slot).
    pub live: bool,
    /// Unix seconds of the next slot, 0 = unscheduled.
    pub next_start: i64,
    pub next_end: i64,
}

/// Active shows with a schedule; cached for offline start.
pub fn fetch_shows() -> Result<Vec<Show>> {
    let cache = cache_dir().join("shows.json");
    match fetch_shows_online() {
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

fn fetch_shows_online() -> Result<Vec<Show>> {
    let ts = |v: &Value| chrono::DateTime::parse_from_rfc3339(v.as_str().unwrap_or_default()).map_or(0, |d| d.timestamp());
    let mut out = Vec::new();
    // Inactive shows come last; stop at the first page without an active one.
    for page in 1..=10 {
        let v = get_json(&format!("{BASE}/shows?per_page=100&page={page}"))?;
        let list = v["results"].as_array().context("shows: kein results-Array")?;
        let before = out.len();
        out.extend(list.iter().filter(|s| s["active"].as_bool() == Some(true)).filter_map(|s| {
            Some(Show {
                slug: s["slug"].as_str()?.to_string(),
                schedule: s["human_readable_schedule"].as_array().into_iter().flatten().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", "),
                episodes: s["ondemand_episode_count"].as_u64().unwrap_or(0),
                name: s["name"].as_str()?.trim().to_string(),
                tagline: s["artists_tagline"].as_str().unwrap_or_default().trim().to_string(),
                channel_id: s["channels"][0]["id"].as_u64()?,
                live: s["now_playing"].as_bool().unwrap_or(false),
                next_start: ts(&s["next_start_at"]),
                next_end: ts(&s["next_end_at"]),
            })
        }));
        if out.len() == before {
            break;
        }
    }
    Ok(out)
}

/// Long text of one show (plain text).
pub fn fetch_show_description(slug: &str) -> Result<String> {
    let v = get_json(&format!("{BASE}/shows/{slug}"))?;
    Ok(v["description"].as_str().unwrap_or_default().trim().to_string())
}
