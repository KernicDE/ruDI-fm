//! Publishes the current track to the outside world:
//! MPRIS (Plasma, playerctl, OBS plugins like Tuna), Discord Rich Presence,
//! files for OBS text/image sources and a small web overlay for OBS browser sources.

use std::fs;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use discord_rich_presence::activity::{Activity, ActivityType, Assets, Button, StatusDisplayType, Timestamps};
use discord_rich_presence::{DiscordIpc, DiscordIpcClient};
use serde::Serialize;
use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig};

use crate::AppEvent;
use crate::api;
use crate::config::nowplaying_dir;

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct NowPlaying {
    pub playing: bool,
    pub channel: String,
    pub channel_key: String,
    pub channel_url: String,
    pub artist: String,
    pub title: String,
    /// Unix seconds when the track started (as heard), 0 = unknown.
    pub started: i64,
    /// Seconds, 0 = unknown.
    pub duration: i64,
    /// Remote cover (track art, falls back to channel image).
    pub cover_url: Option<String>,
}

impl NowPlaying {
    fn text(&self) -> String {
        match (self.artist.is_empty(), self.title.is_empty()) {
            (false, false) => format!("{} – {}", self.artist, self.title),
            (true, false) => self.title.clone(),
            (false, true) => self.artist.clone(),
            _ => self.channel.clone(),
        }
    }
}

pub fn unix_now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Shared with the overlay web server.
#[derive(Default)]
struct Shared {
    np: NowPlaying,
    /// Cover served at /cover: (source URL, JPEG bytes).
    cover: Option<(String, Vec<u8>)>,
    /// Increments whenever the served cover changes.
    cover_seq: u64,
    version: u64,
}

enum Msg {
    Update(NowPlaying),
    /// Finished cover download (None = failed).
    Cover(String, Option<Vec<u8>>),
}

pub struct Publisher {
    tx: Sender<Msg>,
}

impl Publisher {
    pub fn start(discord_app_id: String, overlay_port: u16, events: Sender<AppEvent>) -> Self {
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Mutex::new(Shared::default()));
        if overlay_port != 0 {
            let shared = shared.clone();
            let events = events.clone();
            thread::spawn(move || {
                if let Err(e) = overlay_server(overlay_port, shared) {
                    let _ = events.send(AppEvent::Status(format!("Overlay-Server: {e}")));
                }
            });
        }
        let loop_tx = tx.clone();
        thread::spawn(move || publish_loop(rx, loop_tx, shared, discord_app_id, events));
        Publisher { tx }
    }

    pub fn update(&self, np: NowPlaying) {
        let _ = self.tx.send(Msg::Update(np));
    }
}

/// Write via temp file + rename, so OBS never reads a half-written file.
fn write_atomic(path: &Path, data: &[u8]) {
    let tmp = path.with_extension("tmp");
    if fs::write(&tmp, data).is_ok() {
        let _ = fs::rename(&tmp, path);
    }
}

/// Normalize any cover (DI.FM channel images are PNG) to JPEG.
fn to_jpeg(bytes: &[u8]) -> Vec<u8> {
    let Ok(img) = image::load_from_memory(bytes) else { return bytes.to_vec() };
    let mut out = Vec::new();
    let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90);
    match img.to_rgb8().write_with_encoder(enc) {
        Ok(()) => out,
        Err(_) => bytes.to_vec(),
    }
}

const COVER_CACHE: usize = 24;

