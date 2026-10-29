mod api;
mod app;
mod config;
mod data;
mod nowplaying;
mod player;
mod stats;
mod ui;
mod updater;

use std::collections::HashMap;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyEventKind};
use ratatui_image::picker::Picker;

use api::{Channel, Track};
use app::App;
use config::{Config, State};
use player::PlayerEvent;

pub enum AppEvent {
    Channels(Result<Vec<Channel>, String>),
    NowAll(HashMap<u64, Track>),
    History(u64, Vec<Track>),
    Streams(String, Result<Vec<String>, String>),
    Cover(String, Option<image::DynamicImage>),
    Player(PlayerEvent),
    Media(souvlaki::MediaControlEvent),
    Discord(bool),
    Status(String),
}

fn main() -> Result<()> {
    if std::env::args().any(|a| a == "--version" || a == "-V") {
        println!("ruDI.FM {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).is_some_and(|a| a == "update") {
        return updater::run();
    }
    if args.get(1).is_some_and(|a| a == "stats") {
        let entries = stats::load();
        match args.get(2).map(String::as_str) {
            Some("--csv") => stats::print_csv(&entries),
            Some("--json") => println!("{}", serde_json::to_string_pretty(&entries)?),
            _ => stats::print_summary(&entries),
        }
        return Ok(());
    }
    let cfg = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("ruDI.FM: {e:#}");
            std::process::exit(1);
        }
    };
    let state = State::load();
    check_mpv();

    let mut terminal = ratatui::init();
    // Graphics protocol (Kitty, Sixel, …) must be queried after entering the alternate screen.
    let picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks());
    // Late answers to that query would otherwise swallow the first key press.
    while event::poll(Duration::from_millis(80)).unwrap_or(false) {
        let _ = event::read();
    }

    let result = run(&mut terminal, cfg, state, picker);
    ratatui::restore();
    result
}

/// Exit with an install hint before the TUI starts if mpv is missing.
fn check_mpv() {
    let ok = std::process::Command::new("mpv")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if ok {
        return;
    }
    let hint = if cfg!(target_os = "macos") {
        "brew install mpv"
    } else if cfg!(target_os = "windows") {
        "winget install mpv"
    } else {
        "Paketmanager der Distribution, z. B. `sudo pacman -S mpv` oder `sudo apt install mpv`"
    };
    eprintln!("ruDI.FM braucht mpv für die Wiedergabe, es wurde aber nicht gefunden (PATH).\nInstallieren: {hint}");
    std::process::exit(1);
}

fn run(terminal: &mut ratatui::DefaultTerminal, cfg: Config, state: State, picker: Picker) -> Result<()> {
    let (tx, rx) = mpsc::channel();
    let mut app = App::new(cfg, state, picker, tx.clone())?;
    std::thread::spawn(move || {
        if updater::available() {
            let _ = tx.send(AppEvent::Status("Update verfügbar: im Terminal `rudi-fm update` ausführen".into()));
        }
    });

    while !app.quit {
        while let Ok(ev) = rx.try_recv() {
            app.handle(ev);
        }
        app.tick();
        terminal.draw(|f| ui::draw(f, &mut app))?;

        if event::poll(Duration::from_millis(200))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            app.on_key(key);
        }
    }
    app.stop();
    app.state.save();
    // give the publisher a moment to clear Discord/MPRIS
    app.tick();
    std::thread::sleep(Duration::from_millis(150));
    Ok(())
}
