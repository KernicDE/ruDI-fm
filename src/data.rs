//! German channel descriptions and grouping, embedded at compile time.

use std::collections::HashMap;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct ChannelInfo {
    pub key: String,
    pub group: String,
    pub tempo: String,
    pub desc: String,
    pub artists: String,
}

#[derive(Debug, Deserialize)]
pub struct Catalog {
    pub groups_order: Vec<String>,
    /// (label, channel keys) – order preserved in the JSON array of pairs.
    #[serde(deserialize_with = "moods_in_order")]
    pub moods: Vec<(String, Vec<String>)>,
    channels: Vec<ChannelInfo>,
    #[serde(skip)]
    by_key: HashMap<String, usize>,
}

fn moods_in_order<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<(String, Vec<String>)>, D::Error> {
    // serde_json with preserve_order is not enabled, so keep the known order explicitly.
    const ORDER: [&str; 4] = ["Arbeiten / Fokus", "Entspannen", "Gaming / Energie", "Party / Nostalgie"];
    let mut map: HashMap<String, Vec<String>> = HashMap::deserialize(d)?;
    let mut out: Vec<(String, Vec<String>)> =
        ORDER.iter().filter_map(|k| map.remove(*k).map(|v| (k.to_string(), v))).collect();
    let mut rest: Vec<_> = map.into_iter().collect();
    rest.sort();
    out.extend(rest);
    Ok(out)
}

impl Catalog {
    pub fn load() -> Self {
        let mut c: Catalog =
            serde_json::from_str(include_str!("../data/channels_de.json")).expect("channels_de.json ungültig");
        c.by_key = c.channels.iter().enumerate().map(|(i, ch)| (ch.key.clone(), i)).collect();
        c
    }

    pub fn get(&self, key: &str) -> Option<&ChannelInfo> {
        self.by_key.get(key).map(|&i| &self.channels[i])
    }

    /// Sort position of a group; unknown groups go last.
    pub fn group_rank(&self, group: &str) -> usize {
        self.groups_order.iter().position(|g| g == group).unwrap_or(usize::MAX)
    }

    /// Position of a channel within the embedded (curated) order.
    pub fn channel_rank(&self, key: &str) -> usize {
        self.by_key.get(key).copied().unwrap_or(usize::MAX)
    }
}
