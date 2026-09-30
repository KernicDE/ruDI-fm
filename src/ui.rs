use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Clear, LineGauge, List, ListItem, Padding, Paragraph, Tabs, Wrap};
use ratatui_image::{Resize, StatefulImage};

use crate::app::{App, Row};
use crate::nowplaying::unix_now;

const ACCENT: Color = Color::Rgb(63, 169, 245);
const ACCENT_DIM: Color = Color::Rgb(30, 80, 120);
const GOLD: Color = Color::Rgb(240, 190, 70);
const MUTED: Color = Color::Rgb(125, 132, 150);
const FG: Color = Color::Rgb(225, 228, 235);
const OK: Color = Color::Rgb(110, 200, 120);
const WARN: Color = Color::Rgb(230, 110, 90);

fn block(title: &str) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT_DIM))
        .title(Span::styled(format!(" {title} "), Style::new().fg(ACCENT).bold()))
}

fn mmss(secs: i64) -> String {
    let s = secs.max(0);
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

fn ago(started: i64) -> String {
    let d = (unix_now() - started).max(0);
    match d {
        0..=59 => "gerade".into(),
        60..=3599 => format!("vor {} min", d / 60),
        _ => format!("vor {} h", d / 3600),
    }
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let [header, body, footer] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(10), Constraint::Length(1)]).areas(f.area());

    draw_header(f, app, header);

    let [left, right] = Layout::horizontal([Constraint::Percentage(42), Constraint::Percentage(58)]).areas(body);
    draw_list(f, app, left);

    let [now, info, hist] =
        Layout::vertical([Constraint::Length(13), Constraint::Min(8), Constraint::Length(9)]).areas(right);
    draw_now_playing(f, app, now);
    draw_info(f, app, info);
    draw_history(f, app, hist);
    draw_footer(f, app, footer);

    if app.show_help {
        draw_help(f);
    }
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let [logo, tabs] = Layout::horizontal([Constraint::Length(11), Constraint::Min(10)]).areas(area);
    f.render_widget(
        Line::from(vec![Span::styled(" ru", Style::new().fg(MUTED)), Span::styled("DI.FM ", Style::new().fg(ACCENT).bold())]),
        logo,
    );
    let titles: Vec<Line> = app.tabs.iter().enumerate().map(|(i, t)| Line::from(format!("{} {t}", i + 1))).collect();
    let selected = if app.search.is_empty() && !app.searching { Some(app.tab) } else { None };
    f.render_widget(
        Tabs::new(titles)
            .select(selected)
            .style(Style::new().fg(MUTED))
            .highlight_style(Style::new().fg(ACCENT).bold().add_modifier(Modifier::UNDERLINED))
            .divider(Span::styled("│", Style::new().fg(ACCENT_DIM))),
        tabs,
    );
}

