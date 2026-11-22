//! Stato dell'applicazione TUI: schermate, pannelli, selezioni, coda.

use crate::actions;
use crate::adb::{Adb, Device};
use crate::device_fs::{self, Entry};
use crate::transfer::{Direction, JobKind, JobSpec, Queue};
use crate::util::{dev_basename, dev_join, dev_parent, fmt_date, now_secs};
use anyhow::Result;
use ratatui::layout::Rect;
use ratatui::widgets::ListState;
use std::collections::{HashSet, VecDeque};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Duration;
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Local,
    Remote,
}

#[derive(Debug, Clone)]
pub enum Screen {
    Devices { sel: usize },
    Files,
    Confirm {
        title: String,
        message: String,
        action: ConfirmAction,
    },
    Input {
        title: String,
        value: String,
        action: InputAction,
    },
    Help,
    /// Download di platform-tools in corso (stato in `App::dl`).
    Download,
    /// Elenco "Vai a…" (luoghi, preferiti, recenti).
    Goto {
        side: Side,
        items: Vec<crate::places::Place>,
        sel: usize,
        pos: (u16, u16),
    },
    /// Menu contestuale (click destro).
    Menu {
        items: Vec<(String, MenuAction)>,
        sel: usize,
        pos: (u16, u16),
        title: String,
        /// Dimensione reale (calcolata in background).
        size: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    Open,
    Transfer,
    TransferWhole,
    Sync,
    Mark,
    MarkAll,
    Rename,
    Delete,
    Mkdir,
    Refresh,
    Sort,
    Hidden,
}

/// Rettangolo del menu contestuale (condiviso da rendering e hit-testing).
pub fn menu_rect(items: &[(String, MenuAction)], pos: (u16, u16), screen: Rect) -> Rect {
    popup_rect(items.iter().map(|i| i.0.chars().count()).max().unwrap_or(10).max(48), items.len(), pos, screen)
}

/// Rettangolo di un popup con `n` righe larghe al massimo `maxw` caratteri.
pub fn popup_rect(maxw: usize, n: usize, pos: (u16, u16), screen: Rect) -> Rect {
    let w = (maxw as u16 + 6).min(screen.width);
    let h = (n as u16 + 2).min(screen.height);
    let x = pos.0.min(screen.width.saturating_sub(w));
    let y = pos.1.min(screen.height.saturating_sub(h));
    Rect { x, y, width: w, height: h }
}

#[derive(Debug, Clone)]
pub enum ConfirmAction {
    /// Scarica platform-tools (adb non trovato).
    DownloadAdb,
    DeleteDevice(Vec<String>),
    DeleteLocal(Vec<String>),
}

#[derive(Debug, Clone)]
pub enum InputAction {
    Mkdir(Side),
    Rename(Side, String),
    Filter(Side),
    GotoPath(Side),
    Connect,
    /// Percorso di adb (file o cartella che lo contiene).
    AdbPath,
    /// Trasferimento dal pannello indicato (destinazione editabile).
    Transfer(Side, bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortMode {
    Name,
    Size,
    Date,
}

impl SortMode {
    pub fn label(self) -> &'static str {
        match self {
            SortMode::Name => t!("nome"),
            SortMode::Size => t!("dimensione"),
            SortMode::Date => t!("data"),
        }
    }
    fn next(self) -> Self {
        match self {
            SortMode::Name => SortMode::Size,
            SortMode::Size => SortMode::Date,
            SortMode::Date => SortMode::Name,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MarkedItem {
    pub path: String,
    pub is_dir: bool,
}

pub struct Pane {
    pub cwd: String,
    pub entries: Vec<Entry>,
    /// Elenco completo non filtrato (la vista `entries` è derivata da qui).
    pub raw: Vec<Entry>,
    pub sort: SortMode,
    pub show_hidden: bool,
    pub filter: String,
    pub goto_name: Option<String>,
    pub sel: usize,
    pub marked: HashSet<MarkedItem>,
    pub list_state: ListState,
    pub loading: bool,
    pub error: Option<String>,
}

impl Pane {
    pub fn new(cwd: String) -> Self {
        Self {
            cwd,
            entries: Vec::new(),
            raw: Vec::new(),
            sort: SortMode::Name,
            show_hidden: false,
            filter: String::new(),
            goto_name: None,
            sel: 0,
            marked: HashSet::new(),
            list_state: ListState::default(),
            loading: false,
            error: None,
        }
    }

    pub fn selected(&self) -> Option<&Entry> {
        self.entries.get(self.sel)
    }
}

pub struct Pending {
    side: Side,
    cwd: String,
    rx: Receiver<Result<Vec<Entry>>>,
}

pub struct App {
    pub adb: Adb,
    pub devices: Vec<Device>,
    pub screen: Screen,
    pub local: Pane,
    pub remote: Option<Pane>,
    pub queue: Option<Arc<Queue>>,
    pub active: Side,
    pub log: VecDeque<String>,
    pub info: Option<String>,
    pub pending: Option<Pending>,
    pub msgs: Option<Receiver<String>>,
    pub quit: bool,
    /// Aree dei pannelli (Locale, Device) dell'ultimo frame, per il mouse.
    pub pane_rects: [Rect; 2],
    last_click: Option<(std::time::Instant, Side, usize)>,
    pub saved: crate::places::Saved,
    size_rx: Option<Receiver<(u64, String)>>,
    pub dl: Option<Arc<Mutex<crate::adb::DlState>>>,
    /// Aggiornamento automatico dell'elenco device (schermata di selezione).
    dev_rx: Option<Receiver<Result<Vec<crate::adb::Device>, String>>>,
    dev_last: std::time::Instant,
    menu_gen: u64,
}

fn clamp_sel(p: &mut Pane) {
    let max = p.entries.len().saturating_sub(1);
    p.sel = p.sel.min(max);
    p.list_state.select(Some(p.sel));
}

/// Ricalcola la vista (nascosti, filtro, ordinamento) da `raw`,
/// mantenendo il cursore sulla stessa voce se ancora presente.
pub fn rebuild(p: &mut Pane) {
    let cur = p.selected().map(|e| e.name.clone());
    let needle = p.filter.to_lowercase();
    let mut dotdot: Vec<Entry> = Vec::new();
    let mut v: Vec<Entry> = Vec::new();
    for e in &p.raw {
        if e.name == ".." {
            dotdot.push(e.clone());
            continue;
        }
        if !p.show_hidden && e.name.starts_with('.') {
            continue;
        }
        if !needle.is_empty() && !e.name.to_lowercase().contains(&needle) {
            continue;
        }
        v.push(e.clone());
    }
    let mode = p.sort;
    v.sort_by(|a, b| {
        b.is_dir.cmp(&a.is_dir).then_with(|| match mode {
            SortMode::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            SortMode::Size => b.size.cmp(&a.size).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
            SortMode::Date => b.mtime.cmp(&a.mtime).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
        })
    });
    dotdot.extend(v);
    p.entries = dotdot;
    p.sel = cur
        .and_then(|n| p.entries.iter().position(|e| e.name == n))
        .unwrap_or(0);
    clamp_sel(p);
}

fn apply_entries(p: &mut Pane, entries: Vec<Entry>) {
    p.raw = entries;
    p.error = None;
    p.loading = false;
    rebuild(p);
    if let Some(n) = p.goto_name.take() {
        if let Some(i) = p.entries.iter().position(|e| e.name == n) {
            p.sel = i;
            clamp_sel(p);
        }
    }
}

fn default_backup_dir(log: &mut VecDeque<String>) -> String {
    match crate::util::default_backup_dir() {
        Ok(d) => d,
        Err(e) => {
            log.push_back(t!("⚠ cartella backup: {e}", e = e));
            std::env::current_dir()
                .map(|p| crate::util::norm_local(&p.to_string_lossy()))
                .unwrap_or_else(|_| "/".into())
        }
    }
}

/// Schermata di conferma per scaricare adb, con le istruzioni manuali per il sistema in uso.
fn download_offer_screen() -> Screen {
    Screen::Confirm {
        title: t!("Scarica adb").into(),
        message: crate::adb::download_offer_text(),
        action: ConfirmAction::DownloadAdb,
    }
}

impl App {
    pub fn new() -> Self {
        let mut log = VecDeque::new();
        let mut screen = Screen::Devices { sel: 0 };
        let bin = match crate::adb::locate() {
            Ok(b) => b,
            Err(b) => {
                log.push_back(format!("⚠ {}", crate::adb::missing_hint()));
                screen = download_offer_screen();
                b
            }
        };
        let adb = Adb::new(bin);
        let mut devices = Vec::new();
        if !matches!(screen, Screen::Confirm { .. }) {
            match adb.devices() {
                Ok(v) => devices = v,
                Err(e) => log.push_back(format!("⚠ adb: {e}")),
            }
        }
        let cwd = default_backup_dir(&mut log);
        let local = Pane::new(cwd);
        Self {
            adb,
            devices,
            screen,
            local,
            remote: None,
            queue: None,
            active: Side::Local,
            log,
            info: None,
            pending: None,
            msgs: None,
            quit: false,
            pane_rects: [Rect::default(); 2],
            last_click: None,
            saved: crate::places::Saved::load(),
            size_rx: None,
            dl: None,
            dev_rx: None,
            dev_last: std::time::Instant::now(),
            menu_gen: 0,
        }
    }

    pub fn log(&mut self, s: impl Into<String>) {
        self.log.push_back(s.into());
        while self.log.len() > 400 {
            self.log.pop_front();
        }
    }

    /// Drena i risultati asincroni (listing remoto, messaggi da thread).
    pub fn poll(&mut self) {
        self.poll_download();
        self.poll_devices();
        if let Some(rx) = self.size_rx.as_ref() {
            while let Ok((g, txt)) = rx.try_recv() {
                if g == self.menu_gen {
                    if let Screen::Menu { size, .. } = &mut self.screen {
                        *size = txt;
                    }
                }
            }
        }
        if let Some(p) = self.pending.as_ref() {
            match p.rx.try_recv() {
                Ok(Ok(entries)) => {
                    let side = p.side;
                    let cwd = p.cwd.clone();
                    self.pending = None;
                    if side == Side::Remote {
                        if let Some(r) = self.remote.as_mut() {
                            if r.cwd == cwd {
                                apply_entries(r, entries);
                            }
                        }
                    }
                }
                Ok(Err(e)) => {
                    let side = p.side;
                    self.pending = None;
                    if side == Side::Remote {
                        if let Some(r) = self.remote.as_mut() {
                            r.error = Some(format!("{e}"));
                            r.loading = false;
                        }
                    }
                    self.log(format!("⚠ {e}"));
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.pending = None;
                }
            }
        }
        if let Some(rx) = self.msgs.take() {
            let mut dead = false;
            loop {
                match rx.try_recv() {
                    Ok(m) => self.log(m),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        dead = true;
                        break;
                    }
                }
            }
            if !dead {
                self.msgs = Some(rx);
            }
        }
    }

    pub fn refresh(&mut self, side: Side) {
        let cur = match side {
            Side::Local => Some(self.local.cwd.clone()),
            Side::Remote => self.remote.as_ref().map(|r| r.cwd.clone()),
        };
        if let Some(c) = cur {
            self.saved.visit(side, &c);
        }
        match side {
            Side::Local => {
                let cwd = self.local.cwd.clone();
                match device_fs::list_local(&cwd) {
                    Ok(entries) => apply_entries(&mut self.local, entries),
                    Err(e) => {
                        self.local.error = Some(format!("{e}"));
                        self.local.loading = false;
                        self.log(format!("⚠ {e}"));
                    }
                }
            }
            Side::Remote => {
                let Some(remote) = self.remote.as_ref() else { return };
                let cwd = remote.cwd.clone();
                let adb = self.adb.clone();
                let (tx, rx) = channel();
                let cwd2 = cwd.clone();
                std::thread::spawn(move || {
                    let res = device_fs::list(&adb, &cwd2);
                    let _ = tx.send(res);
                });
                self.pending = Some(Pending {
                    side: Side::Remote,
                    cwd,
                    rx,
                });
                if let Some(r) = self.remote.as_mut() {
                    r.loading = true;
                }
            }
        }
    }

    pub fn connect(&mut self, sel: usize) {
        let Some(dev) = self.devices.get(sel).cloned() else { return };
        if dev.state != "device" {
            self.log(t!(
                "⚠ {serial}: stato '{state}' — sblocca il telefono e conferma la finestra RSA, poi riprova",
                serial = dev.serial,
                state = dev.state
            ));
            return;
        }
        self.adb.serial = Some(dev.serial.clone());
        let transport = if dev.serial.contains(':') { "Wi-Fi" } else { "USB" };
        match self.adb.info() {
            Ok(info) => {
                self.info = Some(format!(
                    "{} · Android {} · {} ({})",
                    info.model, info.android, dev.serial, transport
                ));
            }
            Err(_) => {
                self.info = Some(format!("{} ({})", dev.serial, transport));
            }
        }
        self.queue = Some(Arc::new(Queue::spawn(self.adb.clone())));
        self.remote = Some(Pane::new("/sdcard".into()));
        self.active = Side::Remote;
        self.log(t!("device connesso: {serial}", serial = dev.serial));
        self.refresh(Side::Local);
        self.refresh(Side::Remote);
        self.screen = Screen::Files;
    }

    fn pane(&mut self, side: Side) -> Option<&mut Pane> {
        match side {
            Side::Local => Some(&mut self.local),
            Side::Remote => self.remote.as_mut(),
        }
    }

    pub fn switch_active(&mut self) {
        self.active = match self.active {
            Side::Local => Side::Remote,
            Side::Remote => Side::Local,
        };
    }

    pub fn move_sel(&mut self, delta: i64) {
        let Some(p) = self.pane(self.active) else { return };
        let len = p.entries.len() as i64;
        let cur = p.sel as i64;
        p.sel = (cur + delta).clamp(0, len.saturating_sub(1)) as usize;
        p.list_state.select(Some(p.sel));
    }

    pub fn sel_home_end(&mut self, end: bool) {
        let Some(p) = self.pane(self.active) else { return };
        p.sel = if end { p.entries.len().saturating_sub(1) } else { 0 };
        p.list_state.select(Some(p.sel));
    }

    pub fn go_parent(&mut self) {
        let side = self.active;
        let cwd = match side {
            Side::Local => self.local.cwd.clone(),
            Side::Remote => self.remote.as_ref().map(|p| p.cwd.clone()).unwrap_or_default(),
        };
        let parent = match side {
            Side::Local => crate::util::local_parent(&cwd),
            Side::Remote => dev_parent(&cwd),
        };
        let changed = match side {
            Side::Local => parent != cwd,
            Side::Remote => parent != cwd && !parent.is_empty(),
        };
        if !changed {
            return;
        }
        if let Some(p) = self.pane(side) {
            // risalendo, il cursore torna sulla cartella da cui si proviene
            let from = dev_basename(&p.cwd).to_string();
            p.cwd = parent;
            p.marked.clear();
            p.filter.clear();
            p.entries.clear();
            p.raw.clear();
            p.sel = 0;
            p.list_state.select(Some(0));
            p.goto_name = Some(from);
        }
        self.refresh(side);
    }

    /// Enter su una voce: directory -> naviga; file -> trasferimento verso l'altro pannello.
    pub fn enter(&mut self) {
        let side = self.active;
        let e = {
            let Some(p) = self.pane(side) else { return };
            let Some(e) = p.selected().cloned() else { return };
            if e.is_dir {
                if e.name == ".." {
                    p.goto_name = Some(dev_basename(&p.cwd).to_string());
                }
                p.cwd = e.full.clone();
                p.marked.clear();
                p.filter.clear();
                p.sel = 0;
                p.raw.clear();
                p.entries.clear();
                p.list_state.select(Some(0));
            }
            e.is_dir
        };
        if e {
            self.refresh(side);
        } else {
            match side {
                Side::Remote => self.pull_jobs(),
                Side::Local => self.push_jobs(),
            }
        }
    }

    /// `→`/`l`: entra solo se è una directory (non avvia trasferimenti).
    pub fn enter_dir_only(&mut self) {
        let is_dir = self
            .pane(self.active)
            .and_then(|p| p.selected().map(|e| e.is_dir))
            .unwrap_or(false);
        if is_dir {
            self.enter();
        }
    }

    fn toggle_mark_stay(&mut self) {
        let Some(p) = self.pane(self.active) else { return };
        let Some(e) = p.selected().cloned() else { return };
        if e.name == ".." {
            return;
        }
        let item = MarkedItem { path: e.full, is_dir: e.is_dir };
        if !p.marked.remove(&item) {
            p.marked.insert(item);
        }
    }

    pub fn toggle_mark(&mut self) {
        let Some(p) = self.pane(self.active) else { return };
        let Some(e) = p.selected().cloned() else { return };
        if e.name == ".." {
            return;
        }
        let item = MarkedItem {
            path: e.full.clone(),
            is_dir: e.is_dir,
        };
        if !p.marked.remove(&item) {
            p.marked.insert(item);
        }
        self.move_sel(1);
    }

    /// Seleziona/deseleziona tutto (tranne "..").
    pub fn mark_all(&mut self) {
        let Some(p) = self.pane(self.active) else { return };
        let all: Vec<MarkedItem> = p
            .entries
            .iter()
            .filter(|e| e.name != "..")
            .map(|e| MarkedItem {
                path: e.full.clone(),
                is_dir: e.is_dir,
            })
            .collect();
        if p.marked.len() == all.len() && !all.is_empty() {
            p.marked.clear();
        } else {
            p.marked = all.into_iter().collect();
        }
    }

    fn selection_items(&self, side: Side) -> Vec<MarkedItem> {
        let Some(p) = (match side {
            Side::Local => Some(&self.local),
            Side::Remote => self.remote.as_ref(),
        }) else {
            return Vec::new();
        };
        if !p.marked.is_empty() {
            let mut v: Vec<MarkedItem> = p.marked.iter().cloned().collect();
            v.sort_by(|a, b| a.path.cmp(&b.path));
            v
        } else {
            match p.selected() {
                Some(e) if e.name != ".." => vec![MarkedItem {
                    path: e.full.clone(),
                    is_dir: e.is_dir,
                }],
                _ => Vec::new(),
            }
        }
    }

    /// `y`: accoda il pull (device -> locale) di selezione/marcature del pannello remoto.
    pub fn pull_jobs(&mut self) {
        self.transfer_request(Side::Remote, false);
    }

    /// `T`: trasferisce l'INTERA cartella corrente del pannello attivo.
    pub fn transfer_whole_dir(&mut self) {
        let side = self.active;
        self.transfer_request(side, true);
    }

    /// Voci da trasferire: la cartella corrente intera oppure selezione/marcature.
    fn transfer_items(&self, side: Side, whole: bool) -> Vec<MarkedItem> {
        if !whole {
            return self.selection_items(side);
        }
        let cwd = match side {
            Side::Local => Some(self.local.cwd.clone()),
            Side::Remote => self.remote.as_ref().map(|p| p.cwd.clone()),
        };
        match cwd {
            Some(c) if !c.trim_end_matches('/').is_empty() => vec![MarkedItem {
                path: c.trim_end_matches('/').to_string(),
                is_dir: true,
            }],
            _ => Vec::new(),
        }
    }

    /// Dialogo di trasferimento: riepilogo + cartella di destinazione modificabile
    /// (precompilata con la cartella dell'altro pannello).
    fn transfer_request(&mut self, from: Side, whole: bool) {
        if self.queue.is_none() {
            self.log(t!("⚠ collega prima un device"));
            return;
        }
        // y/p sulla voce ".." = trasferisci la cartella corrente intera
        let on_dotdot = !whole
            && self.pane(from).map(|p| p.marked.is_empty() && p.selected().map(|e| e.name == "..").unwrap_or(false)).unwrap_or(false);
        let whole = whole || on_dotdot;
        let items = self.transfer_items(from, whole);
        if items.is_empty() {
            self.log(t!("niente da trasferire: seleziona o marca qualche voce (spazio)"));
            return;
        }
        let (dirs, files) = (
            items.iter().filter(|i| i.is_dir).count(),
            items.iter().filter(|i| !i.is_dir).count(),
        );
        let src_pane = if from == Side::Local { Some(&self.local) } else { self.remote.as_ref() };
        let bytes: u64 = src_pane
            .map(|p| {
                p.entries
                    .iter()
                    .filter(|e| !e.is_dir && items.iter().any(|i| i.path == e.full))
                    .map(|e| e.size)
                    .sum()
            })
            .unwrap_or(0);
        let dest = match from {
            Side::Remote => self.local.cwd.clone(),
            Side::Local => match self.remote.as_ref() {
                Some(r) => r.cwd.clone(),
                None => return,
            },
        };
        let (verb, arrow) = if from == Side::Remote { ("Pull", t!("→ locale")) } else { ("Push", "→ device") };
        let what = if whole {
            let n = items.first().map(|i| dev_basename(&i.path).to_string()).unwrap_or_default();
            t!("cartella intera «{n}»", n = n)
        } else {
            t!("{dirs} cartelle, {files} file (≥ {size})", dirs = dirs, files = files, size = crate::util::humanize(bytes))
        };
        self.screen = Screen::Input {
            title: t!("{verb} {arrow}: {what} — cartella di destinazione", verb = verb, arrow = arrow, what = what),
            value: dest,
            action: InputAction::Transfer(from, whole),
        };
    }

    fn pull_to(&mut self, local_cwd: String, whole: bool) {
        let Some(queue) = self.queue.clone() else { return };
        let items = self.transfer_items(Side::Remote, whole);
        let local_cwd = local_cwd.trim_end_matches('/').to_string();
        if let Err(e) = std::fs::create_dir_all(&local_cwd) {
            self.log(t!("⚠ destinazione {dir}: {e}", dir = local_cwd, e = e));
            return;
        }
        let mut n = 0;
        for it in items {
            let name = dev_basename(&it.path).to_string();
            let dst = format!("{local_cwd}/{name}");
            let kind = if it.is_dir { JobKind::Dir } else { JobKind::File };
            queue.submit(JobSpec::new(
                Direction::Pull,
                kind,
                it.path.clone(),
                dst,
                format!("pull {name}"),
            ));
            n += 1;
        }
        self.log(t!("accodati {n} pull → {dir}", n = n, dir = local_cwd));
    }

    /// `p`: accoda il push (locale -> device) di selezione/marcature del pannello locale.
    pub fn push_jobs(&mut self) {
        self.transfer_request(Side::Local, false);
    }

    fn push_to(&mut self, dst_dir: String, whole: bool) {
        let Some(queue) = self.queue.clone() else { return };
        let items = self.transfer_items(Side::Local, whole);
        let dst_dir = dst_dir.trim_end_matches('/').to_string();
        if dst_dir.is_empty() {
            self.log(t!("⚠ destinazione non valida"));
            return;
        }
        if let Err(e) = device_fs::mkdir_device(&self.adb, &dst_dir) {
            self.log(t!("⚠ destinazione {dir}: {e}", dir = dst_dir, e = e));
            return;
        }
        let mut n = 0;
        for it in items {
            let name = Path::new(&it.path)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let kind = if it.is_dir { JobKind::Dir } else { JobKind::File };
            queue.submit(JobSpec::new(
                Direction::Push,
                kind,
                it.path.clone(),
                dst_dir.clone(),
                format!("push {name}"),
            ));
            n += 1;
        }
        self.log(t!("accodati {n} push → {dir}", n = n, dir = dst_dir));
    }

    /// `s`: mirror incrementale della directory remota corrente nel pannello locale.
    pub fn sync_job(&mut self) {
        let Some(queue) = self.queue.clone() else { return };
        let Some(remote) = self.remote.as_ref() else { return };
        let name = if remote.cwd.trim_end_matches('/').is_empty() {
            "root".to_string()
        } else {
            dev_basename(&remote.cwd).to_string()
        };
        let dst = format!("{}/{name}", self.local.cwd.trim_end_matches('/'));
        queue.submit(JobSpec::new(
            Direction::Pull,
            JobKind::Sync,
            remote.cwd.clone(),
            dst.clone(),
            format!("sync {name}"),
        ));
        self.log(t!("avviato sync: {src} → {dst}", src = remote.cwd, dst = dst));
    }

    /// `d`: richiede conferma di cancellazione degli elementi del pannello attivo.
    pub fn delete_request(&mut self) {
        let side = self.active;
        let items = self.selection_items(side);
        if items.is_empty() {
            self.log(t!("niente da eliminare"));
            return;
        }
        let names: Vec<String> = items
            .iter()
            .map(|i| {
                if side == Side::Remote {
                    dev_basename(&i.path).to_string()
                } else {
                    Path::new(&i.path)
                        .file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| i.path.clone())
                }
            })
            .collect();
        let preview: String = {
            let mut s = names.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
            if names.len() > 3 {
                s.push_str(", …");
            }
            s
        };
        let action = match side {
            Side::Remote => ConfirmAction::DeleteDevice(items.iter().map(|i| i.path.clone()).collect()),
            Side::Local => ConfirmAction::DeleteLocal(items.iter().map(|i| i.path.clone()).collect()),
        };
        let (dirs, files) = (
            items.iter().filter(|i| i.is_dir).count(),
            items.iter().filter(|i| !i.is_dir).count(),
        );
        let bytes: u64 = {
            let p = if side == Side::Local { Some(&self.local) } else { self.remote.as_ref() };
            p.map(|p| {
                p.entries
                    .iter()
                    .filter(|e| !e.is_dir && items.iter().any(|i| i.path == e.full))
                    .map(|e| e.size)
                    .sum()
            })
            .unwrap_or(0)
        };
        let where_ = if side == Side::Remote { "DEVICE" } else { t!("locale") };
        self.screen = Screen::Confirm {
            title: t!("Conferma cancellazione").into(),
            message: t!(
                "Eliminare da {where_}: {dirs} cartelle (con contenuto) e {files} file ({size}) ?\n{preview}\n\nOperazione irreversibile.",
                where_ = where_,
                dirs = dirs,
                files = files,
                size = crate::util::humanize(bytes),
                preview = preview
            ),
            action,
        };
    }

    pub fn confirm(&mut self, yes: bool) {
        let action = match &self.screen {
            Screen::Confirm { action, .. } => action.clone(),
            _ => return,
        };
        if let ConfirmAction::DownloadAdb = action {
            if yes {
                self.start_download();
            } else {
                self.adb_path_request();
            }
            return;
        }
        self.screen = Screen::Files;
        if !yes {
            return;
        }
        match action {
            ConfirmAction::DownloadAdb => {}
            ConfirmAction::DeleteDevice(paths) => {
                let mut ok = 0;
                for p in &paths {
                    match device_fs::rm_device(&self.adb, p) {
                        Ok(()) => ok += 1,
                        Err(e) => self.log(format!("⚠ rm {p}: {e}")),
                    }
                    if let Some(r) = self.remote.as_mut() {
                        r.marked.remove(&MarkedItem {
                            path: p.clone(),
                            is_dir: true,
                        });
                        r.marked.remove(&MarkedItem {
                            path: p.clone(),
                            is_dir: false,
                        });
                    }
                }
                self.log(t!("eliminati {ok} elementi sul device", ok = ok));
                self.refresh(Side::Remote);
            }
            ConfirmAction::DeleteLocal(paths) => {
                let mut ok = 0;
                for p in &paths {
                    match device_fs::delete_local(p) {
                        Ok(()) => ok += 1,
                        Err(e) => self.log(format!("⚠ rm {p}: {e}")),
                    }
                    self.local.marked.remove(&MarkedItem {
                        path: p.clone(),
                        is_dir: true,
                    });
                    self.local.marked.remove(&MarkedItem {
                        path: p.clone(),
                        is_dir: false,
                    });
                }
                self.log(t!("eliminati {ok} elementi in locale", ok = ok));
                self.refresh(Side::Local);
            }
        }
    }

    /// `n`: richiesta nome nuova cartella nel pannello attivo.
    pub fn mkdir_request(&mut self) {
        let side = self.active;
        self.screen = Screen::Input {
            title: match side {
                Side::Local => t!("Nuova cartella in locale").into(),
                Side::Remote => t!("Nuova cartella sul device").into(),
            },
            value: String::new(),
            action: InputAction::Mkdir(side),
        };
    }

    /// Schermata device: `c` -> input indirizzo Wi-Fi adb.
    pub fn connect_request(&mut self) {
        self.screen = Screen::Input {
            title: t!("adb connect IP:PORT (es. 192.168.1.5:5555)").into(),
            value: String::new(),
            action: InputAction::Connect,
        };
    }

    /// Schermata device: `d` -> offerta di download di adb.
    pub fn adb_download_request(&mut self) {
        self.screen = download_offer_screen();
    }

    /// Avvia il download di platform-tools in un thread.
    fn start_download(&mut self) {
        let st = Arc::new(Mutex::new(crate::adb::DlState::default()));
        self.dl = Some(st.clone());
        self.screen = Screen::Download;
        std::thread::spawn(move || {
            let res = crate::adb::download_platform_tools(&st).map_err(|e| format!("{e:#}"));
            st.lock().unwrap().result = Some(res);
        });
    }

    /// Controlla l'esito del download e, se riuscito, passa al nuovo adb.
    fn poll_download(&mut self) {
        let Some(st) = self.dl.clone() else { return };
        let res = st.lock().unwrap().result.take();
        let Some(res) = res else { return };
        self.dl = None;
        match res {
            Ok(bin) => {
                crate::adb::save_path(&bin);
                self.log(t!("adb scaricato: {bin}", bin = bin));
                self.adb.bin = bin;
                self.refresh_devices();
                self.screen = Screen::Devices { sel: 0 };
            }
            Err(e) => {
                self.log(format!("⚠ {e}"));
                self.screen = Screen::Input {
                    title: t!("download non riuscito: {e} — indica il percorso di adb a mano (Esc per saltare)", e = e),
                    value: String::new(),
                    action: InputAction::AdbPath,
                };
            }
        }
    }

    /// Schermata device: `a` -> input percorso di adb.
    pub fn adb_path_request(&mut self) {
        self.screen = Screen::Input {
            title: crate::adb::path_prompt(),
            value: String::new(),
            action: InputAction::AdbPath,
        };
    }

    pub fn submit_input(&mut self, value: String) {
        let action = match &self.screen {
            Screen::Input { action, .. } => action.clone(),
            _ => return,
        };
        self.screen = Screen::Files;
        let v = value.trim().to_string();
        match action {
            InputAction::AdbPath => {
                let bin = crate::adb::resolve_user_path(&v);
                if v.is_empty() || !crate::adb::works(&bin) {
                    self.log(t!("⚠ adb non valido: {bin}", bin = bin));
                    self.screen = Screen::Input {
                        title: t!("adb non trovato in «{v}» — riprova (Esc per annullare)", v = v),
                        value: v,
                        action: InputAction::AdbPath,
                    };
                    return;
                }
                crate::adb::save_path(&bin);
                self.log(t!("adb: {bin} (salvato)", bin = bin));
                self.adb.bin = bin;
                self.refresh_devices();
                self.screen = Screen::Devices { sel: 0 };
            }
            InputAction::Mkdir(side) => {
                if v.is_empty() || v.contains('/') || (cfg!(windows) && v.contains('\\')) || v.split('/').any(|c| c == "..") {
                    self.log(format!("{} {v:?}", t!("⚠ nome non valido:")));
                    return;
                }
                match side {
                    Side::Local => {
                        let p = format!("{}/{}", self.local.cwd.trim_end_matches('/'), v);
                        match device_fs::mkdir_local(&p) {
                            Ok(()) => {
                                self.log(t!("creata {p}", p = p));
                                self.refresh(Side::Local);
                            }
                            Err(e) => self.log(format!("⚠ mkdir: {e}")),
                        }
                    }
                    Side::Remote => {
                        let Some(remote) = self.remote.as_ref() else { return };
                        let p = dev_join(&remote.cwd, &v);
                        match device_fs::mkdir_device(&self.adb, &p) {
                            Ok(()) => {
                                self.log(t!("creata {p}", p = p));
                                self.refresh(Side::Remote);
                            }
                            Err(e) => self.log(format!("⚠ mkdir: {e}")),
                        }
                    }
                }
            }
            InputAction::Transfer(from, whole) => {
                if v.is_empty() {
                    return;
                }
                match from {
                    Side::Remote => self.pull_to(v, whole),
                    Side::Local => self.push_to(v, whole),
                }
            }
            InputAction::GotoPath(side) => {
                let mut path = v;
                if side == Side::Local {
                    if path == "~" || path.starts_with("~/") || path.starts_with("~\\") {
                        if let Some(h) = crate::util::home_dir() {
                            path = format!("{h}{}", &path[1..]);
                        }
                    }
                    path = crate::util::norm_local(&path);
                } else if !path.starts_with('/') {
                    path = format!("/{path}");
                }
                self.goto(side, path);
            }
            InputAction::Filter(side) => {
                if let Some(p) = self.pane(side) {
                    p.filter = v;
                    rebuild(p);
                }
            }
            InputAction::Rename(side, old) => {
                if v.is_empty() || v.contains('/') || (cfg!(windows) && v.contains('\\')) || v == "." || v == ".." {
                    self.log(format!("{} {v:?}", t!("⚠ nome non valido:")));
                    return;
                }
                if dev_basename(&old) == v {
                    return;
                }
                let parent = dev_parent(&old);
                let new = dev_join(&parent, &v);
                let res = match side {
                    Side::Local => device_fs::rename_local(&old, &new),
                    Side::Remote => device_fs::rename_device(&self.adb, &old, &new),
                };
                match res {
                    Ok(()) => {
                        self.log(t!("rinominato in {v}", v = v));
                        if let Some(p) = self.pane(side) {
                            p.marked.clear();
                            p.goto_name = Some(v);
                        }
                        self.refresh(side);
                    }
                    Err(e) => self.log(t!("⚠ rinomina: {e}", e = e)),
                }
            }
            InputAction::Connect => {
                if v.is_empty() {
                    return;
                }
                match self.adb.connect(&v) {
                    Ok(out) => self.log(format!("adb connect: {out}")),
                    Err(e) => self.log(format!("⚠ adb connect: {e}")),
                }
                if let Ok(devs) = self.adb.devices() {
                    self.devices = devs;
                }
                self.screen = Screen::Devices { sel: 0 };
            }
        }
    }

    /// Individua pannello e riga sotto il cursore del mouse.
    fn hit(&self, x: u16, y: u16) -> Option<(Side, Option<usize>)> {
        for (i, side) in [(0, Side::Local), (1, Side::Remote)] {
            let r = self.pane_rects[i];
            if r.width == 0 || x < r.x || x >= r.x + r.width || y < r.y || y >= r.y + r.height {
                continue;
            }
            let pane = if side == Side::Local { Some(&self.local) } else { self.remote.as_ref() }?;
            let inner_top = r.y + 1;
            let row = if y >= inner_top && y + 1 < r.y + r.height {
                let idx = pane.list_state.offset() + (y - inner_top) as usize;
                if idx < pane.entries.len() { Some(idx) } else { None }
            } else {
                None
            };
            return Some((side, row));
        }
        None
    }

    fn select_at(&mut self, side: Side, idx: Option<usize>) {
        self.active = side;
        if let Some(i) = idx {
            if let Some(p) = self.pane(side) {
                p.sel = i;
                p.list_state.select(Some(i));
            }
        }
    }

    /// Gestione mouse: click sinistro = seleziona (doppio = apri/trasferisci),
    /// click destro = menu contestuale, rotella = scorri.
    pub fn on_mouse(&mut self, m: crossterm::event::MouseEvent) {
        use crossterm::event::{MouseButton, MouseEventKind};
        let (w, h) = crossterm::terminal::size().unwrap_or((120, 40));
        let screen = Rect { x: 0, y: 0, width: w, height: h };

        // menu aperto: i click lo governano
        if let Screen::Menu { items, pos, .. } = &self.screen {
            let r = menu_rect(items, *pos, screen);
            let inside = m.column >= r.x && m.column < r.x + r.width && m.row > r.y && m.row + 1 < r.y + r.height;
            let idx = (m.row.saturating_sub(r.y + 1)) as usize;
            match m.kind {
                MouseEventKind::Moved | MouseEventKind::Drag(_) if inside => {
                    if let Screen::Menu { sel, items, .. } = &mut self.screen {
                        *sel = idx.min(items.len().saturating_sub(1));
                    }
                }
                MouseEventKind::Down(MouseButton::Left) => {
                    if inside {
                        let act = items.get(idx).map(|i| i.1);
                        self.screen = Screen::Files;
                        if let Some(a) = act {
                            self.run_menu_action(a);
                        }
                    } else {
                        self.screen = Screen::Files;
                    }
                }
                MouseEventKind::Down(MouseButton::Right) => {
                    self.screen = Screen::Files;
                    self.on_mouse(m); // riapre il menu nel nuovo punto
                }
                MouseEventKind::Down(_) => self.screen = Screen::Files,
                _ => {}
            }
            return;
        }
        if let Screen::Goto { items, pos, side, .. } = &self.screen {
            let side = *side;
            let maxw = items.iter().map(|i| i.0.chars().count()).max().unwrap_or(10);
            let r = popup_rect(maxw, items.len(), *pos, screen);
            let inside = m.column >= r.x && m.column < r.x + r.width && m.row > r.y && m.row + 1 < r.y + r.height;
            let idx = (m.row.saturating_sub(r.y + 1)) as usize;
            match m.kind {
                MouseEventKind::Moved | MouseEventKind::Drag(_) if inside => {
                    if let Screen::Goto { sel, items, .. } = &mut self.screen {
                        *sel = idx.min(items.len().saturating_sub(1));
                    }
                }
                MouseEventKind::Down(MouseButton::Left) if inside => {
                    let path = items.get(idx).map(|i| i.1.clone());
                    if let Some(p) = path {
                        self.goto_choose(side, p);
                    }
                }
                MouseEventKind::Down(_) => self.screen = Screen::Files,
                _ => {}
            }
            return;
        }
        if !matches!(self.screen, Screen::Files) {
            return;
        }
        // Menu contestuale: click destro, ma anche click centrale o Ctrl+click sinistro, per i terminali
        // (es. terminale integrato di VS Code, Windows Terminal) che intercettano il click destro.
        let ctrl = m.modifiers.contains(crossterm::event::KeyModifiers::CONTROL);
        let ctx = matches!(m.kind, MouseEventKind::Down(MouseButton::Right) | MouseEventKind::Down(MouseButton::Middle))
            || (ctrl && m.kind == MouseEventKind::Down(MouseButton::Left));
        if ctx {
            if let Some((side, row)) = self.hit(m.column, m.row) {
                self.select_at(side, row);
                self.open_menu(side, (m.column, m.row));
            }
            return;
        }
        match m.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                if let Some((side, _)) = self.hit(m.column, m.row) {
                    self.active = side;
                    self.move_sel(if m.kind == MouseEventKind::ScrollUp { -3 } else { 3 });
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let Some((side, row)) = self.hit(m.column, m.row) else { return };
                let top = self.pane_rects[if side == Side::Local { 0 } else { 1 }].y;
                if m.row == top {
                    self.title_click(side, m.column);
                    return;
                }
                self.select_at(side, row);
                if let Some(i) = row {
                    let now = std::time::Instant::now();
                    let dbl = matches!(self.last_click, Some((t, s, j)) if s == side && j == i && now.duration_since(t) < Duration::from_millis(450));
                    self.last_click = if dbl { None } else { Some((now, side, i)) };
                    if dbl {
                        self.enter();
                    }
                }
            }
            _ => {}
        }
    }

    /// Costruisce e mostra il menu contestuale per la voce sotto il cursore.
    fn open_menu(&mut self, side: Side, pos: (u16, u16)) {
        let other = if side == Side::Local { "device" } else { t!("locale") };
        let (entry, marked, cwd_name) = {
            let Some(p) = self.pane(side) else { return };
            (
                p.selected().cloned().filter(|e| e.name != ".."),
                p.marked.len(),
                dev_basename(&p.cwd).to_string(),
            )
        };
        let mut items: Vec<(String, MenuAction)> = Vec::new();
        let mut title = format!("«{cwd_name}»");
        if let Some(e) = &entry {
            title = if marked > 0 { t!("{marked} selezionati", marked = marked) } else { format!("«{}»", e.name) };
            let target = if marked > 0 { t!("{marked} selezionati", marked = marked) } else if e.is_dir { t!("cartella «{name}»", name = e.name) } else { format!("«{}»", e.name) };
            if e.is_dir && marked == 0 {
                items.push((t!("Apri cartella").into(), MenuAction::Open));
            }
            items.push((t!("Copia {target} → {other}", target = target, other = other), MenuAction::Transfer));
            items.push((t!("Marca / smarca").into(), MenuAction::Mark));
            if marked == 0 {
                items.push((t!("Rinomina…").into(), MenuAction::Rename));
            }
            items.push((t!("Elimina {target}…", target = target), MenuAction::Delete));
        }
        items.push((t!("Copia cartella corrente «{cwd_name}» intera → {other}", cwd_name = cwd_name, other = other), MenuAction::TransferWhole));
        if side == Side::Remote {
            items.push((t!("Sync incrementale cartella → locale").into(), MenuAction::Sync));
        }
        items.push((t!("Nuova cartella qui…").into(), MenuAction::Mkdir));
        items.push((t!("Seleziona tutto / niente").into(), MenuAction::MarkAll));
        items.push((t!("Ordina (nome/dimensione/data)").into(), MenuAction::Sort));
        items.push((t!("Mostra/nascondi file nascosti").into(), MenuAction::Hidden));
        items.push((t!("Aggiorna").into(), MenuAction::Refresh));
        // dimensione reale (ricorsiva) di voce/marcate/cartella corrente, in background
        let targets: Vec<(String, bool)> = if marked > 0 {
            self.pane(side).map(|p| p.marked.iter().map(|m| (m.path.clone(), m.is_dir)).collect()).unwrap_or_default()
        } else if let Some(e) = &entry {
            vec![(e.full.clone(), e.is_dir)]
        } else {
            let cwd = self.pane(side).map(|p| p.cwd.clone()).unwrap_or_default();
            vec![(cwd, true)]
        };
        self.menu_gen += 1;
        let gen = self.menu_gen;
        let (tx, rx) = channel();
        self.size_rx = Some(rx);
        let adb = self.adb.clone();
        std::thread::spawn(move || {
            let (mut bytes, mut files, mut dirs) = (0u64, 0u64, 0u64);
            for (path, is_dir) in &targets {
                if side == Side::Local {
                    let (b, f, d) = crate::util::local_totals(Path::new(path));
                    bytes += b;
                    files += f;
                    dirs += d;
                } else {
                    let q = crate::util::sh_quote(path);
                    let out = adb
                        .shell(&format!("toybox du -sb {q} 2>/dev/null; toybox find -H {q} -type f 2>/dev/null | toybox wc -l"))
                        .unwrap_or_default();
                    let mut it = out.lines();
                    bytes += it.next().and_then(|l| l.split_whitespace().next()).and_then(|t| t.parse::<u64>().ok()).unwrap_or(0);
                    files += it.next().and_then(|l| l.trim().parse::<u64>().ok()).unwrap_or(0);
                    if *is_dir {
                        dirs += 1;
                    }
                }
            }
            let mut txt = format!(
                "{} ({} byte)",
                crate::util::humanize(bytes),
                crate::util::fmt_int(bytes)
            );
            if dirs > 0 || files > 1 {
                txt.push_str(&t!(" · {n} file", n = crate::util::fmt_int(files)));
                if dirs > 0 {
                    txt.push_str(&format!(
                        " · {} {}",
                        crate::util::fmt_int(dirs),
                        if dirs == 1 { t!("cartella") } else { t!("cartelle") }
                    ));
                }
            }
            let _ = tx.send((gen, txt));
        });
        self.screen = Screen::Menu { items, sel: 0, pos, title, size: t!("calcolo dimensione…").into() };
    }

    fn run_menu_action(&mut self, a: MenuAction) {
        match a {
            MenuAction::Open => self.enter_dir_only(),
            MenuAction::Transfer => match self.active {
                Side::Remote => self.pull_jobs(),
                Side::Local => self.push_jobs(),
            },
            MenuAction::TransferWhole => self.transfer_whole_dir(),
            MenuAction::Sync => self.sync_job(),
            MenuAction::Mark => self.toggle_mark(),
            MenuAction::MarkAll => self.mark_all(),
            MenuAction::Rename => self.rename_request(),
            MenuAction::Delete => self.delete_request(),
            MenuAction::Mkdir => self.mkdir_request(),
            MenuAction::Refresh => {
                let s = self.active;
                self.refresh(s);
            }
            MenuAction::Sort => self.cycle_sort(),
            MenuAction::Hidden => self.toggle_hidden(),
        }
    }

    /// Porta il pannello su un percorso qualsiasi.
    pub fn goto(&mut self, side: Side, path: String) {
        let path = if path.len() > 1 { path.trim_end_matches('/').to_string() } else { path };
        if path.is_empty() {
            return;
        }
        if side == Side::Local && !Path::new(&path).is_dir() {
            self.log(t!("⚠ cartella non trovata: {path}", path = path));
            return;
        }
        let Some(p) = self.pane(side) else { return };
        if p.cwd == path {
            return;
        }
        // se si risale, il cursore torna sulla cartella da cui si proviene
        if dev_parent(&p.cwd) == path {
            p.goto_name = Some(dev_basename(&p.cwd).to_string());
        }
        p.cwd = path;
        p.marked.clear();
        p.filter.clear();
        p.raw.clear();
        p.entries.clear();
        p.sel = 0;
        p.list_state.select(Some(0));
        self.refresh(side);
    }

    /// Click sul titolo del pannello: un segmento del path → vai lì;
    /// il resto del titolo apre "Vai a…".
    fn title_click(&mut self, side: Side, x: u16) {
        let i = if side == Side::Local { 0 } else { 1 };
        let r = self.pane_rects[i];
        let (label, cwd) = match side {
            Side::Local => ("Locale", self.local.cwd.clone()),
            Side::Remote => match self.remote.as_ref() {
                Some(p) => ("Device", p.cwd.clone()),
                None => return,
            },
        };
        let prefix = 1 + label.chars().count() + 2;
        let col = x as i64 - (r.x as i64 + 1) - prefix as i64;
        if col >= 0 {
            if let Some((_, _, target)) = crate::places::crumbs(&cwd)
                .into_iter()
                .find(|(a, b, _)| (col as usize) >= *a && (col as usize) < *b)
            {
                self.active = side;
                self.goto(side, target);
                return;
            }
        }
        self.active = side;
        self.open_goto(side, (r.x + 2, r.y + 1));
    }

    /// `g`: elenco "Vai a…" con luoghi, preferiti e recenti.
    pub fn open_goto(&mut self, side: Side, pos: (u16, u16)) {
        let (places, cwd) = match side {
            Side::Local => (crate::places::local_places(), self.local.cwd.clone()),
            Side::Remote => match self.remote.as_ref() {
                Some(r) => (crate::places::remote_places(&self.adb), r.cwd.clone()),
                None => return,
            },
        };
        let mut items: Vec<crate::places::Place> = Vec::new();
        let mut seen = HashSet::new();
        let mut cur_idx = 0;
        for (l, p) in places {
            if seen.insert(p.clone()) {
                items.push((format!("▸ {l}  —  {p}"), p));
            }
        }
        for (s, p) in &self.saved.favs {
            if *s == side && seen.insert(p.clone()) {
                items.push((format!("★ {}  —  {p}", dev_basename(p)), p.clone()));
            }
        }
        for (s, p) in &self.saved.recents {
            if *s == side && seen.insert(p.clone()) {
                items.push((format!("↺ {}  —  {p}", dev_basename(p)), p.clone()));
            }
        }
        items.push((t!("✎ Digita un percorso…").into(), String::new()));
        if let Some(i) = items.iter().position(|(_, p)| *p == cwd) {
            cur_idx = i;
        }
        self.screen = Screen::Goto { side, items, sel: cur_idx, pos };
    }

    fn goto_choose(&mut self, side: Side, path: String) {
        self.screen = Screen::Files;
        if path.is_empty() {
            let cwd = match side {
                Side::Local => self.local.cwd.clone(),
                Side::Remote => self.remote.as_ref().map(|r| r.cwd.clone()).unwrap_or_default(),
            };
            self.screen = Screen::Input {
                title: t!("Vai al percorso").into(),
                value: cwd,
                action: InputAction::GotoPath(side),
            };
        } else {
            self.goto(side, path);
        }
    }

    /// `B`: aggiunge/rimuove la cartella corrente dai preferiti.
    pub fn toggle_favorite(&mut self) {
        let side = self.active;
        let cwd = match side {
            Side::Local => self.local.cwd.clone(),
            Side::Remote => match self.remote.as_ref() {
                Some(r) => r.cwd.clone(),
                None => return,
            },
        };
        let on = self.saved.toggle_fav(side, &cwd);
        self.log(if on { t!("★ preferito aggiunto: {cwd}", cwd = cwd) } else { t!("preferito rimosso: {cwd}", cwd = cwd) });
    }

    pub fn page(&mut self, dir: i64) {
        let step = 12;
        self.move_sel(dir * step);
    }

    /// `o`: cicla l'ordinamento del pannello attivo.
    pub fn cycle_sort(&mut self) {
        let side = self.active;
        let Some(p) = self.pane(side) else { return };
        p.sort = p.sort.next();
        rebuild(p);
        let l = p.sort.label();
        self.log(t!("ordinamento: {l}", l = l));
    }

    /// `.`: mostra/nasconde i file che iniziano con '.'.
    pub fn toggle_hidden(&mut self) {
        let side = self.active;
        let Some(p) = self.pane(side) else { return };
        p.show_hidden = !p.show_hidden;
        rebuild(p);
        let on = p.show_hidden;
        self.log(if on { t!("file nascosti: visibili") } else { t!("file nascosti: nascosti") });
    }

    /// `*`: inverte le marcature.
    pub fn invert_marks(&mut self) {
        let Some(p) = self.pane(self.active) else { return };
        let all: HashSet<MarkedItem> = p
            .entries
            .iter()
            .filter(|e| e.name != "..")
            .map(|e| MarkedItem { path: e.full.clone(), is_dir: e.is_dir })
            .collect();
        p.marked = all.difference(&p.marked).cloned().collect();
    }

    /// `/`: filtro incrementale per nome.
    pub fn filter_request(&mut self) {
        let side = self.active;
        let cur = self.pane(side).map(|p| p.filter.clone()).unwrap_or_default();
        self.screen = Screen::Input {
            title: t!("Filtro nome (vuoto = rimuovi)").into(),
            value: cur,
            action: InputAction::Filter(side),
        };
    }

    /// `R`/F2: rinomina la voce selezionata.
    pub fn rename_request(&mut self) {
        let side = self.active;
        let Some(p) = self.pane(side) else { return };
        let Some(e) = p.selected().cloned() else { return };
        if e.name == ".." {
            return;
        }
        self.screen = Screen::Input {
            title: t!("Rinomina «{name}»", name = e.name),
            value: e.name.clone(),
            action: InputAction::Rename(side, e.full),
        };
    }

    /// `S`: esporta SMS + call log nel pannello locale (thread separato).
    pub fn export_sms_calllog(&mut self) {
        if self.queue.is_none() {
            self.log(t!("⚠ collega prima un device"));
            return;
        }
        let adb = self.adb.clone();
        let cwd = self.local.cwd.trim_end_matches('/').to_string();
        let (tx, rx) = channel::<String>();
        self.msgs = Some(rx);
        std::thread::spawn(move || {
            let sink: Arc<Mutex<Sender<String>>> = Arc::new(Mutex::new(tx));
            let flag = Arc::new(AtomicBool::new(false));
            let stamp = fmt_date(now_secs()).replace(' ', "-").replace(':', "");
            let sms_f = format!("{cwd}/sms-{stamp}.jsonl");
            match actions::export_sms(&adb, &sms_f, &flag, &sink) {
                Ok(n) => {
                    let _ = sink.lock().unwrap().send(t!("SMS: {n} messaggi → {file}", n = n, file = sms_f));
                }
                Err(e) => {
                    let _ = sink.lock().unwrap().send(format!("⚠ SMS: {e}"));
                }
            }
            let call_f = format!("{cwd}/calllog-{stamp}.jsonl");
            match actions::export_calllog(&adb, &call_f, &flag, &sink) {
                Ok(n) => {
                    let _ = sink.lock().unwrap().send(t!("chiamate: {n} voci → {file}", n = n, file = call_f));
                }
                Err(e) => {
                    let _ = sink.lock().unwrap().send(t!("⚠ chiamate: {e}", e = e));
                }
            }
        });
        self.log(t!("esportazione SMS/call log avviata…"));
    }

    /// `x`: annulla il job in esecuzione.
    pub fn cancel_running(&mut self) {
        if let Some(q) = self.queue.as_ref() {
            if let Some(id) = q.running_id() {
                q.cancel(id);
                self.log(t!("annullamento richiesto…"));
            } else {
                self.log(t!("nessun trasferimento attivo"));
            }
        }
    }

    /// `X`: annulla tutti i trasferimenti ancora in coda (quello attivo prosegue).
    pub fn cancel_queued(&mut self) {
        let n = self.queue.as_ref().map(|q| q.cancel_queued()).unwrap_or(0);
        if n > 0 {
            self.log(t!("annullati {n} trasferimenti in coda", n = n));
        } else {
            self.log(t!("nessun trasferimento in coda"));
        }
    }

    fn dev_move(&mut self, d: i64) {
        if let Screen::Devices { sel } = &mut self.screen {
            let len = self.devices.len() as i64;
            let cur = *sel as i64;
            *sel = (cur + d).clamp(0, len.saturating_sub(1)) as usize;
        }
    }

    /// Nella schermata di selezione controlla ogni 2 s (in un thread) se è stato collegato/scollegato un device.
    fn poll_devices(&mut self) {
        const EVERY: Duration = Duration::from_secs(2);
        if let Some(rx) = &self.dev_rx {
            match rx.try_recv() {
                Ok(res) => {
                    self.dev_rx = None;
                    self.dev_last = std::time::Instant::now();
                    if let Ok(v) = res {
                        if v != self.devices {
                            let plugged = v.iter().find(|d| !self.devices.iter().any(|o| o.serial == d.serial)).map(|d| d.serial.clone());
                            self.devices = v;
                            let len = self.devices.len();
                            if let Screen::Devices { sel } = &mut self.screen {
                                *sel = (*sel).min(len.saturating_sub(1));
                            }
                            if let Some(serial) = plugged {
                                self.log(t!("device collegato: {serial}", serial = serial));
                            }
                        }
                    }
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => self.dev_rx = None,
            }
            return;
        }
        if matches!(self.screen, Screen::Devices { .. }) && self.dev_last.elapsed() >= EVERY {
            let adb = self.adb.clone();
            let (tx, rx) = channel();
            self.dev_rx = Some(rx);
            std::thread::spawn(move || {
                let _ = tx.send(adb.devices().map_err(|e| e.to_string()));
            });
        }
    }

    pub fn refresh_devices(&mut self) {
        match self.adb.devices() {
            Ok(v) => {
                let n = v.len();
                self.devices = v;
                self.log(t!("trovati {n} device", n = n));
            }
            Err(e) => self.log(format!("⚠ adb devices: {e}")),
        }
    }

    /// Dispatch dei tasti in base alla schermata corrente.
    pub fn on_key(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::KeyCode;
        if key.kind != crossterm::event::KeyEventKind::Press {
            return;
        }
        // Ctrl+C: in raw mode non arriva SIGINT, quindi lo gestiamo noi
        if key.code == crossterm::event::KeyCode::Char('c')
            && key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
        {
            self.cancel_running();
            self.quit = true;
            return;
        }
        // copia i dati della schermata per evitare conflitti di borrow
        enum Route {
            Devices { sel: usize },
            Files,
            Confirm(#[allow(dead_code)] ConfirmAction),
            Input(InputAction, String),
            Download,
            Help,
            Menu,
            Goto,
        }
        let route = match &self.screen {
            Screen::Devices { sel } => Route::Devices { sel: *sel },
            Screen::Files => Route::Files,
            Screen::Confirm { action, .. } => Route::Confirm(action.clone()),
            Screen::Input { action, value, .. } => Route::Input(action.clone(), value.clone()),
            Screen::Download => Route::Download,
            Screen::Help => Route::Help,
            Screen::Menu { .. } => Route::Menu,
            Screen::Goto { .. } => Route::Goto,
        };
        match route {
            Route::Devices { sel } => match key.code {
                KeyCode::Up | KeyCode::Char('k') => self.dev_move(-1),
                KeyCode::Down | KeyCode::Char('j') => self.dev_move(1),
                KeyCode::Enter => self.connect(sel),
                KeyCode::Char('c') => self.connect_request(),
                KeyCode::Char('a') => self.adb_path_request(),
                KeyCode::Char('d') => self.adb_download_request(),
                KeyCode::Char('r') => self.refresh_devices(),
                KeyCode::Char('q') => self.quit = true,
                _ => {}
            },
            Route::Files => match key.code {
                KeyCode::Char('q') => self.quit = true,
                KeyCode::Char('?') | KeyCode::F(1) => self.screen = Screen::Help,
                KeyCode::Esc => {
                    let s = self.active;
                    if let Some(p) = self.pane(s) {
                        if !p.filter.is_empty() {
                            p.filter.clear();
                            rebuild(p);
                        } else {
                            p.marked.clear();
                        }
                    }
                }
                KeyCode::PageUp => self.page(-1),
                KeyCode::PageDown => self.page(1),
                KeyCode::Char('T') => self.transfer_whole_dir(),
                KeyCode::Char('m') | KeyCode::Menu => {
                    let s = self.active;
                    let (w, h) = crossterm::terminal::size().unwrap_or((120, 40));
                    let r = self.pane_rects[if s == Side::Local { 0 } else { 1 }];
                    let row = self.pane(s).map(|p| p.sel.saturating_sub(p.list_state.offset())).unwrap_or(0) as u16;
                    let pos = ((r.x + 4).min(w.saturating_sub(1)), (r.y + 2 + row).min(h.saturating_sub(1)));
                    self.open_menu(s, pos);
                }
                KeyCode::Char('R') | KeyCode::F(2) => self.rename_request(),
                KeyCode::Char('o') => self.cycle_sort(),
                KeyCode::Char('.') => self.toggle_hidden(),
                KeyCode::Char('*') => self.invert_marks(),
                KeyCode::Char('/') => self.filter_request(),
                KeyCode::Delete => self.delete_request(),
                KeyCode::F(7) => self.mkdir_request(),
                KeyCode::Tab | KeyCode::BackTab => self.switch_active(),
                KeyCode::Right | KeyCode::Char('l') => self.enter_dir_only(),
                KeyCode::Up if key.modifiers.contains(crossterm::event::KeyModifiers::SHIFT) => {
                    self.toggle_mark_stay();
                    self.move_sel(-1);
                }
                KeyCode::Down if key.modifiers.contains(crossterm::event::KeyModifiers::SHIFT) => {
                    self.toggle_mark();
                }
                KeyCode::Insert => self.toggle_mark(),
                KeyCode::Char('K') => {
                    self.toggle_mark_stay();
                    self.move_sel(-1);
                }
                KeyCode::Char('J') => self.toggle_mark(),
                KeyCode::Up | KeyCode::Char('k') => self.move_sel(-1),
                KeyCode::Down | KeyCode::Char('j') => self.move_sel(1),
                KeyCode::Home => self.sel_home_end(false),
                KeyCode::Char('g') => {
                    let s = self.active;
                    let r = self.pane_rects[if s == Side::Local { 0 } else { 1 }];
                    self.open_goto(s, (r.x + 2, r.y + 1));
                }
                KeyCode::Char('B') => self.toggle_favorite(),
                KeyCode::End | KeyCode::Char('G') => self.sel_home_end(true),
                KeyCode::Left | KeyCode::Char('h') | KeyCode::Backspace => self.go_parent(),
                KeyCode::Enter => self.enter(),
                KeyCode::Char(' ') => self.toggle_mark(),
                KeyCode::Char('a') => self.mark_all(),
                KeyCode::Char('y') => self.pull_jobs(),
                KeyCode::Char('p') => self.push_jobs(),
                KeyCode::Char('s') => self.sync_job(),
                KeyCode::Char('d') => self.delete_request(),
                KeyCode::Char('n') => self.mkdir_request(),
                KeyCode::Char('r') => {
                    let s = self.active;
                    self.refresh(s);
                }
                KeyCode::Char('S') => self.export_sms_calllog(),
                KeyCode::Char('x') => self.cancel_running(),
                KeyCode::Char('X') => self.cancel_queued(),
                _ => {}
            },
            Route::Help => self.screen = Screen::Files,
            Route::Goto => match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    if let Screen::Goto { sel, items, .. } = &mut self.screen {
                        *sel = if *sel == 0 { items.len() - 1 } else { *sel - 1 };
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if let Screen::Goto { sel, items, .. } = &mut self.screen {
                        *sel = (*sel + 1) % items.len();
                    }
                }
                KeyCode::Enter => {
                    let pick = match &self.screen {
                        Screen::Goto { side, items, sel, .. } => items.get(*sel).map(|i| (*side, i.1.clone())),
                        _ => None,
                    };
                    if let Some((side, path)) = pick {
                        self.goto_choose(side, path);
                    } else {
                        self.screen = Screen::Files;
                    }
                }
                _ => self.screen = Screen::Files,
            },
            Route::Menu => match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    if let Screen::Menu { sel, items, .. } = &mut self.screen {
                        *sel = if *sel == 0 { items.len() - 1 } else { *sel - 1 };
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if let Screen::Menu { sel, items, .. } = &mut self.screen {
                        *sel = (*sel + 1) % items.len();
                    }
                }
                KeyCode::Enter => {
                    let act = match &self.screen {
                        Screen::Menu { items, sel, .. } => items.get(*sel).map(|i| i.1),
                        _ => None,
                    };
                    self.screen = Screen::Files;
                    if let Some(a) = act {
                        self.run_menu_action(a);
                    }
                }
                _ => self.screen = Screen::Files,
            },
            Route::Download => {
                if key.code == KeyCode::Esc {
                    if let Some(d) = &self.dl {
                        d.lock().unwrap().cancel = true;
                    }
                }
            }
            Route::Confirm(_) => match key.code {
                KeyCode::Char('y') | KeyCode::Enter => self.confirm(true),
                KeyCode::Char('n') | KeyCode::Esc => self.confirm(false),
                _ => {}
            },
            Route::Input(action, value) => match key.code {
                KeyCode::Esc => {
                    self.screen = if matches!(action, InputAction::AdbPath | InputAction::Connect) {
                        Screen::Devices { sel: 0 }
                    } else {
                        Screen::Files
                    }
                }
                KeyCode::Enter => self.submit_input(value),
                KeyCode::Backspace => {
                    let mut v = value;
                    v.pop();
                    if let Screen::Input { value, .. } = &mut self.screen {
                        *value = v;
                    }
                }
                KeyCode::Char(c)
                    if !key
                        .modifiers
                        .contains(crossterm::event::KeyModifiers::CONTROL) =>
                {
                    let mut v = value;
                    v.push(c);
                    if let Screen::Input { value, .. } = &mut self.screen {
                        *value = v;
                    }
                }
                _ => {}
            },
        }
    }
}

// Sink per il thread di esportazione: invia messaggi di stato al log della TUI.
impl crate::transfer::Sink for Arc<Mutex<Sender<String>>> {
    fn set_total(&self, _t: u64) {}
    fn set_bytes(&self, b: u64) {
        let _ = self.lock().unwrap().send(t!("righe esportate: {b}", b = b));
    }
    fn add_bytes(&self, _n: u64) {}
    fn msg(&self, m: &str) {
        let _ = self.lock().unwrap().send(m.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    fn click(kind: MouseEventKind, col: u16, row: u16) -> MouseEvent {
        MouseEvent { kind, column: col, row, modifiers: KeyModifiers::NONE }
    }

    #[test]
    fn right_click_opens_context_menu() {
        let tmp = std::env::temp_dir().join("abt-test-backup");
        std::env::set_var("ABT_BACKUP_DIR", &tmp);
        let mut app = App::new();
        app.screen = Screen::Files;
        app.pane_rects = [Rect { x: 0, y: 1, width: 60, height: 20 }, Rect { x: 60, y: 1, width: 60, height: 20 }];
        app.on_mouse(click(MouseEventKind::Down(MouseButton::Right), 5, 5));
        assert!(matches!(app.screen, Screen::Menu { .. }), "il click destro deve aprire il menu");

        // alternative per i terminali che intercettano il click destro
        for ev in [
            click(MouseEventKind::Down(MouseButton::Middle), 5, 5),
            MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: 5, row: 5, modifiers: KeyModifiers::CONTROL },
        ] {
            app.screen = Screen::Files;
            app.on_mouse(ev);
            assert!(matches!(app.screen, Screen::Menu { .. }), "{ev:?} deve aprire il menu");
        }

        // il click sinistro semplice non apre il menu
        app.screen = Screen::Files;
        app.on_mouse(click(MouseEventKind::Down(MouseButton::Left), 5, 5));
        assert!(matches!(app.screen, Screen::Files));
    }

    /// Un device collegato dopo l'avvio compare da solo nella schermata di selezione.
    #[test]
    fn device_plugged_after_start_appears_automatically() {
        let dir = std::env::temp_dir().join("abt-test-fakeadb");
        let _ = std::fs::create_dir_all(&dir);
        let flag = dir.join("plugged");
        let _ = std::fs::remove_file(&flag);
        // adb finto: elenca un device solo se esiste il file "plugged"
        let bin = if cfg!(windows) {
            let p = dir.join("fakeadb.cmd");
            let script = format!(
                "@echo off\r\necho List of devices attached\r\nif exist \"{}\" echo ABC123 device model:Pixel\r\n",
                flag.display()
            );
            std::fs::write(&p, script).unwrap();
            p
        } else {
            let p = dir.join("fakeadb.sh");
            let script = format!(
                "#!/bin/sh\necho 'List of devices attached'\n[ -f '{}' ] && echo 'ABC123 device model:Pixel'\nexit 0\n",
                flag.display()
            );
            std::fs::write(&p, script).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            p
        };
        std::env::set_var("ABT_BACKUP_DIR", dir.join("bk"));
        std::env::set_var("ADB_BIN", &bin);
        let mut app = App::new();
        assert!(app.devices.is_empty());
        app.screen = Screen::Devices { sel: 0 };
        app.dev_last = std::time::Instant::now() - Duration::from_secs(10);
        std::fs::write(&flag, "1").unwrap(); // "collego" il telefono dopo l'avvio
        for _ in 0..200 {
            app.poll();
            if !app.devices.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let _ = std::fs::remove_file(&flag);
        assert_eq!(app.devices.len(), 1);
        assert_eq!(app.devices[0].serial, "ABC123");
    }
}
