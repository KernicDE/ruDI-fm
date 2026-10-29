use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Bar, BarChart, BarGroup, Block, BorderType, Borders, Clear, LineGauge, List, ListItem, Padding, Paragraph, Sparkline, Tabs, Wrap};
use ratatui_image::{Resize, StatefulImage};

use crate::app::{App, Row};
use crate::i18n::tr;
use crate::nowplaying::unix_now;
use crate::stats::fmt_dur;

const ACCENT: Color = Color::Rgb(63, 169, 245);
const ACCENT_DIM: Color = Color::Rgb(30, 80, 120);
const GOLD: Color = Color::Rgb(240, 190, 70);
const MUTED: Color = Color::Rgb(125, 132, 150);
const FG: Color = Color::Rgb(225, 228, 235);
const OK: Color = Color::Rgb(110, 200, 120);

fn block(title: &str) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT_DIM))
        .title(Span::styled(format!(" {title} "), Style::new().fg(ACCENT).bold()))
}

/// Winamp-style ticker: text wider than `w` scrolls to its end and back, pausing at both ends.
fn marquee(s: &str, w: usize) -> String {
    let step = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() / 200) as usize;
    marquee_at(s, w, step)
}

fn marquee_at(s: &str, w: usize, step: usize) -> String {
    const PAUSE: usize = 5;
    let n = s.chars().count();
    if w == 0 || n <= w {
        return s.to_string();
    }
    let over = n - w;
    let p = step % (2 * (over + PAUSE));
    let off = if p < over + PAUSE { p.saturating_sub(PAUSE) } else { over - (p - over - PAUSE).saturating_sub(PAUSE) };
    s.chars().skip(off).take(w).collect()
}

fn mmss(secs: i64) -> String {
    let s = secs.max(0);
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

fn ago(started: i64) -> String {
    let d = (unix_now() - started).max(0);
    match d {
        0..=59 => tr("gerade", "just now").into(),
        60..=3599 if crate::i18n::en() => format!("{} min ago", d / 60),
        60..=3599 => format!("vor {} min", d / 60),
        _ if crate::i18n::en() => format!("{} h ago", d / 3600),
        _ => format!("vor {} h", d / 3600),
    }
}

pub fn draw(f: &mut Frame, app: &mut App) {
    // short terminals fall back to the one-line player
    let mini = app.mini || f.area().height < 22;
    let bar_h = if mini { 1 } else { 4 };
    let [header, body, bar, hints] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(5), Constraint::Length(bar_h), Constraint::Length(1)]).areas(f.area());

    draw_header(f, app, header);

    let list_w = if app.tab >= 2 && app.search.is_empty() { 58 } else { 34 };
    let [left, right] = Layout::horizontal([Constraint::Length(list_w), Constraint::Min(30)]).areas(body);
    draw_list(f, app, left);

    if app.tab >= 2 && app.search.is_empty() {
        draw_show(f, app, right);
    } else {
        let [info, hist] = Layout::vertical([Constraint::Length(14), Constraint::Min(6)]).areas(right);
        draw_info(f, app, info);
        draw_history(f, app, hist);
    }
    draw_player(f, app, bar, mini);
    draw_hints(f, app, hints);

    if app.show_help {
        draw_help(f);
    }
    if app.stats.is_some() {
        draw_stats(f, app);
    }
}