fn draw_list(f: &mut Frame, app: &mut App, area: Rect) {
    let title = if app.searching || !app.search.is_empty() {
        format!("Suche: {}{}", app.search, if app.searching { "▏" } else { "" })
    } else {
        let n = app.rows.iter().filter(|r| matches!(r, Row::Channel(_))).count();
        format!("{} · {n} Kanäle", app.tabs[app.tab])
    };
    let blk = block(&title);

    if app.loading {
        f.render_widget(Paragraph::new("Lade Kanäle …").fg(MUTED).block(blk), area);
        return;
    }
    if app.rows.is_empty() {
        let msg = if app.tab == 1 && app.search.is_empty() { "Noch keine Favoriten – mit f markieren." } else { "Keine Treffer." };
        f.render_widget(Paragraph::new(msg).fg(MUTED).block(blk), area);
        return;
    }

    let inner_w = area.width.saturating_sub(4) as usize;
    let name_w = 24.min(inner_w);
    let playing = app.playing.as_ref().map(|p| p.channel);
    let items: Vec<ListItem> = app
        .rows
        .iter()
        .map(|r| match r {
            Row::Header(g) => ListItem::new(Line::from(Span::styled(
                format!("── {g} "),
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
            ))),
            Row::Channel(i) => {
                let ch = &app.channels[*i];
                let fav = app.state.favorites.contains(&ch.key);
                let is_playing = playing == Some(*i);
                let marker = if is_playing {
                    Span::styled("▶ ", Style::new().fg(OK).bold())
                } else if fav {
                    Span::styled("★ ", Style::new().fg(GOLD))
                } else {
                    Span::raw("  ")
                };
                let name: String = ch.name.chars().take(name_w).collect();
                let name_style = if is_playing { Style::new().fg(OK).bold() } else { Style::new().fg(FG) };
                let mut spans = vec![marker, Span::styled(format!("{name:<name_w$}"), name_style)];
                if let Some(t) = app.now_all.get(&ch.id) {
                    spans.push(Span::styled(format!(" {}", t.display()), Style::new().fg(MUTED)));
                }
                ListItem::new(Line::from(spans))
            }
        })
        .collect();

    let list = List::new(items)
        .block(blk)
        .highlight_style(Style::new().bg(Color::Rgb(28, 40, 58)).add_modifier(Modifier::BOLD))
        .highlight_symbol("")
        .scroll_padding(3);
    f.render_stateful_widget(list, area, &mut app.list);
}

