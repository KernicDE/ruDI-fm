//! mpv as audio backend, controlled via JSON IPC.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::AppEvent;

#[derive(Debug, Clone)]
pub enum PlayerEvent {
    /// ICY stream title ("Artist - Title").
    IcyTitle(String),
    /// Audio is actually flowing (false while buffering/stopped).
    Playing(bool),
    /// Stream ended or failed; payload = reason from mpv.
    Ended(String),
}

pub struct Player {
    child: Child,
    socket: UnixStream,
    path: PathBuf,
}

impl Player {
    pub fn spawn(volume: i64, tx: Sender<AppEvent>) -> Result<Self> {
        let path = std::env::temp_dir().join(format!("rudi-fm-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);

        let child = Command::new("mpv")
            .args([
                "--idle=yes",
                "--no-video",
                "--no-terminal",
                // no user config / scripts: avoids a second MPRIS entry from mpv-mpris
                "--config=no",
                "--load-scripts=no",
                "--audio-client-name=ruDI.FM",
                "--cache=yes",
                "--cache-secs=20",
                "--demuxer-readahead-secs=10",
                "--network-timeout=15",
                &format!("--volume={volume}"),
                &format!("--input-ipc-server={}", path.display()),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("mpv starten (ist mpv installiert?)")?;

        let mut socket = None;
        for _ in 0..50 {
            if let Ok(s) = UnixStream::connect(&path) {
                socket = Some(s);
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        let socket = socket.context("mpv IPC-Socket nicht erreichbar")?;

        let reader = socket.try_clone()?;
        thread::spawn(move || read_events(reader, tx));

        let mut p = Player { child, socket, path };
        p.command(json!(["observe_property", 1, "metadata"]))?;
        p.command(json!(["observe_property", 2, "core-idle"]))?;
        Ok(p)
    }

    fn command(&mut self, cmd: Value) -> Result<()> {
        let mut line = serde_json::to_vec(&json!({ "command": cmd }))?;
        line.push(b'\n');
        self.socket.write_all(&line)?;
        Ok(())
    }

    pub fn play(&mut self, url: &str) -> Result<()> {
        self.command(json!(["loadfile", url, "replace"]))?;
        self.command(json!(["set_property", "pause", false]))
    }

    /// Live radio: stop instead of pause, so resuming is not delayed.
    pub fn stop(&mut self) -> Result<()> {
        self.command(json!(["stop"]))
    }

    pub fn set_volume(&mut self, volume: i64) -> Result<()> {
        self.command(json!(["set_property", "volume", volume]))
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.command(json!(["quit"]));
        thread::sleep(Duration::from_millis(100));
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.path);
    }
}

fn read_events(stream: UnixStream, tx: Sender<AppEvent>) {
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { break };
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        let ev = match v["event"].as_str() {
            Some("property-change") => match v["name"].as_str() {
                Some("metadata") => v["data"]
                    .as_object()
                    .and_then(|m| m.iter().find(|(k, _)| k.eq_ignore_ascii_case("icy-title")))
                    .and_then(|(_, t)| t.as_str())
                    .map(|t| PlayerEvent::IcyTitle(t.trim().to_string())),
                Some("core-idle") => v["data"].as_bool().map(|idle| PlayerEvent::Playing(!idle)),
                _ => None,
            },
            Some("end-file") => {
                let reason = v["reason"].as_str().unwrap_or("");
                // "stop"/"redirect" are caused by us (stop / loadfile replace)
                (!matches!(reason, "stop" | "redirect" | "quit")).then(|| {
                    let detail = v["file_error"].as_str().unwrap_or(reason);
                    PlayerEvent::Ended(detail.to_string())
                })
            }
            _ => None,
        };
        if let Some(ev) = ev
            && tx.send(AppEvent::Player(ev)).is_err()
        {
            break;
        }
    }
}
