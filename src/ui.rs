//! Rendering TUI (ratatui).

use crate::app::{App, Side, Screen};
use crate::transfer::{Direction, JobKind, Status};
use crate::util::{fmt_date, humanize};
use ratatui::layout::{Constraint, Direction as LDirection, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Scrollbar, ScrollbarOrientation,
    ScrollbarState, Wrap,
};
use ratatui::Frame;
use std::collections::HashSet;

/// Colore identificativo del pannello: blu = computer (Locale), verde = device Android.
fn accent(side: Side) -> Color {
    match side {
        Side::Local => Color::Rgb(97, 175, 239),
        Side::Remote => Color::Rgb(152, 195, 121),
    }
}

const DIM: Color = Color::Rgb(110, 118, 135);
const SOFT: Color = Color::Rgb(150, 158, 175);
const ROW_INACTIVE: Color = Color::Rgb(46, 52, 64);
const MARK: Color = Color::Rgb(229, 192, 123);

/// Icona e colore del nome in base al tipo di file.
fn kind_style(name: &str, is_dir: bool, side: Side) -> (&'static str, Color) {
    if is_dir {
        return ("▸", accent(side));
    }
    let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "heic" | "bmp" | "dng" => ("◈", Color::Rgb(198, 120, 221)),
        "mp4" | "mkv" | "mov" | "avi" | "3gp" | "webm" => ("▶", Color::Rgb(224, 108, 117)),
        "mp3" | "m4a" | "flac" | "wav" | "ogg" | "opus" | "aac" | "amr" => ("♪", Color::Rgb(86, 182, 194)),
        "zip" | "tar" | "gz" | "7z" | "rar" | "xz" | "bz2" => ("▤", Color::Rgb(209, 154, 102)),
        "apk" | "xapk" => ("◆", Color::Rgb(152, 195, 121)),
        "pdf" | "doc" | "docx" | "txt" | "md" | "odt" | "xls" | "xlsx" | "ppt" | "pptx" | "csv" | "json" => {
            ("≡", Color::Rgb(220, 223, 228))
        }
        _ => ("·", Color::Rgb(171, 178, 191)),
    }
}

/// Barra di avanzamento testuale.
fn bar(pct: u64, width: usize) -> String {
    let filled = (pct.min(100) as usize * width) / 100;
    format!("{}{}", "█".repeat(filled), "░".repeat(width - filled))
}

fn status_glyph(s: Status) -> (&'static str, Color) {
    match s {
        Status::Queued => ("…", DIM),
        Status::Running => ("▶", Color::Cyan),
        Status::Done => ("✔", Color::Green),
        Status::Failed => ("✖", Color::Red),
        Status::Cancelled => ("⊘", Color::Yellow),
    }
}

fn trunc(s: &str, w: usize) -> String {
    if w == 0 {
        return String::new();
    }
    if s.chars().count() <= w {
        s.to_string()
    } else {
        let c: String = s.chars().take(w.saturating_sub(1)).collect();
        format!("{c}…")
    }
}

pub fn draw(f: &mut Frame, app: &mut App) {
    match &app.screen {
        Screen::Devices { sel } => draw_devices(f, app, *sel),
        Screen::Files => draw_files(f, app),
        Screen::Help => {
            draw_files(f, app);
            draw_help(f);
        }
        Screen::Menu { .. } => {
            draw_files(f, app);
            draw_menu(f, app);
        }
        Screen::Goto { .. } => {
            draw_files(f, app);
            draw_goto(f, app);
        }
        Screen::Confirm { .. } | Screen::Input { .. } => {
            draw_files(f, app);
            draw_dialog(f, app);
        }
    }
}