fn draw_header(f: &mut Frame, app: &mut App, area: Rect) {
    let [logo, tabs] = Layout::horizontal([Constraint::Length(11), Constraint::Min(10)]).areas(area);
    f.render_widget(
        Line::from(vec![Span::styled(" ru", Style::new().fg(MUTED)), Span::styled("DI.FM ", Style::new().fg(ACCENT).bold())]),
        logo,
    );
    let titles: Vec<Line> = app.tabs.iter().enumerate().map(|(i, t)| Line::from(format!("{} {t}", i + 1))).collect();
    // Tabs pads each title with one space per side and puts a 1-cell divider between them.
    let mut x = tabs.x;
    app.hits.tabs = titles
        .iter()
        .map(|t| {
            let w = t.width() as u16 + 2;
            let r = Rect::new(x, tabs.y, w, 1).intersection(tabs);
            x += w + 1;
            r
        })
        .collect();
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
        format!("{} {}{}", tr("Suche:", "Search:"), app.search, if app.searching { "▏" } else { "" })
    } else {
        let n = app.rows.iter().filter(|r| r.is_item()).count();
        let unit = if app.tab >= 2 { tr("Shows", "shows") } else { tr("Kanäle", "channels") };
        format!("{} · {n} {unit}", app.tabs[app.tab])
    };
    let blk = block(&title);
    app.hits.list = blk.inner(area);

    if app.loading {
        f.render_widget(Paragraph::new(tr("Lade Kanäle …", "Loading channels …")).fg(MUTED).block(blk), area);
        return;
    }
    if app.rows.is_empty() {
        let msg = match (app.search.is_empty(), app.tab) {
            (true, 1) => tr("Noch keine Favoriten – mit f markieren.", "No favorites yet – press f to add."),
            (true, 2) => tr("Gerade läuft keine Show.", "No show is live right now."),
            (true, 3) => tr("Keine Shows geladen.", "No shows loaded."),
            _ => tr("Keine Treffer.", "No matches."),
        };
        f.render_widget(Paragraph::new(msg).fg(MUTED).block(blk), area);
        return;
    }

    let inner_w = area.width.saturating_sub(4) as usize;
    let name_w = inner_w.saturating_sub(1);
    let playing = app.playing.as_ref().map(|p| p.channel);
    let items: Vec<ListItem> = app
        .rows
        .iter()
        .map(|r| match r {
            Row::Header(g) => ListItem::new(Line::from(Span::styled(
                format!("── {g} "),
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
            ))),
            Row::Show(si) => {
                let sh = &app.shows[*si];
                let ch = app.channels.iter().position(|c| c.id == sh.channel_id);
                let is_playing = ch.is_some() && playing == ch;
                let marker = if is_playing {
                    Span::styled("▶ ", Style::new().fg(OK).bold())
                } else if sh.live {
                    Span::styled("● ", Style::new().fg(OK))
                } else {
                    Span::raw("  ")
                };
                // live tab: next slot with weekday; plan tab: day is in the header
                let when = match (app.tab, sh.next_start) {
                    (_, 0) => String::new(),
                    (2, t) => format!("{} {} {}", crate::app::weekday(t), crate::app::date(t), crate::app::hm(t)),
                    (_, t) => crate::app::hm(t),
                };
                let chan = ch.map(|c| app.channels[c].name.as_str()).unwrap_or_default();
                let name_style = if sh.live { Style::new().fg(OK).bold() } else { Style::new().fg(FG) };
                ListItem::new(Line::from(vec![
                    marker,
                    Span::styled(format!("{when:<16}"), Style::new().fg(MUTED)),
                    Span::styled(sh.name.clone(), name_style),
                    Span::styled(format!("  {} · {chan}", sh.tagline), Style::new().fg(MUTED)),
                ]))
            }
            Row::Channel(i) => {
                let ch = &app.channels[*i];
                let fav = app.state.favorites.contains(&ch.key);
                let is_playing = playing == Some(*i);
                let marker = vec![
                    if is_playing { Span::styled("▶", Style::new().fg(OK).bold()) } else { Span::raw(" ") },
                    if fav { Span::styled("★ ", Style::new().fg(GOLD)) } else { Span::raw("  ") },
                ];
                let name: String = ch.name.chars().take(name_w).collect();
                let name_style = if is_playing { Style::new().fg(OK).bold() } else { Style::new().fg(FG) };
                ListItem::new(Line::from([marker, vec![Span::styled(name, name_style)]].concat()))
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

const BTN_W: u16 = 5;
const VOL_BAR: u16 = 12;

/// Areas of the transport row; kept pure so tests can check every terminal width.
struct Controls {
    btn: [Rect; 3],
    gauge: Rect,
    /// Whole volume readout and the clickable bar inside it (empty when too narrow).
    vol: Rect,
    vol_bar: Rect,
}

fn controls(row: Rect) -> Controls {
    let at = |x: u16, w: u16| Rect::new(x, row.y, w, 1).intersection(row);
    let btn = std::array::from_fn(|i| at(row.x + i as u16 * (BTN_W + 1), BTN_W));
    let wide = row.width >= 70;
    let vol_w = if wide { 4 + VOL_BAR + 5 } else { 5 };
    let vol = at(row.right().saturating_sub(vol_w), vol_w);
    let vol_bar = if wide { at(vol.x + 4, VOL_BAR) } else { Rect::default() };
    let gx = row.x + 3 * BTN_W + 2 + 2;
    let gauge = at(gx, vol.x.saturating_sub(gx + 2));
    Controls { btn, gauge, vol, vol_bar }
}

fn draw_player(f: &mut Frame, app: &mut App, area: Rect, mini: bool) {
    app.hits.now = area;
    app.hits.ctl = Default::default();
    app.hits.vol = Rect::default();
    let playing = app.playing.is_some();
    let np = app.now_playing();
    let v = app.state.volume;
    let elapsed = if playing && np.started > 0 { Some(unix_now() - np.started) } else { None };
    let time = match (elapsed, np.duration) {
        (Some(e), d) if d > 0 => format!("{} / {}", mmss(e.clamp(0, d)), mmss(d)),
        (Some(e), _) => format!("{} {}", tr("seit", "for"), mmss(e)),
        _ => String::new(),
    };
    let heard = match &app.playing {
        Some(p) if np.title.is_empty() => "…".to_string(),
        Some(_) if np.artist.is_empty() => np.title.clone(),
        Some(_) => format!("{} – {}", np.title, np.artist),
        None => String::new(),
    };
    let name = app.playing.as_ref().map(|p| app.channels[p.channel].name.to_uppercase()).unwrap_or_default();
    let audio = app.playing.as_ref().is_some_and(|p| p.audio);

    if mini {
        let left = if playing { format!(" {} {name}  ", if audio { "▶" } else { "○" }) } else { format!(" ■ {}", tr("Gestoppt – Enter spielt", "Stopped – Enter plays")) };
        let right = if playing { format!("  {time}  {v}% ") } else { format!("  {v}% ") };
        let [l, mid, r] = Layout::horizontal([
            Constraint::Length(left.chars().count() as u16),
            Constraint::Min(0),
            Constraint::Length(right.chars().count() as u16),
        ])
        .areas(area);
        f.render_widget(Span::styled(left, Style::new().fg(if audio { OK } else { MUTED }).bold()), l);
        f.render_widget(Span::styled(marquee(&heard, mid.width as usize), Style::new().fg(FG).bold()), mid);
        f.render_widget(Span::styled(right, Style::new().fg(MUTED)), r);
        return;
    }

    let blk = Block::default().borders(Borders::TOP).border_style(Style::new().fg(ACCENT_DIM));
    let inner = blk.inner(area);
    f.render_widget(blk, area);
    let [cover, _, text] = Layout::horizontal([Constraint::Length(6), Constraint::Length(2), Constraint::Min(10)]).areas(inner);
    if let Some(proto) = app.cover.as_mut() {
        f.render_stateful_widget(StatefulImage::default().resize(Resize::Fit(None)), cover, proto);
    } else {
        f.render_widget(Paragraph::new("\n♪").alignment(Alignment::Center).fg(ACCENT_DIM), cover);
    }
    let [r0, r1, r2] = Layout::vertical([Constraint::Length(1); 3]).areas(text);

    let mut head = vec![];
    if let Some(p) = &app.playing {
        head.push(Span::styled(name, Style::new().fg(ACCENT).bold()));
        head.push(Span::styled(
            if p.audio { "  ● live" } else { tr("  ○ verbinde …", "  ○ connecting …") },
            Style::new().fg(if p.audio { OK } else { GOLD }),
        ));
    } else {
        head.push(Span::styled(tr("Gestoppt", "Stopped"), Style::new().fg(MUTED).bold()));
    }
    f.render_widget(Line::from(head), r0);
    if let Some(t) = app.playing.as_ref().and_then(|p| p.track.as_ref()).filter(|t| t.votes_up + t.votes_down > 0) {
        f.render_widget(Line::from(Span::styled(format!("▲ {}  ▼ {}", t.votes_up, t.votes_down), Style::new().fg(MUTED))).alignment(Alignment::Right), r0);
    }
    if playing {
        f.render_widget(Span::styled(marquee(&heard, r1.width as usize), Style::new().fg(FG).bold()), r1);
    } else {
        f.render_widget(
            Span::styled(tr("Enter spielt den gewählten Kanal · Leertaste setzt fort", "Enter plays the selected channel · Space resumes"), Style::new().fg(MUTED)),
            r1,
        );
    }

    let c = controls(r2);
    let labels = ["◀◀", if playing { "■" } else { "▶" }, "▶▶"];
    for (i, l) in labels.iter().enumerate() {
        f.render_widget(Paragraph::new(*l).alignment(Alignment::Center).style(Style::new().fg(ACCENT).bold()), c.btn[i]);
    }
    app.hits.ctl = c.btn;
    if !time.is_empty() && c.gauge.width >= time.len() as u16 {
        if np.duration > 0 && c.gauge.width >= 20 {
            let ratio = elapsed.unwrap_or(0).clamp(0, np.duration) as f64 / np.duration as f64;
            f.render_widget(
                LineGauge::default()
                    .ratio(ratio)
                    .label(Span::styled(time, Style::new().fg(MUTED)))
                    .filled_style(Style::new().fg(ACCENT))
                    .unfilled_style(Style::new().fg(ACCENT_DIM)),
                c.gauge,
            );
        } else {
            f.render_widget(Span::styled(time, Style::new().fg(MUTED)), c.gauge);
        }
    }
    if c.vol_bar.width > 0 {
        app.hits.vol = c.vol_bar;
        let filled = (v.min(100) as usize * VOL_BAR as usize) / 100;
        f.render_widget(
            Line::from(vec![
                Span::styled("Vol ", Style::new().fg(MUTED)),
                Span::styled("━".repeat(filled), Style::new().fg(ACCENT)),
                Span::styled("━".repeat(VOL_BAR as usize - filled), Style::new().fg(ACCENT_DIM)),
                Span::styled(format!(" {v:>3}%"), Style::new().fg(MUTED)),
            ]),
            c.vol,
        );
    } else {
        f.render_widget(Span::styled(format!("{v:>4}%"), Style::new().fg(MUTED)), c.vol);
    }
}

fn draw_show(f: &mut Frame, app: &App, area: Rect) {
    let Some(sh) = app.selected_show() else {
        f.render_widget(block(tr("Show", "Show")), area);
        return;
    };
    let label = |s: &'static str| Span::styled(s, Style::new().fg(MUTED));
    let chan = app.channels.iter().find(|c| c.id == sh.channel_id).map_or("", |c| c.name.as_str());
    let mut text = Text::default();
    text.push_line(Line::from(vec![Span::styled(sh.tagline.clone(), Style::new().fg(ACCENT)), Span::styled(format!(" · {chan}"), Style::new().fg(MUTED))]));
    if sh.live {
        text.push_line(Line::from(Span::styled(tr("● Läuft gerade", "● Live now"), Style::new().fg(OK).bold())));
    }
    if sh.next_start > 0 {
        text.push_line(Line::from(vec![
            label(tr("Nächste Sendung: ", "Next broadcast: ")),
            Span::styled(format!("{} {} {}", crate::app::weekday(sh.next_start), crate::app::date(sh.next_start), crate::app::hm(sh.next_start)), Style::new().fg(FG)),
        ]));
    }
    if !sh.schedule.is_empty() {
        text.push_line(Line::from(vec![label(tr("Rhythmus:        ", "Schedule:        ")), Span::styled(sh.schedule.clone(), Style::new().fg(FG))]));
    }
    if sh.episodes > 0 {
        text.push_line(Line::from(vec![label(tr("Episoden:        ", "Episodes:        ")), Span::styled(sh.episodes.to_string(), Style::new().fg(FG))]));
    }
    text.push_line(Line::default());
    let desc = match app.show_desc.get(&sh.slug) {
        Some(d) if !d.is_empty() => d.as_str(),
        Some(_) => tr("Keine Beschreibung.", "No description."),
        None => tr("Lade Beschreibung …", "Loading description …"),
    };
    for l in desc.lines() {
        text.push_line(Line::from(Span::styled(l.to_string(), Style::new().fg(FG))));
    }
    text.push_line(Line::default());
    text.push_line(Line::from(Span::styled(tr("Enter: Kanal in „Alle“ öffnen und abspielen", "Enter: open channel in “All” and play"), Style::new().fg(MUTED))));
    f.render_widget(
        Paragraph::new(text).wrap(Wrap { trim: true }).block(block(&sh.name).padding(Padding::horizontal(1))),
        area,
    );
}

const LABEL_W: usize = 18;

/// Greedy word wrap.
fn wrap(text: &str, w: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        match lines.last_mut() {
            Some(l) if l.chars().count() + 1 + word.chars().count() <= w => {
                l.push(' ');
                l.push_str(word);
            }
            _ => lines.push(word.to_string()),
        }
    }
    lines
}

/// `label  value` with the wrapped value indented under itself.
fn labeled(label: &str, value: &str, width: usize) -> Vec<Line<'static>> {
    let lines = wrap(value, width.saturating_sub(LABEL_W).max(10));
    lines
        .into_iter()
        .enumerate()
        .map(|(i, l)| {
            let head = if i == 0 { format!("{label:<LABEL_W$}") } else { " ".repeat(LABEL_W) };
            Line::from(vec![Span::styled(head, Style::new().fg(MUTED)), Span::styled(l, Style::new().fg(FG))])
        })
        .collect()
}

fn draw_info(f: &mut Frame, app: &App, area: Rect) {
    let Some(c) = app.selected_channel() else {
        f.render_widget(block(tr("Kanal", "Channel")), area);
        return;
    };
    let ch = &app.channels[c];
    let info = app.info(ch);
    let mut text = Text::default();

    let group = app.group_of(ch);
    let mut meta = Vec::new();
    if group != ch.name {
        meta.push(Span::styled(group, Style::new().fg(ACCENT)));
    }
    if let Some(t) = info.map(|i| i.tempo.as_str()).filter(|t| !t.is_empty()) {
        meta.push(Span::styled(format!("{}{t}", if meta.is_empty() { "" } else { " · " }), Style::new().fg(MUTED)));
    }
    if app.state.favorites.contains(&ch.key) {
        meta.push(Span::styled(format!("{}{}", if meta.is_empty() { "" } else { "  " }, tr("★ Favorit", "★ Favorite")), Style::new().fg(GOLD)));
    }
    if !meta.is_empty() {
        text.push_line(Line::from(meta));
        text.push_line(Line::default());
    }

    let desc = app.desc(ch).to_string();
    text.push_line(Line::from(Span::styled(desc, Style::new().fg(FG))));
    text.push_line(Line::default());

    let width = area.width.saturating_sub(4) as usize;
    let artists = info.map(|i| i.artists.clone()).unwrap_or_else(|| ch.artists.join(", "));
    text.extend(labeled(tr("Typische Artists:", "Typical artists:"), &artists, width));
    if !ch.filters.is_empty() {
        text.extend(labeled(tr("DI.FM-Genres:", "DI.FM genres:"), &ch.filters.join(", "), width));
    }
    let similar: Vec<&str> = ch
        .similar
        .iter()
        .filter_map(|id| app.channels.iter().find(|c| c.id == *id).map(|c| c.name.as_str()))
        .take(5)
        .collect();
    if !similar.is_empty() {
        text.extend(labeled(tr("Ähnlich:", "Similar:"), &similar.join(", "), width));
    }

    f.render_widget(
        Paragraph::new(text).wrap(Wrap { trim: true }).block(block(&ch.name).padding(Padding::horizontal(1))),
        area,
    );
}

fn draw_history(f: &mut Frame, app: &App, area: Rect) {
    let Some(c) = app.selected_channel() else {
        f.render_widget(block(tr("Zuletzt gespielt", "Recently played")), area);
        return;
    };
    let ch = &app.channels[c];
    let title = format!("{} {}", tr("Zuletzt auf", "Recently on"), ch.name);
    let blk = block(&title).padding(Padding::horizontal(1));
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
        None => vec![Line::from(Span::styled(tr("Lade …", "Loading …"), Style::new().fg(MUTED)))],
    };
    f.render_widget(Paragraph::new(lines).block(blk), area);
}

