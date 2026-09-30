use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use image::DynamicImage;
use ratatui::widgets::ListState;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;
use souvlaki::MediaControlEvent;

use crate::AppEvent;
use crate::api::{self, Channel, Track};
use crate::config::{Config, State};
use crate::data::Catalog;
use crate::nowplaying::{NowPlaying, Publisher, unix_now};
use crate::player::{Player, PlayerEvent};

const NOW_ALL_INTERVAL: Duration = Duration::from_secs(30);
const HISTORY_MAX_AGE: Duration = Duration::from_secs(60);
const SELECT_DEBOUNCE: Duration = Duration::from_millis(350);
const STATUS_TTL: Duration = Duration::from_secs(6);
const MATCH_RETRY: Duration = Duration::from_millis(1500);

#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    Header(String),
    Channel(usize),
}

pub struct Playing {
    pub channel: usize,
    urls: Vec<String>,
    url_idx: usize,
    failures: u32,
    retry_at: Option<Instant>,
    /// Audio actually flowing.
    pub audio: bool,
    pub icy: String,
    /// Unix time the current ICY title was first heard.
    pub heard_start: i64,
    /// API track matched to the ICY title (duration, cover, votes).
    pub track: Option<Track>,
    match_retries: u32,
    match_retry_at: Option<Instant>,
    /// First title after connecting: we joined mid-track, so take the start time from the API.
    first_title: bool,
}

impl Playing {
    /// Artist/title as heard: matched API track, else split ICY title.
    pub fn artist_title(&self) -> (String, String) {
        if let Some(t) = &self.track {
            return (t.artist.clone(), t.title.clone());
        }
        match self.icy.split_once(" - ") {
            Some((a, t)) => (a.trim().to_string(), t.trim().to_string()),
            None => (String::new(), self.icy.clone()),
        }
    }
}

pub struct App {
    pub cfg: Config,
    pub state: State,
    pub catalog: Catalog,
    pub channels: Vec<Channel>,
    pub loading: bool,
    pub now_all: HashMap<u64, Track>,
    now_all_at: Option<Instant>,
    pub history: HashMap<u64, (Instant, Vec<Track>)>,
    history_pending: Option<u64>,

    pub tabs: Vec<String>,
    pub tab: usize,
    pub search: String,
    pub searching: bool,
    pub rows: Vec<Row>,
    pub list: ListState,
    selected_at: Instant,
    pub show_help: bool,

    pub playing: Option<Playing>,
    stream_cache: HashMap<String, Vec<String>>,
    player: Player,
    publisher: Publisher,
    last_published: Option<NowPlaying>,

    pub picker: Picker,
    pub cover: Option<StatefulProtocol>,
    cover_url: Option<String>,
    cover_cache: HashMap<String, DynamicImage>,

    pub discord: Option<bool>,
    pub status: Option<(String, Instant)>,
    tx: Sender<AppEvent>,
    pub quit: bool,
}

fn spawn<F: FnOnce() -> AppEvent + Send + 'static>(tx: &Sender<AppEvent>, f: F) {
    let tx = tx.clone();
    thread::spawn(move || {
        let _ = tx.send(f());
    });
}