fn header(f: &mut Frame, area: Rect, app: &App) {
    let connected = app.info.is_some();
    let info = app.info.clone().unwrap_or_else(|| "nessun device connesso".into());
    let left = Line::from(vec![
        Span::styled(
            " ◉ ABT ",
            Style::default().fg(Color::Black).bg(accent(Side::Remote)).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            if connected { "●" } else { "○" },
            Style::default().fg(if connected { accent(Side::Remote) } else { Color::Rgb(224, 108, 117) }),
        ),
        Span::raw(" "),
        Span::styled(info, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
    ]);
    f.render_widget(Paragraph::new(left), area);
    if matches!(app.screen, Screen::Devices { .. }) {
        return;
    }
    let side = app.active;
    let (name, other) = if side == Side::Local { ("LOCALE", "DEVICE") } else { ("DEVICE", "LOCALE") };
    let right = Line::from(vec![
        Span::styled(format!(" {other} "), Style::default().fg(DIM)),
        Span::styled(" ⇄ ", Style::default().fg(DIM)),
        Span::styled(
            format!(" ▌{name} "),
            Style::default().fg(Color::Black).bg(accent(side)).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
    ]);
    f.render_widget(Paragraph::new(right).alignment(ratatui::layout::Alignment::Right), area);
}

fn footer(f: &mut Frame, area: Rect, text: &str) {
    let w = area.width as usize;
    let line = Line::from(Span::styled(trunc(text, w.saturating_sub(1)), Style::default().fg(Color::Gray)));
    f.render_widget(Paragraph::new(line), area);
}

/// Barra comandi: tasto evidenziato + etichetta, su più righe.
fn hints(f: &mut Frame, area: Rect, rows: &[&[(&str, &str)]]) {
    let lines: Vec<Line> = rows
        .iter()
        .map(|row| {
            let mut spans = vec![Span::raw(" ")];
            for (k, l) in row.iter() {
                spans.push(Span::styled(
                    format!(" {k} "),
                    Style::default().fg(Color::White).bg(Color::Rgb(62, 68, 82)).add_modifier(Modifier::BOLD),
                ));
                spans.push(Span::styled(format!(" {l}  "), Style::default().fg(SOFT)));
            }
            Line::from(spans)
        })
        .collect();
    f.render_widget(Paragraph::new(lines), area);
}

/// Titolo del pannello con il percorso a segmenti sottolineati (cliccabili).
fn spans_title(label: &str, cwd: &str, extra: &str, side: Side, active: bool) -> Vec<Span<'static>> {
    // il prefisso " {label}: " ha larghezza fissa (usata dall'hit-test del mouse)
    let badge = if active {
        Style::default().fg(Color::Black).bg(accent(side)).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(accent(side))
    };
    let mut v = vec![Span::styled(format!(" {}: ", label.to_uppercase()), badge)];
    let chars: Vec<char> = cwd.chars().collect();
    let link = Style::default().fg(if active { Color::White } else { SOFT }).add_modifier(Modifier::UNDERLINED);
    let plain = Style::default().fg(if active { Color::White } else { SOFT });
    let mut pos = 0usize;
    for (a, b, _) in crate::places::crumbs(cwd) {
        if a > pos {
            v.push(Span::styled(chars[pos..a].iter().collect::<String>(), plain));
        }
        v.push(Span::styled(chars[a..b.min(chars.len())].iter().collect::<String>(), link));
        pos = b.min(chars.len());
    }
    if pos < chars.len() {
        v.push(Span::styled(chars[pos..].iter().collect::<String>(), plain));
    }
    v.push(Span::styled(format!(" {extra}"), Style::default().fg(MARK)));
    v
}

fn draw_goto(f: &mut Frame, app: &App) {
    let Screen::Goto { items, sel, pos, side } = &app.screen else { return };
    let maxw = items.iter().map(|i| i.0.chars().count()).max().unwrap_or(10);
    let r = crate::app::popup_rect(maxw, items.len(), *pos, f.area());
    f.render_widget(Clear, r);
    let rows: Vec<ListItem> = items.iter().map(|(l, _)| ListItem::new(Line::from(format!(" {l} ")))).collect();
    let mut st = ListState::default();
    st.select(Some(*sel));
    let title = if *side == Side::Local { " Vai a… (Locale) " } else { " Vai a… (Device) " };
    let list = List::new(rows)
        .block(
            Block::default()
                .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(accent(*side)))
                .title(title)
                .title_bottom(" ▸ luoghi · ★ preferiti (B) · ↺ recenti "),
        )
        .highlight_style(Style::default().fg(Color::Black).bg(accent(*side)));
    f.render_stateful_widget(list, r, &mut st);
}