fn publish_loop(
    rx: Receiver<Msg>,
    tx: Sender<Msg>,
    shared: Arc<Mutex<Shared>>,
    discord_app_id: String,
    events: Sender<AppEvent>,
) {
    let mpris_tx = events.clone();
    let dbus_name = format!("rudi_fm.instance{}", std::process::id());
    let mut controls = MediaControls::new(PlatformConfig {
        display_name: "ruDI.FM",
        dbus_name: &dbus_name,
        hwnd: None,
    })
    .ok();
    if let Some(c) = controls.as_mut() {
        let _ = c.attach(move |e: MediaControlEvent| {
            let _ = mpris_tx.send(AppEvent::Media(e));
        });
    }

    let mut discord = Discord::new(discord_app_id);
    let out_dir = nowplaying_dir();
    let _ = fs::create_dir_all(&out_dir);
    // leftovers from a previous run (per-track MPRIS covers)
    for entry in fs::read_dir(&out_dir).into_iter().flatten().flatten() {
        if entry.file_name().to_string_lossy().starts_with("cover-") {
            let _ = fs::remove_file(entry.path());
        }
    }

    let mut cache: VecDeque<(String, Vec<u8>)> = VecDeque::new();
    let mut downloading: Option<String> = None;
    let mut cover_file: Option<PathBuf> = None;
    let mut current = NowPlaying::default();

    loop {
        // Wait for work; wake up periodically to retry Discord and failed cover downloads.
        let mut msgs = Vec::new();
        match rx.recv_timeout(Duration::from_secs(15)) {
            Ok(m) => msgs.push(m),
            Err(mpsc::RecvTimeoutError::Timeout) => discord.set(&current, None),
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        msgs.extend(rx.try_iter());

        let mut np_changed = false;
        for m in msgs {
            match m {
                Msg::Update(np) => {
                    np_changed |= np != current;
                    current = np;
                }
                Msg::Cover(url, bytes) => {
                    if downloading.as_deref() == Some(url.as_str()) {
                        downloading = None;
                    }
                    if let Some(b) = bytes {
                        cache.retain(|(u, _)| *u != url);
                        cache.push_back((url, to_jpeg(&b)));
                        while cache.len() > COVER_CACHE {
                            cache.pop_front();
                        }
                    }
                }
            }
        }

        // Cover: swap in as soon as it is available; until then the previous one stays.
        let mut cover_changed = false;
        let served = shared.lock().unwrap_or_else(|e| e.into_inner()).cover.as_ref().map(|(u, _)| u.clone());
        if let Some(url) = current.cover_url.clone().filter(|u| served.as_ref() != Some(u)) {
            if let Some((_, jpeg)) = cache.iter().find(|(u, _)| *u == url) {
                write_atomic(&out_dir.join("cover.jpg"), jpeg);
                let unique = out_dir.join(format!("cover-{}.jpg", unix_now()));
                if let Some(old) = cover_file.take() {
                    let _ = fs::remove_file(old);
                }
                if fs::write(&unique, jpeg).is_ok() {
                    cover_file = Some(unique);
                }
                let mut sh = shared.lock().unwrap_or_else(|e| e.into_inner());
                sh.cover = Some((url, jpeg.clone()));
                sh.cover_seq += 1;
                cover_changed = true;
            } else if downloading.is_none() {
                downloading = Some(url.clone());
                let tx = tx.clone();
                thread::spawn(move || {
                    let bytes = api::get_bytes(&url).ok();
                    let _ = tx.send(Msg::Cover(url, bytes));
                });
            }
        }

        if !np_changed && !cover_changed {
            continue;
        }

        let cover_ready = {
            let mut sh = shared.lock().unwrap_or_else(|e| e.into_inner());
            sh.np = current.clone();
            sh.version += 1;
            sh.cover.as_ref().map(|(u, _)| u) == current.cover_url.as_ref()
        };
        write_files(&out_dir, &current);

        if let Some(c) = controls.as_mut() {
            let local = cover_file.as_ref().filter(|_| cover_ready).map(|p| format!("file://{}", p.display()));
            let title = if current.title.is_empty() { current.channel.clone() } else { current.title.clone() };
            let artist = current.artist.clone();
            let album = format!("DI.FM {}", current.channel);
            let _ = c.set_metadata(MediaMetadata {
                title: Some(&title),
                artist: Some(&artist),
                album: Some(&album),
                cover_url: local.as_deref().or(current.cover_url.as_deref()),
                duration: (current.duration > 0).then(|| Duration::from_secs(current.duration as u64)),
            });
            let progress =
                (current.started > 0).then(|| MediaPosition(Duration::from_secs((unix_now() - current.started).max(0) as u64)));
            let _ = c.set_playback(if current.playing { MediaPlayback::Playing { progress } } else { MediaPlayback::Stopped });
        }

        // Discord loads the cover from the remote URL itself, only text changes matter.
        if np_changed {
            discord.set(&current, Some(true));
        }
        if let Some(status) = discord.take_status() {
            let _ = events.send(AppEvent::Discord(status));
        }
    }
    discord.clear();
}

fn write_files(dir: &Path, np: &NowPlaying) {
    let text = if np.playing { np.text() } else { String::new() };
    write_atomic(&dir.join("nowplaying.txt"), text.as_bytes());
    write_atomic(&dir.join("artist.txt"), if np.playing { np.artist.as_bytes() } else { b"" });
    write_atomic(&dir.join("title.txt"), if np.playing { np.title.as_bytes() } else { b"" });
    write_atomic(&dir.join("channel.txt"), if np.playing { np.channel.as_bytes() } else { b"" });
    if let Ok(json) = serde_json::to_string_pretty(np) {
        write_atomic(&dir.join("nowplaying.json"), json.as_bytes());
    }
}

// ---------------------------------------------------------------- Discord

struct Discord {
    app_id: String,
    client: Option<DiscordIpcClient>,
    last_attempt: i64,
    status: Option<bool>,
    reported: Option<bool>,
}

impl Discord {
    fn new(app_id: String) -> Self {
        Discord { app_id, client: None, last_attempt: 0, status: None, reported: None }
    }

    /// Current connection state, only when it changed since the last call.
    fn take_status(&mut self) -> Option<bool> {
        (self.status.is_some() && self.status != self.reported).then(|| {
            self.reported = self.status;
            self.status.unwrap()
        })
    }

    fn connect(&mut self) -> bool {
        if self.client.is_some() {
            return true;
        }
        if self.app_id.is_empty() || unix_now() - self.last_attempt < 30 {
            return false;
        }
        self.last_attempt = unix_now();
        let mut c = DiscordIpcClient::new(&self.app_id);
        if c.connect().is_ok() {
            self.client = Some(c);
            self.status = Some(true);
            true
        } else {
            self.status = Some(false);
            false
        }
    }

    /// `changed == None` is a periodic retry: only acts if not yet connected.
    fn set(&mut self, np: &NowPlaying, changed: Option<bool>) {
        let was_connected = self.client.is_some();
        if changed.is_none() && was_connected {
            return;
        }
        if !self.connect() {
            return;
        }
        if was_connected && changed == Some(false) {
            return;
        }
        let client = self.client.as_mut().unwrap();
        let result = if np.playing {
            let details = if np.title.is_empty() { np.channel.clone() } else { np.title.clone() };
            let state = if np.artist.is_empty() { format!("DI.FM {}", np.channel) } else { np.artist.clone() };
            let large_text = format!("DI.FM · {}", np.channel);
            let mut assets = Assets::new().large_text(&large_text);
            if let Some(url) = np.cover_url.as_deref() {
                assets = assets.large_image(url);
            }
            let mut ts = Timestamps::new();
            if np.started > 0 {
                ts = ts.start(np.started * 1000);
                if np.duration > 0 {
                    ts = ts.end((np.started + np.duration) * 1000);
                }
            }
            let activity = Activity::new()
                .activity_type(ActivityType::Listening)
                .status_display_type(StatusDisplayType::Details)
                .details(&details)
                .state(&state)
                .assets(assets)
                .timestamps(ts)
                .buttons(vec![Button::new("Kanal auf DI.FM", &np.channel_url)]);
            client.set_activity(activity)
        } else {
            client.clear_activity()
        };
        if result.is_err() {
            let _ = client.close();
            self.client = None;
            self.status = Some(false);
        }
    }

    fn clear(&mut self) {
        if let Some(c) = self.client.as_mut() {
            let _ = c.clear_activity();
            let _ = c.close();
        }
    }
}

// ---------------------------------------------------------------- OBS overlay

fn header(k: &str, v: &str) -> tiny_http::Header {
    tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()).unwrap()
}

