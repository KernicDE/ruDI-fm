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
    channels: Vec<ChannelInfo>,
    #[serde(skip)]
    by_key: HashMap<String, usize>,
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
