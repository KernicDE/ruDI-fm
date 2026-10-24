//! Local listening log (`history.jsonl`, one line per heard track) and its aggregation.

use std::collections::{BTreeSet, HashMap};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use chrono::{Datelike, Local, TimeZone, Timelike};
use serde::{Deserialize, Serialize};

use crate::config::state_dir;

/// A track counts once it was heard at least this long.
pub const MIN_SECS: u64 = 30;
/// Gap between two tracks that still belongs to the same listening session.
const SESSION_GAP: i64 = 600;
const DAY: i64 = 86_400;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub ts: i64,
    pub channel_key: String,
    pub channel: String,
    pub artist: String,
    pub title: String,
    pub secs: u64,
}

fn path() -> PathBuf {
    state_dir().join("history.jsonl")
}

pub fn append(e: &Entry) {
    let _ = fs::create_dir_all(state_dir());
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path())
        && let Ok(line) = serde_json::to_string(e)
    {
        let _ = writeln!(f, "{line}");
    }
}

pub fn load() -> Vec<Entry> {
    let mut v: Vec<Entry> = fs::read_to_string(path())
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    v.sort_by_key(|e| e.ts);
    v
}

fn local_day(ts: i64) -> i32 {
    Local.timestamp_opt(ts, 0).single().map(|d| d.date_naive().num_days_from_ce()).unwrap_or(0)
}

fn local_midnight(ts: i64) -> i64 {
    Local.timestamp_opt(ts, 0).single().and_then(|d| d.date_naive().and_hms_opt(0, 0, 0)?.and_local_timezone(Local).earliest()).map(|d| d.timestamp()).unwrap_or(ts)
}

/// Seconds per channel key (all time) and seconds heard today.
pub fn totals(entries: &[Entry], now: i64) -> (HashMap<String, u64>, u64) {
    let today = local_day(now);
    let mut per = HashMap::new();
    let mut day = 0;
    for e in entries {
        *per.entry(e.channel_key.clone()).or_insert(0) += e.secs;
        if local_day(e.ts) == today {
            day += e.secs;
        }
    }
    (per, day)
}