fn overlay_server(port: u16, shared: Arc<Mutex<Shared>>) -> anyhow::Result<()> {
    let server = tiny_http::Server::http(("127.0.0.1", port)).map_err(|e| anyhow::anyhow!("{e}"))?;
    for req in server.incoming_requests() {
        // One thread per request: a client that stops reading (e.g. a hidden OBS source)
        // must not block everybody else.
        let shared = shared.clone();
        thread::spawn(move || {
            let resp = overlay_response(req.url(), &shared)
                .with_header(header("Cache-Control", "no-store"))
                .with_header(header("Access-Control-Allow-Origin", "*"));
            let _ = req.respond(resp);
        });
    }
    Ok(())
}

fn overlay_response(url: &str, shared: &Mutex<Shared>) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
    let sh = || shared.lock().unwrap_or_else(|e| e.into_inner());
    match url.split('?').next().unwrap_or("/") {
        "/" | "/overlay" => tiny_http::Response::from_data(OVERLAY_HTML.as_bytes().to_vec())
            .with_header(header("Content-Type", "text/html; charset=utf-8")),
        "/nowplaying.json" => {
            let sh = sh();
            let mut v = serde_json::to_value(&sh.np).unwrap_or_default();
            v["version"] = sh.version.into();
            v["cover_seq"] = sh.cover_seq.into();
            v["local_cover"] = (sh.cover.as_ref().map(|(u, _)| u) == sh.np.cover_url.as_ref()).into();
            tiny_http::Response::from_data(v.to_string().into_bytes()).with_header(header("Content-Type", "application/json"))
        }
        "/cover" => match sh().cover.as_ref().map(|(_, b)| b.clone()) {
            Some(bytes) => tiny_http::Response::from_data(bytes).with_header(header("Content-Type", "image/jpeg")),
            None => tiny_http::Response::from_data(Vec::new()).with_status_code(404),
        },
        _ => tiny_http::Response::from_data(b"not found".to_vec()).with_status_code(404),
    }
}

const OVERLAY_HTML: &str = include_str!("overlay.html");