fn draw_hints(f: &mut Frame, app: &App, area: Rect) {
    if let Some((msg, _)) = &app.status {
        f.render_widget(Span::styled(format!(" {msg} "), Style::new().fg(GOLD)), area);
        return;
    }
    let keys: Vec<(&str, &str)> = if app.searching {
        vec![("Enter", tr("Spielen", "Play")), ("Esc", tr("Suche beenden", "End search"))]
    } else if app.tab >= 2 {
        vec![("Enter", tr("Kanal anhören", "Listen")), ("␣", tr("Start/Stop", "Start/Stop")), ("/", tr("Suche", "Search")), ("?", tr("Hilfe", "Help"))]
    } else {
        vec![("Enter", tr("Spielen", "Play")), ("␣", tr("Start/Stop", "Start/Stop")), ("f", tr("Favorit", "Favorite")), ("/", tr("Suche", "Search")), ("?", tr("Hilfe", "Help"))]
    };
    let spans: Vec<Span> = keys
        .into_iter()
        .flat_map(|(k, d)| [Span::styled(format!(" {k}"), Style::new().fg(ACCENT).bold()), Span::styled(format!(" {d} "), Style::new().fg(MUTED))])
        .collect();
    f.render_widget(Line::from(spans), area);
}

fn draw_help(f: &mut Frame) {
    let area = f.area();
    let w = 62.min(area.width);
    let h = 27.min(area.height);
    let popup = Rect::new((area.width - w) / 2, (area.height - h) / 2, w, h);
    let keys = [
        ("↑/↓  j/k", tr("Kanal wählen", "Select channel")),
        ("PgUp/PgDn  g/G", tr("Seitenweise / Anfang / Ende", "Page / start / end")),
        ("Enter", tr("Kanal abspielen", "Play channel")),
        (tr("Leertaste", "Space"), tr("Start / Stop (letzter Kanal)", "Start / stop (last channel)")),
        ("s", tr("Stoppen", "Stop")),
        ("n / p", tr("Nächster / vorheriger Kanal der Liste", "Next / previous channel in list")),
        ("r", tr("Zufälliger Kanal", "Random channel")),
        ("o", tr("Zum laufenden Kanal springen", "Jump to playing channel")),
        ("+ / -", tr("Lautstärke", "Volume")),
        ("f", tr("Favorit an/aus", "Toggle favorite")),
        ("/", tr("Suche (Name, Beschreibung, Artists, Genre)", "Search names, artists, genres")),
        ("Esc", tr("Suche beenden", "End search")),
        ("Tab / 1–4", tr("Liste: Alle, Favoriten, Live Shows, Show Plan", "List: all, favorites, live shows, show plan")),
        ("m", tr("Mini-Player (eine Zeile) an/aus", "Toggle one-line player")),
        ("S", tr("Hörstatistik (Tab Seite, ←/→ Zeitraum)", "Listening stats (Tab page, ←/→ period)")),
        (tr("Maus", "Mouse"), tr("Klick wählt, Doppelklick spielt, Rad", "click selects, double-click plays, wheel")),
        ("q", tr("Beenden", "Quit")),
    ];
    let mut lines: Vec<Line> = keys
        .iter()
        .map(|(k, d)| Line::from(vec![Span::styled(format!("{k:>16}  "), Style::new().fg(ACCENT).bold()), Span::styled(*d, Style::new().fg(FG))]))
        .collect();
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(tr("Medientasten / Plasma-Widget steuern ruDI.FM über MPRIS.", "Media keys / Plasma widget control ruDI.FM via MPRIS."), Style::new().fg(MUTED))));
    lines.push(Line::from(Span::styled(tr("OBS: Browserquelle http://127.0.0.1:<port>/", "OBS: browser source http://127.0.0.1:<port>/"), Style::new().fg(MUTED))));
    f.render_widget(Clear, popup);
    f.render_widget(Paragraph::new(lines).block(block(tr("Hilfe · beliebige Taste schließt", "Help · any key closes")).padding(Padding::uniform(1))), popup);
}