fn normalize(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

impl App {
    pub fn new(cfg: Config, state: State, picker: Picker, tx: Sender<AppEvent>) -> anyhow::Result<Self> {
        let catalog = Catalog::load();
        let player = Player::spawn(state.volume, tx.clone())?;
        let publisher = Publisher::start(cfg.discord_app_id.clone(), cfg.overlay_port, tx.clone());
        let mut tabs = vec!["Alle".to_string(), "★ Favoriten".to_string()];
        tabs.extend(catalog.moods.iter().map(|(m, _)| m.clone()));

        let app = App {
            cfg,
            state,
            catalog,
            channels: Vec::new(),
            loading: true,
            now_all: HashMap::new(),
            now_all_at: None,
            history: HashMap::new(),
            history_pending: None,
            tabs,
            tab: 0,
            search: String::new(),
            searching: false,
            rows: Vec::new(),
            list: ListState::default(),
            selected_at: Instant::now(),
            show_help: false,
            playing: None,
            stream_cache: HashMap::new(),
            player,
            publisher,
            last_published: None,
            picker,
            cover: None,
            cover_url: None,
            cover_cache: HashMap::new(),
            discord: None,
            status: None,
            tx,
            quit: false,
        };
        spawn(&app.tx, || AppEvent::Channels(api::fetch_channels().map_err(|e| e.to_string())));
        Ok(app)
    }

    pub fn set_status(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), Instant::now()));
    }

    // ------------------------------------------------------------ lookups

    pub fn info(&self, ch: &Channel) -> Option<&crate::data::ChannelInfo> {
        self.catalog.get(&ch.key)
    }

    pub fn group_of(&self, ch: &Channel) -> String {
        self.info(ch)
            .map(|i| i.group.clone())
            .or_else(|| ch.filters.first().cloned())
            .unwrap_or_else(|| "Weitere".into())
    }

    pub fn selected_channel(&self) -> Option<usize> {
        match self.list.selected().and_then(|i| self.rows.get(i)) {
            Some(Row::Channel(c)) => Some(*c),
            _ => None,
        }
    }

    fn channel_by_key(&self, key: &str) -> Option<usize> {
        self.channels.iter().position(|c| c.key == key)
    }

    pub fn channel_url(ch: &Channel) -> String {
        format!("https://www.di.fm/{}", ch.key)
    }

    // ------------------------------------------------------------ list building

    pub fn rebuild_rows(&mut self) {
        let keep = self.selected_channel().or(self.playing.as_ref().map(|p| p.channel));
        let mut rows = Vec::new();
        let q = self.search.to_lowercase();

        if !q.is_empty() {
            for (i, ch) in self.channels.iter().enumerate() {
                let info = self.info(ch);
                let hay = format!(
                    "{} {} {} {} {} {}",
                    ch.name,
                    ch.key,
                    info.map(|i| i.desc.as_str()).unwrap_or(&ch.description),
                    info.map(|i| i.group.as_str()).unwrap_or(""),
                    ch.artists.join(" "),
                    ch.filters.join(" ")
                )
                .to_lowercase();
                if q.split_whitespace().all(|w| hay.contains(w)) {
                    rows.push(Row::Channel(i));
                }
            }
        } else if self.tab == 0 {
            let mut last_group = String::new();
            for (i, ch) in self.channels.iter().enumerate() {
                let g = self.group_of(ch);
                if g != last_group {
                    rows.push(Row::Header(g.clone()));
                    last_group = g;
                }
                rows.push(Row::Channel(i));
            }
        } else if self.tab == 1 {
            rows.extend(
                self.channels
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| self.state.favorites.contains(&c.key))
                    .map(|(i, _)| Row::Channel(i)),
            );
        } else if let Some((_, keys)) = self.catalog.moods.get(self.tab - 2) {
            rows.extend(keys.iter().filter_map(|k| self.channel_by_key(k)).map(Row::Channel));
        }

        self.rows = rows;
        let idx = keep
            .and_then(|c| self.rows.iter().position(|r| *r == Row::Channel(c)))
            .or_else(|| self.rows.iter().position(|r| matches!(r, Row::Channel(_))));
        self.list.select(idx);
        self.selected_at = Instant::now();
    }

    fn move_selection(&mut self, delta: isize) {
        if self.rows.is_empty() {
            return;
        }
        let len = self.rows.len() as isize;
        let mut i = self.list.selected().unwrap_or(0) as isize;
        let step = delta.signum();
        let mut remaining = delta.abs();
        while remaining > 0 {
            let next = i + step;
            if next < 0 || next >= len {
                break;
            }
            i = next;
            if matches!(self.rows[i as usize], Row::Channel(_)) {
                remaining -= 1;
            }
        }
        // landed on header at the edge: search the other way
        while matches!(self.rows.get(i as usize), Some(Row::Header(_))) {
            i -= step;
        }
        self.list.select(Some(i.clamp(0, len - 1) as usize));
        self.selected_at = Instant::now();
    }

    fn select_edge(&mut self, last: bool) {
        let mut it = self.rows.iter().enumerate().filter(|(_, r)| matches!(r, Row::Channel(_)));
        let idx = if last { it.next_back() } else { it.next() }.map(|(i, _)| i);
        self.list.select(idx);
        self.selected_at = Instant::now();
    }

    /// Channel order used for next/previous: visible list, falling back to all.
    fn neighbour(&self, from: usize, forward: bool) -> Option<usize> {
        let order: Vec<usize> = {
            let v: Vec<usize> = self.rows.iter().filter_map(|r| if let Row::Channel(c) = r { Some(*c) } else { None }).collect();
            if v.contains(&from) { v } else { (0..self.channels.len()).collect() }
        };
        let pos = order.iter().position(|&c| c == from)?;
        let n = order.len();
        Some(order[if forward { (pos + 1) % n } else { (pos + n - 1) % n }])
    }

    // ------------------------------------------------------------ playback

    pub fn play_channel(&mut self, idx: usize) {
        let Some(ch) = self.channels.get(idx) else { return };
        let key = ch.key.clone();
        self.state.last_channel = Some(key.clone());
        self.state.save();
        let _ = self.player.stop();
        self.playing = Some(Playing {
            channel: idx,
            urls: Vec::new(),
            url_idx: 0,
            failures: 0,
            retry_at: None,
            audio: false,
            icy: String::new(),
            heard_start: 0,
            track: None,
            match_retries: 0,
            match_retry_at: None,
            first_title: true,
        });
        if let Some(urls) = self.stream_cache.get(&key).cloned() {
            self.start_urls(&key, urls);
        } else {
            let (q, lk) = (self.cfg.quality.clone(), self.cfg.listen_key.clone());
            spawn(&self.tx, move || {
                let r = api::fetch_stream_urls(&q, &key, &lk).map_err(|e| e.to_string());
                AppEvent::Streams(key, r)
            });
        }
        self.request_history(self.channels[idx].id, true);
    }

    fn start_urls(&mut self, key: &str, urls: Vec<String>) {
        let Some(p) = self.playing.as_mut() else { return };
        if self.channels[p.channel].key != key {
            return;
        }
        p.urls = urls;
        p.url_idx = 0;
        let url = p.urls[0].clone();
        if let Err(e) = self.player.play(&url) {
            self.set_status(format!("mpv: {e}"));
        }
    }

    pub fn stop(&mut self) {
        let _ = self.player.stop();
        self.playing = None;
    }

    pub fn toggle(&mut self) {
        if self.loading {
            self.cfg.autoplay = true;
            self.set_status("Kanalliste lädt – Wiedergabe startet gleich");
            return;
        }
        if self.playing.is_some() {
            self.stop();
        } else if let Some(idx) = self.state.last_channel.clone().and_then(|k| self.channel_by_key(&k)).or(self.selected_channel()) {
            self.play_channel(idx);
        }
    }

    fn skip(&mut self, forward: bool) {
        let from = self.playing.as_ref().map(|p| p.channel).or(self.selected_channel());
        if let Some(next) = from.and_then(|c| self.neighbour(c, forward)) {
            self.play_channel(next);
            if let Some(i) = self.rows.iter().position(|r| *r == Row::Channel(next)) {
                self.list.select(Some(i));
                self.selected_at = Instant::now();
            }
        }
    }

    fn change_volume(&mut self, delta: i64) {
        self.state.volume = (self.state.volume + delta).clamp(0, 130);
        let _ = self.player.set_volume(self.state.volume);
        self.state.save();
    }

    fn on_stream_failure(&mut self, reason: String) {
        let Some(p) = self.playing.as_mut() else { return };
        p.audio = false;
        p.failures += 1;
        if p.url_idx + 1 < p.urls.len() {
            p.url_idx += 1;
            let url = p.urls[p.url_idx].clone();
            let n = p.url_idx + 1;
            let _ = self.player.play(&url);
            self.set_status(format!("Stream abgebrochen ({reason}) – wechsle auf Server {n}"));
        } else {
            let wait = (p.failures.min(6) * 5) as u64;
            p.url_idx = 0;
            p.retry_at = Some(Instant::now() + Duration::from_secs(wait));
            self.set_status(format!("Stream nicht erreichbar ({reason}) – neuer Versuch in {wait}s"));
        }
    }

    // ------------------------------------------------------------ track matching

    fn request_history(&mut self, channel_id: u64, force: bool) {
        if !force && self.history_pending == Some(channel_id) {
            return;
        }
        if !force
            && let Some((at, _)) = self.history.get(&channel_id)
            && at.elapsed() < HISTORY_MAX_AGE
        {
            return;
        }
        self.history_pending = Some(channel_id);
        spawn(&self.tx, move || AppEvent::History(channel_id, api::fetch_history(channel_id).unwrap_or_default()));
    }

    fn try_match_track(&mut self) {
        let Some(p) = self.playing.as_mut() else { return };
        let ch_id = self.channels[p.channel].id;
        let Some((_, hist)) = self.history.get(&ch_id) else { return };
        if p.icy.is_empty() {
            // No ICY yet: assume the newest API entry is what we hear.
            if p.track.is_none() {
                p.track = hist.first().cloned();
                if let Some(t) = &p.track {
                    p.heard_start = t.started;
                }
            }
            return;
        }
        let icy = normalize(&p.icy);
        let found = hist.iter().find(|t| {
            let full = normalize(&format!("{}{}", t.artist, t.title));
            !full.is_empty() && (full == icy || icy.contains(&normalize(&t.title)) && icy.contains(&normalize(&t.artist)))
        });
        match found {
            Some(t) => {
                if p.first_title && t.started > 0 {
                    p.heard_start = t.started;
                }
                p.track = Some(t.clone());
                p.match_retry_at = None;
            }
            // The API often lists a new track a few seconds after it is heard: retry quickly.
            None if p.match_retries < 20 => {
                p.match_retries += 1;
                p.match_retry_at = Some(Instant::now() + MATCH_RETRY);
            }
            None => p.match_retry_at = None,
        }
    }

    pub fn now_playing(&self) -> NowPlaying {
        let Some(p) = &self.playing else { return NowPlaying::default() };
        let ch = &self.channels[p.channel];
        let (artist, title) = p.artist_title();
        NowPlaying {
            playing: true,
            channel: ch.name.clone(),
            channel_key: ch.key.clone(),
            channel_url: Self::channel_url(ch),
            artist,
            title,
            started: p.heard_start,
            duration: p.track.as_ref().map(|t| t.duration).unwrap_or(0),
            cover_url: p.track.as_ref().and_then(|t| t.art_url.clone()).or_else(|| ch.image.clone()),
        }
    }

    // ------------------------------------------------------------ events

    pub fn handle(&mut self, ev: AppEvent) {
        match ev {
            AppEvent::Channels(Ok(mut list)) => {
                list.sort_by_key(|c| {
                    let g = self.catalog.get(&c.key).map(|i| i.group.as_str()).unwrap_or("");
                    (self.catalog.group_rank(g), self.catalog.channel_rank(&c.key), c.name.clone())
                });
                self.channels = list;
                self.loading = false;
                self.rebuild_rows();
                if let Some(idx) = self.state.last_channel.clone().and_then(|k| self.channel_by_key(&k)) {
                    if let Some(i) = self.rows.iter().position(|r| *r == Row::Channel(idx)) {
                        self.list.select(Some(i));
                    }
                    if self.cfg.autoplay {
                        self.play_channel(idx);
                    }
                }
                self.now_all_at = None;
            }
            AppEvent::Channels(Err(e)) => {
                self.loading = false;
                self.set_status(format!("Kanalliste nicht ladbar: {e}"));
            }
            AppEvent::NowAll(map) => {
                if !map.is_empty() {
                    self.now_all = map;
                }
            }
            AppEvent::History(id, tracks) => {
                if self.history_pending == Some(id) {
                    self.history_pending = None;
                }
                if !tracks.is_empty() {
                    self.history.insert(id, (Instant::now(), tracks));
                }
                if self.playing.as_ref().is_some_and(|p| self.channels[p.channel].id == id) {
                    self.try_match_track();
                }
            }
            AppEvent::Streams(key, Ok(urls)) => {
                self.stream_cache.insert(key.clone(), urls.clone());
                self.start_urls(&key, urls);
            }
            AppEvent::Streams(_, Err(e)) => {
                self.set_status(format!("Stream-URLs: {e}"));
                self.playing = None;
            }
            AppEvent::Player(pe) => self.on_player(pe),
            AppEvent::Media(me) => self.on_media(me),
            AppEvent::Discord(ok) => {
                self.discord = Some(ok);
            }
            AppEvent::Status(s) => self.set_status(s),
            AppEvent::Cover(url, img) => {
                if let Some(img) = img {
                    self.cover_cache.insert(url.clone(), img);
                }
                if self.cover_url.as_deref() == Some(url.as_str()) {
                    self.cover = self.cover_cache.get(&url).map(|i| self.picker.new_resize_protocol(i.clone()));
                }
            }
        }
    }

    fn on_player(&mut self, ev: PlayerEvent) {
        let Some(p) = self.playing.as_mut() else { return };
        match ev {
            PlayerEvent::IcyTitle(t) => {
                if t != p.icy && !t.is_empty() {
                    p.first_title = p.icy.is_empty();
                    p.icy = t;
                    p.heard_start = unix_now();
                    p.track = None;
                    p.match_retries = 0;
                    let id = self.channels[p.channel].id;
                    self.try_match_track();
                    if self.playing.as_ref().is_some_and(|p| p.track.is_none()) {
                        self.request_history(id, true);
                    }
                }
            }
            PlayerEvent::Playing(on) => {
                p.audio = on;
                if on {
                    p.failures = 0;
                }
            }
            PlayerEvent::Ended(reason) => self.on_stream_failure(reason),
        }
    }

    fn on_media(&mut self, ev: MediaControlEvent) {
        match ev {
            MediaControlEvent::Play => {
                if self.playing.is_none() {
                    self.toggle();
                }
            }
            MediaControlEvent::Pause | MediaControlEvent::Stop => self.stop(),
            MediaControlEvent::Toggle => self.toggle(),
            MediaControlEvent::Next => self.skip(true),
            MediaControlEvent::Previous => self.skip(false),
            MediaControlEvent::SetVolume(v) => {
                let target = (v * 100.0).round() as i64;
                self.change_volume(target - self.state.volume);
            }
            MediaControlEvent::Quit => self.quit = true,
            _ => {}
        }
    }

    /// Periodic work, called every loop iteration.
    pub fn tick(&mut self) {
        if self.status.as_ref().is_some_and(|(_, at)| at.elapsed() > STATUS_TTL) {
            self.status = None;
        }
        if !self.loading && self.now_all_at.is_none_or(|t| t.elapsed() > NOW_ALL_INTERVAL) {
            self.now_all_at = Some(Instant::now());
            spawn(&self.tx, || AppEvent::NowAll(api::fetch_now_playing_all().unwrap_or_default()));
        }

        // stream reconnect / track match retries
        let mut replay = None;
        let mut rematch = None;
        if let Some(p) = self.playing.as_mut() {
            if p.retry_at.is_some_and(|t| Instant::now() >= t) {
                p.retry_at = None;
                replay = p.urls.first().cloned();
            }
            if p.match_retry_at.is_some_and(|t| Instant::now() >= t) {
                p.match_retry_at = None;
                rematch = Some(self.channels[p.channel].id);
            }
            // Track should be over but no new ICY title: refresh so progress/cover don't go stale.
            if let Some(t) = &p.track
                && t.duration > 0
                && unix_now() > p.heard_start + t.duration + 30
            {
                rematch = Some(self.channels[p.channel].id);
            }
        }
        if let Some(url) = replay {
            let _ = self.player.play(&url);
        }
        if let Some(id) = rematch {
            self.request_history(id, true);
        }

        // selection-dependent data (debounced)
        if self.selected_at.elapsed() > SELECT_DEBOUNCE
            && let Some(c) = self.selected_channel()
        {
            let id = self.channels[c].id;
            self.request_history(id, false);
        }

        // cover shown in the TUI: playing track, otherwise selected channel
        let want = if self.playing.is_some() {
            self.now_playing().cover_url
        } else if self.selected_at.elapsed() > SELECT_DEBOUNCE {
            self.selected_channel().and_then(|c| self.channels[c].image.clone())
        } else {
            self.cover_url.clone()
        };
        if want != self.cover_url {
            self.cover_url = want.clone();
            self.cover = None;
            if let Some(url) = want {
                if let Some(img) = self.cover_cache.get(&url) {
                    self.cover = Some(self.picker.new_resize_protocol(img.clone()));
                } else {
                    spawn(&self.tx, move || {
                        let img = api::get_bytes(&url).ok().and_then(|b| image::load_from_memory(&b).ok());
                        AppEvent::Cover(url, img)
                    });
                }
            }
        }

        let np = self.now_playing();
        if self.last_published.as_ref() != Some(&np) {
            self.publisher.update(np.clone());
            self.last_published = Some(np);
        }
    }

    // ------------------------------------------------------------ keys

    pub fn on_key(&mut self, key: KeyEvent) {
        if self.show_help {
            self.show_help = false;
            return;
        }
        if self.searching {
            match key.code {
                KeyCode::Esc => {
                    self.searching = false;
                    self.search.clear();
                    self.rebuild_rows();
                }
                KeyCode::Enter => {
                    self.searching = false;
                    if let Some(c) = self.selected_channel() {
                        self.play_channel(c);
                    }
                }
                KeyCode::Down => self.move_selection(1),
                KeyCode::Up => self.move_selection(-1),
                KeyCode::Backspace => {
                    self.search.pop();
                    self.rebuild_rows();
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.search.push(c);
                    self.rebuild_rows();
                }
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => self.quit = true,
            KeyCode::Esc if !self.search.is_empty() => {
                self.search.clear();
                self.rebuild_rows();
            }
            KeyCode::Char('?') | KeyCode::F(1) => self.show_help = true,
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::PageDown => self.move_selection(10),
            KeyCode::PageUp => self.move_selection(-10),
            KeyCode::Home | KeyCode::Char('g') => self.select_edge(false),
            KeyCode::End | KeyCode::Char('G') => self.select_edge(true),
            KeyCode::Enter => {
                if let Some(c) = self.selected_channel() {
                    self.play_channel(c);
                }
            }
            KeyCode::Char(' ') => self.toggle(),
            KeyCode::Char('s') => self.stop(),
            KeyCode::Char('n') => self.skip(true),
            KeyCode::Char('p') => self.skip(false),
            KeyCode::Char('+') | KeyCode::Char('=') => self.change_volume(5),
            KeyCode::Char('-') => self.change_volume(-5),
            KeyCode::Char('f') => {
                if let Some(c) = self.selected_channel() {
                    let key = self.channels[c].key.clone();
                    if !self.state.favorites.remove(&key) {
                        self.state.favorites.insert(key);
                    }
                    self.state.save();
                    if self.tab == 1 {
                        self.rebuild_rows();
                    }
                }
            }
            KeyCode::Char('/') => {
                self.searching = true;
                self.search.clear();
                self.rebuild_rows();
            }
            KeyCode::Char('r') => {
                if !self.channels.is_empty() {
                    let idx = (unix_now() as usize).wrapping_mul(2654435761) % self.channels.len();
                    self.play_channel(idx);
                }
            }
            KeyCode::Char('o') => {
                // jump to the playing channel
                if let Some(p) = &self.playing
                    && let Some(i) = self.rows.iter().position(|r| *r == Row::Channel(p.channel))
                {
                    self.list.select(Some(i));
                    self.selected_at = Instant::now();
                }
            }
            KeyCode::Tab => self.set_tab((self.tab + 1) % self.tabs.len()),
            KeyCode::BackTab => self.set_tab((self.tab + self.tabs.len() - 1) % self.tabs.len()),
            KeyCode::Char(c @ '1'..='9') => {
                let i = c as usize - '1' as usize;
                if i < self.tabs.len() {
                    self.set_tab(i);
                }
            }
            _ => {}
        }
    }

    fn set_tab(&mut self, t: usize) {
        self.tab = t;
        self.search.clear();
        self.rebuild_rows();
    }
}