fn draw_menu(f: &mut Frame, app: &App) {
    let Screen::Menu { items, sel, pos, title } = &app.screen else { return };
    let r = crate::app::menu_rect(items, *pos, f.area());
    f.render_widget(Clear, r);
    let rows: Vec<ListItem> = items
        .iter()
        .map(|(l, _)| ListItem::new(Line::from(format!(" {l} "))))
        .collect();
    let mut st = ListState::default();
    st.select(Some(*sel));
    let list = List::new(rows)
        .block(
            Block::default()
                .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(accent(app.active)))
                .title(format!(" {title} ")),
        )
        .highlight_style(Style::default().fg(Color::Black).bg(accent(app.active)));
    f.render_stateful_widget(list, r, &mut st);
}

fn draw_help(f: &mut Frame) {
    let area = centered_rect(80, 85, f.area());
    f.render_widget(Clear, area);
    let rows: &[(&str, &str)] = &[
        ("NAVIGAZIONE", ""),
        ("Tab / Shift+Tab", "cambia pannello"),
        ("↑↓  j k   PgUp PgDn", "muovi cursore · Home/G inizio/fine"),
        ("g  (o click sul titolo)", "Vai a…: luoghi, preferiti, recenti, percorso a mano"),
        ("click sul percorso", "clic su un segmento del path in alto per saltarci"),
        ("B", "aggiungi/rimuovi la cartella corrente dai preferiti"),
        ("→ l   ← h Backspace", "entra in cartella · cartella padre"),
        ("Invio", "cartella: entra · file: trasferisce nell'altro pannello"),
        ("/", "filtra per nome (Esc rimuove il filtro)"),
        ("o   .", "ordina nome/dimensione/data · mostra nascosti"),
        ("r", "aggiorna pannello"),
        ("SELEZIONE", ""),
        ("Spazio / Ins", "marca/smarca e scende · J/K o Shift+↑↓ estendi"),
        ("a   *   Esc", "seleziona tutto · inverti · azzera selezione"),
        ("FILE E CARTELLE", ""),
        ("y   p", "pull / push: chiede la cartella di destinazione (Invio conferma)"),
        ("T  (o y/p su ..)", "trasferisce l'INTERA cartella corrente"),
        ("s", "sync incrementale della cartella device → locale"),
        ("n  (F7)", "nuova cartella"),
        ("R  (F2)", "rinomina"),
        ("d  (Canc)", "elimina (con conferma)"),
        ("ALTRO", ""),
        ("click destro  (m)", "menu contestuale: copia, cartella intera, rinomina, elimina…"),
        ("click sx · doppio click", "seleziona · apri cartella / trasferisci file · rotella scorre"),
        ("S", "esporta SMS + registro chiamate"),
        ("x", "annulla il trasferimento attivo"),
        ("q  Ctrl+C", "esci"),
    ];
    let lines: Vec<Line> = rows
        .iter()
        .map(|(k, l)| {
            if l.is_empty() {
                Line::from(Span::styled(
                    format!(" {k}"),
                    Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
                ))
            } else {
                Line::from(vec![
                    Span::styled(format!("   {:<24}", k), Style::default().fg(accent(Side::Remote))),
                    Span::raw((*l).to_string()),
                ])
            }
        })
        .collect();
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(accent(Side::Remote)))
                .title(" Comandi — qualsiasi tasto per chiudere "),
        ),
        area,
    );
}

