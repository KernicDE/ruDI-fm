# ruDI.FM

Schlanker Terminal-Player für DI.FM Premium, geschrieben in Rust (ratatui + mpv).

- Alle 101 aktiven DI.FM-Kanäle, gruppiert nach Stilrichtung, mit deutschen Beschreibungen, Tempo und typischen Artists
- Aktueller Titel aller Kanäle direkt in der Liste, Verlauf pro Kanal
- Cover im Terminal (Kitty-Grafikprotokoll, sonst Halbblöcke)
- Favoriten, Suche, Stimmungs-Listen (Fokus, Entspannen, Gaming, Party)
- Automatischer Serverwechsel bei Stream-Abbruch (3 Spiegelserver) und Neuverbindung
- Now-Playing nach außen: MPRIS, Discord Rich Presence, Dateien und Web-Overlay für OBS

## Voraussetzungen

- `mpv` (Wiedergabe; ruDI.FM startet mpv ohne Nutzer-Config und ohne Skripte, damit das mpv-mpris-Plugin keinen zweiten Player anmeldet)
- Rust-Toolchain zum Bauen

## Einrichtung

```bash
cp .env.example .env              # DIFM_LISTEN_KEY=<dein Key> eintragen
cargo build --release
install -Dm755 target/release/rudi-fm ~/.local/bin/rudi-fm
install -Dm644 assets/rudi-fm.desktop ~/.local/share/applications/rudi-fm.desktop
```

Der Listen-Key wird in dieser Reihenfolge gesucht:
1. Umgebungsvariable `DIFM_LISTEN_KEY`
2. `~/.config/rudi-fm/.env` (hier ein Symlink auf die `.env` im Projekt)
3. `./.env`

`.env` steht in `.gitignore` und wird nie committet.

Weitere Einstellungen in `~/.config/rudi-fm/config.toml` (wird beim ersten Start angelegt):

| Schlüssel | Standard | Bedeutung |
|---|---|---|
| `quality` | `premium_high` | `premium_high` = MP3 320k, `premium` = AAC 128k, `premium_medium` = AAC 64k |
| `discord_app_id` | leer | Application-ID für Discord Rich Presence, leer = aus |
| `overlay_port` | `8765` | Port des OBS-Overlays, `0` = aus |
| `autoplay` | `false` | Zuletzt gehörten Kanal beim Start abspielen |

Favoriten, letzter Kanal und Lautstärke liegen in `~/.local/state/rudi-fm/state.json`.

## Bedienung

| Taste | Aktion |
|---|---|
| `↑/↓`, `j/k`, `PgUp/PgDn`, `g/G` | Kanal wählen |
| `Enter` | Kanal abspielen |
| `Leertaste` | Start/Stop (letzter Kanal) |
| `s` | Stop |
| `n` / `p` | Nächster / vorheriger Kanal der aktuellen Liste |
| `r` | Zufälliger Kanal |
| `o` | Zum laufenden Kanal springen |
| `+` / `-` | Lautstärke |
| `f` | Favorit an/aus |
| `/` | Suche in Name, Beschreibung, Artists, Genres (`Esc` beendet) |
| `Tab`, `1`–`6` | Liste: Alle, Favoriten, Stimmungen |
| `?` | Hilfe |
| `q` | Beenden |

Pause gibt es bewusst nicht: Bei Live-Radio würde ein pausierter Stream danach zeitversetzt weiterlaufen, deshalb ist es Start/Stop.

## Now-Playing nach außen

### MPRIS (Plasma, Medientasten, playerctl)
ruDI.FM meldet sich als MPRIS-Player `rudi_fm` mit Titel, Artist, Album (`DI.FM <Kanal>`), Länge und Cover (`mpris:artUrl`, lokale Datei).
Plasma-Medienwidget und Medientasten funktionieren direkt: Play/Pause = Start/Stop, Weiter/Zurück = Kanalwechsel.

```bash
playerctl -p rudi_fm metadata
```

### OBS
Drei Wege, je nach Geschmack:

1. **Browserquelle (fertiges Overlay):** URL `http://127.0.0.1:8765/`, z.B. 600×140. Cover, Titel, Artist, Kanal und Fortschrittsbalken, transparenter Hintergrund, blendet bei Stopp aus.
   Parameter: `?pos=right` (rechtsbündig), `?hide=0` (nie ausblenden), `?scale=1.3`.
2. **Text- und Bildquellen aus Dateien** in `~/.local/share/rudi-fm/nowplaying/`:
   `nowplaying.txt` (Artist – Titel), `artist.txt`, `title.txt`, `channel.txt`, `cover.jpg`, `nowplaying.json`.
3. **Now-Playing-Plugins mit MPRIS-Quelle** (z.B. Tuna): Player `rudi_fm` wählen.

JSON-Endpunkte für eigene Overlays: `/nowplaying.json` und `/cover`.

### Discord
Discord liest unter Linux kein MPRIS, deshalb braucht Rich Presence eine eigene Discord-Anwendung:

1. <https://discord.com/developers/applications> → *New Application* → Name z.B. `DI.FM` (der Name erscheint als „Hört DI.FM“)
2. *Application ID* kopieren und in `config.toml` als `discord_app_id` eintragen
3. ruDI.FM neu starten; unten rechts zeigt `Discord ●`, ob die Verbindung steht

Angezeigt werden Titel, Artist, Cover, Kanal, Restzeit und ein Button zum Kanal auf di.fm.

## Aufbau

```
src/
  main.rs         Terminal-Setup, Event-Loop
  app.rs          Zustand, Tasten, Wiedergabe, Titelabgleich ICY ↔ API
  ui.rs           ratatui-Oberfläche
  api.rs          AudioAddict-API (Kanäle, Now-Playing, Verlauf, Stream-URLs)
  player.rs       mpv über JSON-IPC
  nowplaying.rs   MPRIS, Discord, OBS-Dateien und -Webserver
  data.rs         eingebettete deutsche Kanaldaten
  overlay.html    OBS-Browserquelle
data/
  channels_de.json  Gruppen, Tempo, Beschreibungen, Stimmungslisten
```

Titelabgleich: Maßgeblich ist der ICY-Titel aus dem Stream, also das, was gerade tatsächlich zu hören ist. Zu diesem Titel sucht ruDI.FM im API-Verlauf des Kanals Cover, Länge und Votes. Die Kanalliste wird für den Offline-Start in `~/.cache/rudi-fm/` zwischengespeichert.
