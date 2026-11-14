//! Stato dell'applicazione TUI: schermate, pannelli, selezioni, coda.

use crate::actions;
use crate::adb::{Adb, Device};
use crate::device_fs::{self, Entry};
use crate::transfer::{Direction, JobKind, JobSpec, Queue};
use crate::util::{dev_basename, dev_join, dev_parent, fmt_date, now_secs};
use anyhow::Result;
use ratatui::widgets::ListState;
use std::collections::{HashSet, VecDeque};
use std::path::Path;
use std::sync::atomic::AtomicBool;
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
}

#[derive(Debug, Clone)]
pub enum ConfirmAction {
    DeleteDevice(Vec<String>),
    DeleteLocal(Vec<String>),
}

#[derive(Debug, Clone)]
pub enum InputAction {
    Mkdir(Side),
    Rename(Side, String),
    Filter(Side),
    Connect,
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
            SortMode::Name => "nome",
            SortMode::Size => "dimensione",
            SortMode::Date => "data",
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
            log.push_back(format!("⚠ cartella backup: {e}"));
            std::env::current_dir()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| "/".into())
        }
    }
}

impl App {
    pub fn new() -> Self {
        let bin = std::env::var("ADB_BIN").unwrap_or_else(|_| "adb".into());
        let adb = Adb::new(bin);
        let mut devices = Vec::new();
        let mut log = VecDeque::new();
        match adb.devices() {
            Ok(v) => devices = v,
            Err(e) => {
                log.push_back(format!("⚠ adb: {e}"));
                log.push_back("⚠ installa adb (es. `apt install adb`) o imposta ADB_BIN".into());
            }
        }
        let cwd = default_backup_dir(&mut log);
        let local = Pane::new(cwd);
        Self {
            adb,
            devices,
            screen: Screen::Devices { sel: 0 },
            local,
            remote: None,
            queue: None,
            active: Side::Local,
            log,
            info: None,
            pending: None,
            msgs: None,
            quit: false,
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
            self.log(format!(
                "⚠ {}: stato '{}' — sblocca il telefono e conferma la finestra RSA, poi riprova",
                dev.serial, dev.state
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
        self.log(format!("device connesso: {}", dev.serial));
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
            Side::Local => Path::new(&cwd)
                .parent()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|| "/".into()),
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
            self.log("⚠ collega prima un device");
            return;
        }
        // y/p sulla voce ".." = trasferisci la cartella corrente intera
        let on_dotdot = !whole
            && self.pane(from).map(|p| p.marked.is_empty() && p.selected().map(|e| e.name == "..").unwrap_or(false)).unwrap_or(false);
        let whole = whole || on_dotdot;
        let items = self.transfer_items(from, whole);
        if items.is_empty() {
            self.log("niente da trasferire: seleziona o marca qualche voce (spazio)");
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
        let (verb, arrow) = if from == Side::Remote { ("Pull", "→ locale") } else { ("Push", "→ device") };
        let what = if whole {
            let n = items.first().map(|i| dev_basename(&i.path).to_string()).unwrap_or_default();
            format!("cartella intera «{n}»")
        } else {
            format!("{dirs} cartelle, {files} file (≥ {})", crate::util::humanize(bytes))
        };
        self.screen = Screen::Input {
            title: format!("{verb} {arrow}: {what} — cartella di destinazione"),
            value: dest,
            action: InputAction::Transfer(from, whole),
        };
    }

    fn pull_to(&mut self, local_cwd: String, whole: bool) {
        let Some(queue) = self.queue.clone() else { return };
        let items = self.transfer_items(Side::Remote, whole);
        let local_cwd = local_cwd.trim_end_matches('/').to_string();
        if let Err(e) = std::fs::create_dir_all(&local_cwd) {
            self.log(format!("⚠ destinazione {local_cwd}: {e}"));
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
        self.log(format!("accodati {n} pull → {local_cwd}"));
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
            self.log("⚠ destinazione non valida");
            return;
        }
        if let Err(e) = device_fs::mkdir_device(&self.adb, &dst_dir) {
            self.log(format!("⚠ destinazione {dst_dir}: {e}"));
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
        self.log(format!("accodati {n} push → {dst_dir}"));
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
        self.log(format!("avviato sync: {} → {dst}", remote.cwd));
    }

    /// `d`: richiede conferma di cancellazione degli elementi del pannello attivo.
    pub fn delete_request(&mut self) {
        let side = self.active;
        let items = self.selection_items(side);
        if items.is_empty() {
            self.log("niente da eliminare");
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
        let where_ = if side == Side::Remote { "DEVICE" } else { "locale" };
        self.screen = Screen::Confirm {
            title: "Conferma cancellazione".into(),
            message: format!(
                "Eliminare da {where_}: {dirs} cartelle (con contenuto) e {files} file ({}) ?\n{preview}\n\nOperazione irreversibile.",
                crate::util::humanize(bytes)
            ),
            action,
        };
    }

    pub fn confirm(&mut self, yes: bool) {
        let action = match &self.screen {
            Screen::Confirm { action, .. } => action.clone(),
            _ => return,
        };
        self.screen = Screen::Files;
        if !yes {
            return;
        }
        match action {
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
                self.log(format!("eliminati {ok} elementi sul device"));
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
                self.log(format!("eliminati {ok} elementi in locale"));
                self.refresh(Side::Local);
            }
        }
    }

    /// `n`: richiesta nome nuova cartella nel pannello attivo.
    pub fn mkdir_request(&mut self) {
        let side = self.active;
        self.screen = Screen::Input {
            title: match side {
                Side::Local => "Nuova cartella in locale".into(),
                Side::Remote => "Nuova cartella sul device".into(),
            },
            value: String::new(),
            action: InputAction::Mkdir(side),
        };
    }

    /// Schermata device: `c` -> input indirizzo Wi-Fi adb.
    pub fn connect_request(&mut self) {
        self.screen = Screen::Input {
            title: "adb connect IP:PORT (es. 192.168.1.5:5555)".into(),
            value: String::new(),
            action: InputAction::Connect,
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
            InputAction::Mkdir(side) => {
                if v.is_empty() || v.contains('/') || v.split('/').any(|c| c == "..") {
                    self.log(format!("⚠ nome non valido: {v:?}"));
                    return;
                }
                match side {
                    Side::Local => {
                        let p = format!("{}/{}", self.local.cwd.trim_end_matches('/'), v);
                        match device_fs::mkdir_local(&p) {
                            Ok(()) => {
                                self.log(format!("creata {p}"));
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
                                self.log(format!("creata {p}"));
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
            InputAction::Filter(side) => {
                if let Some(p) = self.pane(side) {
                    p.filter = v;
                    rebuild(p);
                }
            }
            InputAction::Rename(side, old) => {
                if v.is_empty() || v.contains('/') || v == "." || v == ".." {
                    self.log(format!("⚠ nome non valido: {v:?}"));
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
                        self.log(format!("rinominato in {v}"));
                        if let Some(p) = self.pane(side) {
                            p.marked.clear();
                            p.goto_name = Some(v);
                        }
                        self.refresh(side);
                    }
                    Err(e) => self.log(format!("⚠ rinomina: {e}")),
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
        self.log(format!("ordinamento: {l}"));
    }

    /// `.`: mostra/nasconde i file che iniziano con '.'.
    pub fn toggle_hidden(&mut self) {
        let side = self.active;
        let Some(p) = self.pane(side) else { return };
        p.show_hidden = !p.show_hidden;
        rebuild(p);
        let on = p.show_hidden;
        self.log(if on { "file nascosti: visibili" } else { "file nascosti: nascosti" });
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
            title: "Filtro nome (vuoto = rimuovi)".into(),
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
            title: format!("Rinomina «{}»", e.name),
            value: e.name.clone(),
            action: InputAction::Rename(side, e.full),
        };
    }

    /// `S`: esporta SMS + call log nel pannello locale (thread separato).
    pub fn export_sms_calllog(&mut self) {
        if self.queue.is_none() {
            self.log("⚠ collega prima un device");
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
                    let _ = sink.lock().unwrap().send(format!("SMS: {n} messaggi → {sms_f}"));
                }
                Err(e) => {
                    let _ = sink.lock().unwrap().send(format!("⚠ SMS: {e}"));
                }
            }
            let call_f = format!("{cwd}/calllog-{stamp}.jsonl");
            match actions::export_calllog(&adb, &call_f, &flag, &sink) {
                Ok(n) => {
                    let _ = sink.lock().unwrap().send(format!("chiamate: {n} voci → {call_f}"));
                }
                Err(e) => {
                    let _ = sink.lock().unwrap().send(format!("⚠ chiamate: {e}"));
                }
            }
        });
        self.log("esportazione SMS/call log avviata…");
    }

    /// `x`: annulla il job in esecuzione.
    pub fn cancel_running(&mut self) {
        if let Some(q) = self.queue.as_ref() {
            if let Some(id) = q.running_id() {
                q.cancel(id);
                self.log("annullamento richiesto…");
            } else {
                self.log("nessun trasferimento attivo");
            }
        }
    }

    fn dev_move(&mut self, d: i64) {
        if let Screen::Devices { sel } = &mut self.screen {
            let len = self.devices.len() as i64;
            let cur = *sel as i64;
            *sel = (cur + d).clamp(0, len.saturating_sub(1)) as usize;
        }
    }

    pub fn refresh_devices(&mut self) {
        match self.adb.devices() {
            Ok(v) => {
                let n = v.len();
                self.devices = v;
                self.log(format!("trovati {n} device"));
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
            Help,
        }
        let route = match &self.screen {
            Screen::Devices { sel } => Route::Devices { sel: *sel },
            Screen::Files => Route::Files,
            Screen::Confirm { action, .. } => Route::Confirm(action.clone()),
            Screen::Input { action, value, .. } => Route::Input(action.clone(), value.clone()),
            Screen::Help => Route::Help,
        };
        match route {
            Route::Devices { sel } => match key.code {
                KeyCode::Up | KeyCode::Char('k') => self.dev_move(-1),
                KeyCode::Down | KeyCode::Char('j') => self.dev_move(1),
                KeyCode::Enter => self.connect(sel),
                KeyCode::Char('c') => self.connect_request(),
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
                KeyCode::Home | KeyCode::Char('g') => self.sel_home_end(false),
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
                _ => {}
            },
            Route::Help => self.screen = Screen::Files,
            Route::Confirm(_) => match key.code {
                KeyCode::Char('y') | KeyCode::Enter => self.confirm(true),
                KeyCode::Char('n') | KeyCode::Esc => self.confirm(false),
                _ => {}
            },
            Route::Input(_action, value) => match key.code {
                KeyCode::Esc => self.screen = Screen::Files,
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
        let _ = self.lock().unwrap().send(format!("righe esportate: {b}"));
    }
    fn add_bytes(&self, _n: u64) {}
    fn msg(&self, m: &str) {
        let _ = self.lock().unwrap().send(m.to_string());
    }
}