fn draw_devices(f: &mut Frame, app: &mut App, sel: usize) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(LDirection::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(3), Constraint::Length(1)])
        .split(area);
    header(f, chunks[0], app);

    let mut main_area = chunks[1];
    if main_area.height >= 18 {
        let logo: Vec<Line> = crate::util::LOGO
            .iter()
            .map(|l| Line::from(Span::styled(*l, Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))))
            .chain(std::iter::once(Line::from(Span::styled(
                "Android Backup Tool",
                Style::default().fg(Color::Green),
            ))))
            .collect();
        let lr = Rect { x: main_area.x, y: main_area.y, width: main_area.width, height: crate::util::LOGO.len() as u16 + 1 };
        f.render_widget(
            Paragraph::new(logo).alignment(ratatui::layout::Alignment::Center),
            lr,
        );
        main_area = Rect {
            x: main_area.x,
            y: main_area.y + crate::util::LOGO.len() as u16 + 2,
            width: main_area.width,
            height: main_area.height - (crate::util::LOGO.len() as u16 + 2),
        };
    }

    if app.devices.is_empty() {
        let msg = "nessun device — collega via USB (attiva debug USB) oppure premi 'c' per adb connect IP:5555";
        f.render_widget(
            Paragraph::new(msg)
                .alignment(ratatui::layout::Alignment::Center)
                .wrap(Wrap { trim: true }),
            main_area,
        );
    } else {
        let items: Vec<ListItem> = app
            .devices
            .iter()
            .map(|d| {
                let state_style = if d.state == "device" {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default().fg(Color::Red)
                };
                ListItem::new(Line::from(vec![
                    Span::styled(format!(" {:<24}", d.serial), Style::default().fg(accent(Side::Remote))),
                    Span::styled(format!("{:<14}", d.state), state_style),
                    Span::styled(
                        d.model.clone().unwrap_or_default(),
                        Style::default().fg(DIM),
                    ),
                ]))
            })
            .collect();
        let mut ls = ListState::default();
        ls.select(Some(sel.min(app.devices.len().saturating_sub(1))));
        let list = List::new(items)
            .block(Block::default().borders(Borders::ALL)
        .border_type(BorderType::Rounded).title(" Seleziona device "))
            .highlight_style(Style::default().fg(Color::Black).bg(accent(Side::Remote)).add_modifier(Modifier::BOLD))
            .highlight_symbol("▌");
        f.render_stateful_widget(list, main_area, &mut ls);
    }
    footer(
        f,
        chunks[2],
        "↑↓/j-k muovi · Invio connetti · c aggiungi Wi-Fi (adb connect) · r aggiorna · q esci",
    );
}

fn draw_files(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(LDirection::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(4),
            Constraint::Length(5),
            Constraint::Length(4),
            Constraint::Length(2),
        ])
        .split(area);
    header(f, chunks[0], app);

    let panes = Layout::default()
        .direction(LDirection::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[1]);
    app.pane_rects = [panes[0], panes[1]];
    let active_local = app.active == Side::Local;
    draw_pane(f, panes[0], &mut app.local, active_local, "Locale", Side::Local);
    match app.remote.as_mut() {
        Some(r) => draw_pane(f, panes[1], r, !active_local, "Device", Side::Remote),
        None => {
            let b = Block::default().borders(Borders::ALL)
        .border_type(BorderType::Rounded).border_style(Style::default().fg(DIM));
            f.render_widget(Paragraph::new("").block(b), panes[1]);
        }
    }

    draw_transfers(f, chunks[2], app);
    draw_log(f, chunks[3], app);
    hints(
        f,
        chunks[4],
        &[
            &[("Tab", "pannello"), ("Spazio", "marca"), ("y", "pull"), ("p", "push"), ("T", "cartella intera"), ("s", "sync"), ("n", "nuova cart."), ("R", "rinomina"), ("d", "elimina")],
            &[("g", "vai a"), ("/", "filtro"), ("o", "ordina"), (".", "nascosti"), ("a", "tutto"), ("*", "inverti"), ("S", "SMS"), ("x", "annulla"), ("?", "aiuto"), ("q", "esci")],
        ],
    );
}