fn draw_stats(f: &mut Frame, app: &App) {
    let Some(v) = &app.stats else { return };
    let (d, now) = (&v.data, unix_now());
    let area = f.area();
    f.render_widget(Clear, area);
    let blk = block(tr("Hörstatistik · Tab Seite · ←/→ Zeitraum · Esc zurück", "Listening stats · Tab page · ←/→ period · Esc back"));
    let inner = blk.inner(area);
    f.render_widget(blk, area);

    let [sum, periods, pages, list, charts] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(5),
        Constraint::Length(7),
    ])
    .areas(inner);

    let favourite = d.channels.first().map(|c| c.0.as_str()).unwrap_or("–");
    let kv = |k: &str, val: String| [Span::styled(format!("{k} "), Style::new().fg(MUTED)), Span::styled(format!("{val}   "), Style::new().fg(FG).bold())];
    let line: Vec<Span> = [
        kv(tr("Hörzeit", "Listened"), fmt_dur(d.secs)),
        kv("Tracks", d.tracks.to_string()),
        kv(tr("Längste Session", "Longest session"), fmt_dur(d.longest_session)),
        kv(tr("Serie", "Streak"), format!("{} {}", d.streak, tr("Tage", "days"))),
        kv(tr("Lieblingskanal", "Top channel"), favourite.to_string()),
    ]
    .into_iter()
    .flatten()
    .collect();
    f.render_widget(Paragraph::new(Line::from(line)).wrap(Wrap { trim: true }), sum);

    let tab_style = Style::new().fg(ACCENT).bold().add_modifier(Modifier::UNDERLINED);
    f.render_widget(
        Tabs::new(crate::stats::Period::ALL.iter().map(|p| Line::from(p.label(now))))
            .select(v.period)
            .style(Style::new().fg(MUTED))
            .highlight_style(tab_style)
            .divider(Span::styled("│", Style::new().fg(ACCENT_DIM))),
        periods,
    );
    f.render_widget(
        Tabs::new([tr("Kanäle", "Channels"), tr("Künstler", "Artists"), "Tracks"].map(Line::from))
            .select(v.page)
            .style(Style::new().fg(MUTED))
            .highlight_style(tab_style)
            .divider(Span::styled("│", Style::new().fg(ACCENT_DIM))),
        pages,
    );

    let ranking = [&d.channels, &d.artists, &d.top_tracks][v.page];
    let top = ranking.first().map(|r| r.1).unwrap_or(0).max(1);
    let note_w = if v.page == 0 { 0 } else { (list.width as usize / 4).clamp(8, 28) };
    let name_w = (list.width as usize).saturating_sub(34 + note_w).clamp(10, 60);
    let rows: Vec<Line> = ranking
        .iter()
        .take(list.height as usize)
        .enumerate()
        .map(|(i, (name, secs, n, chans))| {
            let shown: String = name.chars().take(name_w).collect();
            Line::from(vec![
                Span::styled(format!("{:>2} ", i + 1), Style::new().fg(MUTED)),
                Span::styled(format!("{shown:<name_w$} "), Style::new().fg(FG)),
                Span::styled(format!("{:>8} ", fmt_dur(*secs)), Style::new().fg(GOLD)),
                Span::styled(format!("{n:>4}x "), Style::new().fg(MUTED)),
                Span::styled("█".repeat((*secs * 12 / top) as usize), Style::new().fg(ACCENT)),
                Span::styled(format!(" {}", chans.chars().take(note_w).collect::<String>()), Style::new().fg(MUTED)),
            ])
        })
        .collect();
    let empty = || vec![Line::from(Span::styled(tr("Keine Daten in diesem Zeitraum.", "No data in this period."), Style::new().fg(MUTED)))];
    f.render_widget(Paragraph::new(if rows.is_empty() { empty() } else { rows }), list);

    let [hours, days] = Layout::horizontal([Constraint::Length(50), Constraint::Min(16)]).areas(charts);
    let bars: Vec<Bar> = d
        .hours
        .iter()
        .enumerate()
        .map(|(h, s)| Bar::default().value(*s).label(Line::from(format!("{h:02}"))).text_value(String::new()))
        .collect();
    f.render_widget(
        BarChart::default()
            .block(block(tr("Tageszeit", "Time of day")))
            .data(BarGroup::default().bars(&bars))
            .bar_width(2)
            .bar_gap(0)
            .bar_style(Style::new().fg(ACCENT))
            .label_style(Style::new().fg(MUTED)),
        hours,
    );
    f.render_widget(Sparkline::default().block(block(tr("Letzte 14 Tage", "Last 14 days"))).data(d.days).style(Style::new().fg(GOLD)), days);
}