fn draw_now_playing(f: &mut Frame, app: &mut App, area: Rect) {
    let playing = app.playing.is_some();
    let blk = block(if playing { "Läuft gerade" } else { "Gestoppt" });
    let inner = blk.inner(area);
    f.render_widget(blk, area);

    // cover: roughly square (terminal cells are ~2:1)
    let cover_w = (inner.height * 2 + 1).min(inner.width / 2);
    let [cover_area, _, text_area] =
        Layout::horizontal([Constraint::Length(cover_w), Constraint::Length(2), Constraint::Min(10)]).areas(inner);
    if let Some(proto) = app.cover.as_mut() {
        f.render_stateful_widget(StatefulImage::default().resize(Resize::Fit(None)), cover_area, proto);
    } else {
        f.render_widget(
            Paragraph::new("\n\n\n♪").alignment(Alignment::Center).fg(ACCENT_DIM).block(Block::bordered().border_style(Style::new().fg(ACCENT_DIM))),
            cover_area,
        );
    }

    let mut lines: Vec<Line> = Vec::new();
    let np = app.now_playing();
    if let Some(p) = &app.playing {
        let ch = &app.channels[p.channel];
        lines.push(Line::from(vec![
            Span::styled(ch.name.to_uppercase(), Style::new().fg(ACCENT).bold()),
            Span::styled(
                if p.audio { "  ● live" } else { "  ○ verbinde …" },
                Style::new().fg(if p.audio { OK } else { GOLD }),
            ),
        ]));
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            if np.title.is_empty() { "…".to_string() } else { np.title.clone() },
            Style::new().fg(FG).bold(),
        )));
        lines.push(Line::from(Span::styled(np.artist.clone(), Style::new().fg(FG))));
        if let Some(t) = &p.track {
            let mut s = vec![];
            if t.votes_up + t.votes_down > 0 {
                s.push(Span::styled(format!("▲ {}  ▼ {}", t.votes_up, t.votes_down), Style::new().fg(MUTED)));
            }
            lines.push(Line::from(s));
        } else {
            lines.push(Line::default());
        }
    } else {
        lines.push(Line::from(Span::styled("Nichts läuft.", Style::new().fg(MUTED))));
        lines.push(Line::default());
        lines.push(Line::from(vec![
            Span::styled("Enter", Style::new().fg(ACCENT).bold()),
            Span::styled(" Kanal abspielen · ", Style::new().fg(MUTED)),
            Span::styled("Leertaste", Style::new().fg(ACCENT).bold()),
            Span::styled(" letzten Kanal fortsetzen", Style::new().fg(MUTED)),
        ]));
    }

    let [text, _, gauge, _, vol] = Layout::vertical([
        Constraint::Length(lines.len() as u16),
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(text_area);
    f.render_widget(Paragraph::new(lines), text);

    if playing && np.duration > 0 && np.started > 0 {
        let elapsed = (unix_now() - np.started).clamp(0, np.duration);
        f.render_widget(
            LineGauge::default()
                .ratio(elapsed as f64 / np.duration as f64)
                .label(Span::styled(format!("{} / {}", mmss(elapsed), mmss(np.duration)), Style::new().fg(MUTED)))
                .filled_style(Style::new().fg(ACCENT))
                .unfilled_style(Style::new().fg(ACCENT_DIM)),
            gauge,
        );
    } else if playing && np.started > 0 {
        f.render_widget(Span::styled(format!("seit {}", mmss(unix_now() - np.started)), Style::new().fg(MUTED)), gauge);
    }

    let v = app.state.volume;
    let bar_w = 20usize;
    let filled = (v.min(100) as usize * bar_w) / 100;
    f.render_widget(
        Line::from(vec![
            Span::styled("Vol ", Style::new().fg(MUTED)),
            Span::styled("━".repeat(filled), Style::new().fg(ACCENT)),
            Span::styled("━".repeat(bar_w - filled), Style::new().fg(ACCENT_DIM)),
            Span::styled(format!(" {v}%"), Style::new().fg(MUTED)),
        ]),
        vol,
    );
}

fn draw_info(f: &mut Frame, app: &App, area: Rect) {
    let Some(c) = app.selected_channel() else {
        f.render_widget(block("Kanal"), area);
        return;
    };
    let ch = &app.channels[c];
    let info = app.info(ch);
    let mut text = Text::default();

    let group = app.group_of(ch);
    let mut meta = vec![Span::styled(group, Style::new().fg(ACCENT))];
    if let Some(t) = info.map(|i| i.tempo.as_str()).filter(|t| !t.is_empty()) {
        meta.push(Span::styled(format!(" · {t}"), Style::new().fg(MUTED)));
    }
    if app.state.favorites.contains(&ch.key) {
        meta.push(Span::styled("  ★ Favorit", Style::new().fg(GOLD)));
    }
    text.push_line(Line::from(meta));
    text.push_line(Line::default());

    let desc = info.map(|i| i.desc.clone()).unwrap_or_else(|| ch.description.clone());
    text.push_line(Line::from(Span::styled(desc, Style::new().fg(FG))));
    text.push_line(Line::default());

    let artists = info.map(|i| i.artists.clone()).unwrap_or_else(|| ch.artists.join(", "));
    text.push_line(Line::from(vec![
        Span::styled("Typische Artists: ", Style::new().fg(MUTED)),
        Span::styled(artists, Style::new().fg(FG)),
    ]));
    if !ch.filters.is_empty() {
        text.push_line(Line::from(vec![
            Span::styled("DI.FM-Genres:     ", Style::new().fg(MUTED)),
            Span::styled(ch.filters.join(", "), Style::new().fg(FG)),
        ]));
    }
    let similar: Vec<&str> = ch
        .similar
        .iter()
        .filter_map(|id| app.channels.iter().find(|c| c.id == *id).map(|c| c.name.as_str()))
        .take(5)
        .collect();
    if !similar.is_empty() {
        text.push_line(Line::from(vec![
            Span::styled("Ähnlich:          ", Style::new().fg(MUTED)),
            Span::styled(similar.join(", "), Style::new().fg(FG)),
        ]));
    }

    f.render_widget(
        Paragraph::new(text).wrap(Wrap { trim: true }).block(block(&ch.name).padding(Padding::horizontal(1))),
        area,
    );
}

fn draw_history(f: &mut Frame, app: &App, area: Rect) {
    let Some(c) = app.selected_channel() else {
        f.render_widget(block("Zuletzt gespielt"), area);
        return;
    };
    let ch = &app.channels[c];
    let blk = block("Zuletzt gespielt").padding(Padding::horizontal(1));
    let lines: Vec<Line> = match app.history.get(&ch.id) {
        Some((_, tracks)) => tracks
            .iter()
            .take(area.height.saturating_sub(2) as usize)
            .map(|t| {
                Line::from(vec![
                    Span::styled(format!("{:>10}  ", ago(t.started)), Style::new().fg(MUTED)),
                    Span::styled(t.display(), Style::new().fg(FG)),
                ])
            })
            .collect(),
        None => vec![Line::from(Span::styled("Lade …", Style::new().fg(MUTED)))],
    };
    f.render_widget(Paragraph::new(lines).block(blk), area);
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let mut spans = Vec::new();
    if let Some((msg, _)) = &app.status {
        spans.push(Span::styled(format!(" {msg} "), Style::new().fg(GOLD)));
    } else {
        for (k, d) in [("Enter", "Play"), ("␣", "Start/Stop"), ("n/p", "Kanal ±"), ("+/-", "Vol"), ("f", "Fav"), ("/", "Suche"), ("Tab", "Liste"), ("?", "Hilfe"), ("q", "Ende")] {
            spans.push(Span::styled(format!(" {k}"), Style::new().fg(ACCENT).bold()));
            spans.push(Span::styled(format!(" {d} "), Style::new().fg(MUTED)));
        }
    }
    let [left, right] = Layout::horizontal([Constraint::Min(10), Constraint::Length(34)]).areas(area);
    f.render_widget(Line::from(spans), left);

    let mut r = Vec::new();
    if !app.cfg.discord_app_id.is_empty() {
        let (sym, col) = match app.discord {
            Some(true) => ("●", OK),
            Some(false) => ("○", WARN),
            None => ("○", MUTED),
        };
        r.push(Span::styled(format!("Discord {sym} "), Style::new().fg(col)));
    }
    if app.cfg.overlay_port != 0 {
        r.push(Span::styled(format!("OBS :{} ", app.cfg.overlay_port), Style::new().fg(MUTED)));
    }
    r.push(Span::styled("MPRIS ● ", Style::new().fg(OK)));
    f.render_widget(Line::from(r).alignment(Alignment::Right), right);
}

fn draw_help(f: &mut Frame) {
    let area = f.area();
    let w = 62.min(area.width);
    let h = 24.min(area.height);
    let popup = Rect::new((area.width - w) / 2, (area.height - h) / 2, w, h);
    let keys = [
        ("↑/↓  j/k", "Kanal wählen"),
        ("PgUp/PgDn  g/G", "Seitenweise / Anfang / Ende"),
        ("Enter", "Kanal abspielen"),
        ("Leertaste", "Start / Stop (letzter Kanal)"),
        ("s", "Stop"),
        ("n / p", "Nächster / vorheriger Kanal der Liste"),
        ("r", "Zufälliger Kanal"),
        ("o", "Zum laufenden Kanal springen"),
        ("+ / -", "Lautstärke"),
        ("f", "Favorit an/aus"),
        ("/", "Suche (Name, Beschreibung, Artists, Genre)"),
        ("Esc", "Suche beenden"),
        ("Tab / 1–6", "Liste: Alle, Favoriten, Stimmungen"),
        ("q", "Beenden"),
    ];
    let mut lines: Vec<Line> = keys
        .iter()
        .map(|(k, d)| Line::from(vec![Span::styled(format!("{k:>16}  "), Style::new().fg(ACCENT).bold()), Span::styled(*d, Style::new().fg(FG))]))
        .collect();
    lines.push(Line::default());
    lines.push(Line::from(Span::styled("Medientasten / Plasma-Widget steuern ruDI.FM über MPRIS.", Style::new().fg(MUTED))));
    lines.push(Line::from(Span::styled("OBS: Browserquelle http://127.0.0.1:<port>/", Style::new().fg(MUTED))));
    f.render_widget(Clear, popup);
    f.render_widget(Paragraph::new(lines).block(block("Hilfe · beliebige Taste schließt").padding(Padding::uniform(1))), popup);
}