fn draw_pane(f: &mut Frame, area: Rect, p: &mut crate::app::Pane, active: bool, label: &str, side: Side) {
    let marked: HashSet<String> = p.marked.iter().map(|m| m.path.clone()).collect();
    let w = area.width.saturating_sub(2) as usize;
    // simbolo cursore (1) + marca (3) + icona (2) + size (9) + sep (2) + data (16) + margine (2)
    let name_w = w.saturating_sub(35).max(8);
    let acc = accent(side);

    let mut title = String::new();
    if p.loading {
        title.push_str("⏳ ");
    }
    if !p.marked.is_empty() {
        let total: u64 = p
            .entries
            .iter()
            .filter(|e| !e.is_dir && p.marked.iter().any(|m| m.path == e.full))
            .map(|e| e.size)
            .sum();
        title.push_str(&format!("[● {} · {}] ", p.marked.len(), humanize(total)));
    }
    if !p.filter.is_empty() {
        title.push_str(&format!("[/{}] ", p.filter));
    }
    let bottom = Line::from(Span::styled(
        format!(
            " {} voci · ord: {}{} ",
            p.entries.iter().filter(|e| e.name != "..").count(),
            p.sort.label(),
            if p.show_hidden { " · nascosti" } else { "" }
        ),
        Style::default().fg(if active { acc } else { DIM }),
    ));
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(if active { acc } else { DIM }))
        .title(Line::from(spans_title(label, &p.cwd, &title, side, active)))
        .title_bottom(bottom);

    if let Some(err) = &p.error {
        let text = format!("{err}");
        f.render_widget(
            Paragraph::new(text)
                .style(Style::default().fg(Color::Rgb(224, 108, 117)))
                .block(block)
                .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }

    let items: Vec<ListItem> = p
        .entries
        .iter()
        .map(|e| {
            let is_marked = marked.contains(&e.full);
            let is_dot = e.name == "..";
            let (icon, color) = if is_dot { ("↰", SOFT) } else { kind_style(&e.name, e.is_dir, side) };
            let size = if is_dot {
                String::new()
            } else if e.is_dir {
                format!("{:>9}", "—")
            } else {
                format!("{:>9}", humanize(e.size))
            };
            let date = if is_dot { String::new() } else { fmt_date(e.mtime) };
            let mut name_style = Style::default().fg(color);
            if e.is_dir {
                name_style = name_style.add_modifier(Modifier::BOLD);
            }
            if is_marked {
                name_style = name_style.fg(MARK).add_modifier(Modifier::BOLD);
            }
            ListItem::new(Line::from(vec![
                Span::styled(if is_marked { " ● " } else { "   " }, Style::default().fg(MARK)),
                Span::styled(format!("{icon} "), Style::default().fg(color)),
                Span::styled(format!("{:<name_w$}", trunc(&e.name, name_w)), name_style),
                Span::raw("  "),
                Span::styled(size, Style::default().fg(SOFT)),
                Span::raw("  "),
                Span::styled(date, Style::default().fg(DIM)),
            ]))
        })
        .collect();

    // cursore: pieno e colorato nel pannello attivo, tenue in quello inattivo
    let (hl, sym) = if active {
        (
            Style::default().fg(Color::Black).bg(acc).add_modifier(Modifier::BOLD),
            "▌",
        )
    } else {
        (Style::default().fg(Color::White).bg(ROW_INACTIVE), "▎")
    };
    let total = p.entries.len();
    let list = List::new(items).block(block).highlight_style(hl).highlight_symbol(sym);
    f.render_stateful_widget(list, area, &mut p.list_state);

    let visible = area.height.saturating_sub(2) as usize;
    if total > visible {
        let mut sb = ScrollbarState::new(total.saturating_sub(visible)).position(p.list_state.offset());
        f.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .track_symbol(Some("│"))
                .thumb_symbol("┃")
                .style(Style::default().fg(if active { acc } else { DIM })),
            area.inner(ratatui::layout::Margin { vertical: 1, horizontal: 0 }),
            &mut sb,
        );
    }
}