pub fn fmt_dur(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m", secs / 60),
        _ => format!("{}h {:02}m", secs / 3600, secs / 60 % 60),
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Period {
    Today,
    Week,
    Month,
    Year,
    All,
}

impl Period {
    pub const ALL: [Period; 5] = [Period::Today, Period::Week, Period::Month, Period::Year, Period::All];

    pub fn label(self, now: i64) -> String {
        match self {
            Period::Today => "Heute".into(),
            Period::Week => "7 Tage".into(),
            Period::Month => "30 Tage".into(),
            Period::Year => format!("Jahr {}", Local.timestamp_opt(now, 0).single().map(|d| d.year()).unwrap_or(0)),
            Period::All => "Gesamt".into(),
        }
    }

    fn since(self, now: i64) -> i64 {
        match self {
            Period::Today => local_midnight(now),
            Period::Week => now - 7 * DAY,
            Period::Month => now - 30 * DAY,
            Period::Year => Local
                .timestamp_opt(now, 0)
                .single()
                .and_then(|d| Local.with_ymd_and_hms(d.year(), 1, 1, 0, 0, 0).earliest())
                .map(|d| d.timestamp())
                .unwrap_or(0),
            Period::All => 0,
        }
    }
}

/// (name, seconds, tracks, channels heard on), sorted by seconds descending.
pub type Ranking = Vec<(String, u64, u64, String)>;

type Acc = HashMap<String, (u64, u64, BTreeSet<String>)>;

#[derive(Debug, Default)]
pub struct Stats {
    pub secs: u64,
    pub tracks: u64,
    pub longest_session: u64,
    pub channels: Ranking,
    pub artists: Ranking,
    pub top_tracks: Ranking,
    /// Seconds per local start hour of the track.
    pub hours: [u64; 24],
    /// Seconds per day, last 14 days, today last (always over all entries).
    pub days: [u64; 14],
    /// Consecutive days with listening up to today (or yesterday).
    pub streak: u32,
}

fn rank(map: Acc) -> Ranking {
    let mut v: Vec<_> = map.into_iter().map(|(k, (s, n, c))| (k, s, n, c.into_iter().collect::<Vec<_>>().join(", "))).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v
}

/// `entries` must be sorted by `ts`.
pub fn aggregate(entries: &[Entry], period: Period, now: i64) -> Stats {
    let since = period.since(now);
    let (mut ch, mut ar, mut tr): (Acc, Acc, Acc) = Default::default();
    let mut s = Stats::default();
    let mut session = 0u64;
    let mut session_end = i64::MIN / 2;
    for e in entries.iter().filter(|e| e.ts >= since) {
        s.secs += e.secs;
        s.tracks += 1;
        // ponytail: the whole track counts for its start hour, tracks spanning an hour boundary are not split.
        s.hours[Local.timestamp_opt(e.ts, 0).single().map(|d| d.hour() as usize).unwrap_or(0)] += e.secs;
        session = if e.ts - session_end <= SESSION_GAP { session + e.secs } else { e.secs };
        session_end = e.ts + e.secs as i64;
        s.longest_session = s.longest_session.max(session);
        let add = |map: &mut Acc, key: String| {
            let v = map.entry(key).or_default();
            v.0 += e.secs;
            v.1 += 1;
            v.2.insert(e.channel.clone());
        };
        add(&mut ch, e.channel.clone());
        if !e.artist.is_empty() {
            add(&mut ar, e.artist.clone());
            add(&mut tr, format!("{} – {}", e.artist, e.title));
        }
    }
    (s.channels, s.artists, s.top_tracks) = (rank(ch), rank(ar), rank(tr));
    // a channel's own ranking needs no channel note
    s.channels.iter_mut().for_each(|c| c.3.clear());

    let today = local_day(now);
    let mut listened_days = std::collections::BTreeSet::new();
    for e in entries {
        let d = local_day(e.ts);
        listened_days.insert(d);
        if let Some(i) = (today - d).try_into().ok().filter(|i: &usize| *i < 14) {
            s.days[13 - i] += e.secs;
        }
    }
    let mut d = if listened_days.contains(&today) { today } else { today - 1 };
    while listened_days.contains(&d) {
        s.streak += 1;
        d -= 1;
    }
    s
}

pub fn print_summary(entries: &[Entry]) {
    let now = crate::nowplaying::unix_now();
    if entries.is_empty() {
        println!("Noch keine Hörstatistik (wird beim Hören in {} aufgebaut).", path().display());
        return;
    }
    for p in Period::ALL {
        let s = aggregate(entries, p, now);
        println!("== {} · {} · {} Tracks · längste Session {}", p.label(now), fmt_dur(s.secs), s.tracks, fmt_dur(s.longest_session));
        if p == Period::All {
            for (title, list) in [("Kanäle", &s.channels), ("Künstler", &s.artists), ("Tracks", &s.top_tracks)] {
                println!("\n{title}");
                for (n, secs, cnt, chans) in list.iter().take(10) {
                    println!("  {n:<40} {:>8}  {cnt}x  {chans}", fmt_dur(*secs));
                }
            }
            println!("\nSerie: {} Tage", s.streak);
        }
    }
}

pub fn print_csv(entries: &[Entry]) {
    let q = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
    println!("ts,channel_key,channel,artist,title,secs");
    for e in entries {
        println!("{},{},{},{},{},{}", e.ts, q(&e.channel_key), q(&e.channel), q(&e.artist), q(&e.title), e.secs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(ts: i64, ch: &str, artist: &str, title: &str, secs: u64) -> Entry {
        Entry { ts, channel_key: ch.into(), channel: ch.into(), artist: artist.into(), title: title.into(), secs }
    }

    #[test]
    fn aggregates_rankings_and_sessions() {
        let now = 1_800_000_000;
        let mut v = vec![
            e(now - 3000, "trance", "A", "x", 300),
            e(now - 2700, "trance", "A", "y", 300), // same session as above (gap 0)
            e(now - 100_000, "house", "B", "z", 100),
            e(now - 1000, "house", "", "jingle", 60), // no artist: only channel ranking
        ];
        v.sort_by_key(|e| e.ts);
        let s = aggregate(&v, Period::All, now);
        assert_eq!((s.tracks, s.secs), (4, 760));
        assert_eq!(s.longest_session, 600);
        assert_eq!(s.channels[0], ("trance".into(), 600, 2, String::new()));
        assert_eq!(s.top_tracks[0].3, "trance");
        assert_eq!(s.artists.len(), 2);
        assert_eq!(s.artists[0].0, "A");
        assert_eq!(aggregate(&v, Period::Week, now).tracks, 4);
        assert_eq!(totals(&v, now).0["house"], 160);
    }

    #[test]
    fn streak_counts_consecutive_days() {
        let now = 1_800_000_000;
        let v = vec![e(now - 2 * DAY, "a", "A", "x", 60), e(now - DAY, "a", "A", "x", 60), e(now, "a", "A", "x", 60)];
        assert_eq!(aggregate(&v, Period::All, now).streak, 3);
    }
}