#[cfg(test)]
mod tests {
    use super::{controls, marquee_at, wrap};
    use ratatui::layout::Rect;

    #[test]
    fn controls_fit_every_width() {
        for w in 10..300 {
            let row = Rect::new(3, 5, w, 1);
            let c = controls(row);
            for r in c.btn.iter().chain([&c.gauge, &c.vol, &c.vol_bar]) {
                assert!(r.is_empty() || row.intersection(*r) == *r, "width {w}: {r:?} leaves {row:?}");
            }
            assert!(c.gauge.width == 0 || c.gauge.right() <= c.vol.x, "width {w}: gauge overlaps volume");
            if w >= 24 {
                assert!(c.btn.iter().all(|b| b.width == super::BTN_W), "width {w}: button clipped");
                assert!(c.btn[2].right() <= c.vol.x, "width {w}: buttons overlap volume");
            }
        }
    }

    #[test]
    fn wrap_breaks_at_words() {
        assert_eq!(wrap("aa bb cc dd", 5), ["aa bb", "cc dd"]);
        assert_eq!(wrap("langeswort", 3), ["langeswort"]);
    }

    #[test]
    fn marquee_bounces_between_both_ends() {
        let s = "0123456789";
        assert_eq!(marquee_at(s, 20, 7), s);
        let frames: Vec<String> = (0..40).map(|i| marquee_at(s, 6, i)).collect();
        assert!(frames.iter().all(|f| f.chars().count() == 6));
        assert_eq!(frames[0], "012345");
        assert!(frames.contains(&"456789".to_string()));
        assert_eq!(frames[0], frames[(4 + 5) * 2 % 40 .. ][0]);
    }
}