fn draw_transfers(f: &mut Frame, area: Rect, app: &App) {
    let snaps = app.queue.as_ref().map(|q| q.snapshot()).unwrap_or_default();
    let mut lines: Vec<Line> = Vec::new();
    if snaps.is_empty() {
        lines.push(Line::from(Span::styled(
            " nessun trasferimento (y pull · p push · s sync)",
            Style::default().fg(DIM),
        )));
    } else {
        for s in snaps.iter().rev().take(4) {
            let (g, c) = status_glyph(s.status);
            let pct = if s.total > 0 {
                format!(" {:>3}%", s.bytes * 100 / s.total.max(1))
            } else {
                String::new()
            };
            let bytes = if s.total > 0 {
                format!("{}/{}", humanize(s.bytes), humanize(s.total))
            } else if s.bytes > 0 {
                humanize(s.bytes)
            } else {
                String::new()
            };
            let dir_arrow = match s.dir {
                Direction::Pull => "↓",
                Direction::Push => "↑",
            };
            let msg = if s.status == Status::Failed {
                format!(" — {}", s.msg)
            } else if !s.msg.is_empty() && s.status == Status::Running {
                format!(" — {}", trunc(&s.msg, 40))
            } else {
                String::new()
            };
            let pcv = if s.total > 0 { s.bytes * 100 / s.total.max(1) } else { 0 };
            let gauge = if matches!(s.status, Status::Running | Status::Done) && s.total > 0 {
                bar(pcv, 14)
            } else {
                " ".repeat(14)
            };
            let dir_col = match s.dir {
                Direction::Pull => accent(Side::Local),
                Direction::Push => accent(Side::Remote),
            };
            lines.push(Line::from(vec![
                Span::styled(format!(" {g} "), Style::default().fg(c)),
                Span::styled(format!("{dir_arrow} "), Style::default().fg(dir_col).add_modifier(Modifier::BOLD)),
                Span::styled(gauge, Style::default().fg(c)),
                Span::styled(format!(" {:>12}{} ", bytes, pct), Style::default().fg(SOFT)),
                Span::styled(format!("{}{}", s.label, msg), Style::default().fg(c)),
            ]));
        }
    }
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(DIM))
                .title(Span::styled(" Trasferimenti ", Style::default().fg(SOFT).add_modifier(Modifier::BOLD))),
        ),
        area,
    );
}

fn draw_log(f: &mut Frame, area: Rect, app: &App) {
    let lines: Vec<Line> = app
        .log
        .iter()
        .rev()
        .take(3)
        .map(|s| {
            let style = if s.contains('⚠') {
                Style::default().fg(Color::Red)
            } else {
                Style::default()
            };
            Line::from(Span::styled(trunc(s, area.width as usize - 2), style))
        })
        .collect();
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::ALL)
        .border_type(BorderType::Rounded).title(" Log ")),
        area,
    );
}

fn centered_rect(px: u16, py: u16, r: Rect) -> Rect {
    let v = Layout::default()
        .direction(LDirection::Vertical)
        .constraints([
            Constraint::Percentage((100 - py) / 2),
            Constraint::Percentage(py),
            Constraint::Percentage((100 - py) / 2),
        ])
        .split(r);
    let h = Layout::default()
        .direction(LDirection::Horizontal)
        .constraints([
            Constraint::Percentage((100 - px) / 2),
            Constraint::Percentage(px),
            Constraint::Percentage((100 - px) / 2),
        ])
        .split(v[1]);
    h[1]
}

fn draw_dialog(f: &mut Frame, app: &App) {
    match &app.screen {
        Screen::Confirm { title, message, .. } => {
            let area = centered_rect(60, 30, f.area());
            f.render_widget(Clear, area);
            let txt = format!("{message}\n\n y = conferma   n = annulla");
            f.render_widget(
                Paragraph::new(txt)
                    .wrap(Wrap { trim: true })
                    .block(Block::default().borders(Borders::ALL)
        .border_type(BorderType::Rounded).title(format!(" {title} "))),
                area,
            );
        }
        Screen::Input { title, value, .. } => {
            let area = centered_rect(60, 20, f.area());
            f.render_widget(Clear, area);
            let txt = format!("{value}▌\n\n Invio = conferma   Esc = annulla");
            f.render_widget(
                Paragraph::new(txt)
                    .wrap(Wrap { trim: false })
                    .block(Block::default().borders(Borders::ALL)
        .border_type(BorderType::Rounded).title(format!(" {title} "))),
                area,
            );
        }
        _ => {}
    }
}

// JobKind usato solo per labeling futuro; evita dead-code warning.
#[allow(dead_code)]
fn _kind_marker(_: JobKind) {}
