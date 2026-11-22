//! `abt` — Android Backup Tool: file manager TUI via adb + comandi CLI.
//!
//! Esempi:
//!   abt                     # TUI
//!   abt devices             # elenco device
//!   abt connect 192.168.1.5:5555
//!   abt wifi                # passa l'adbd del device a tcpip (da USB)
//!   abt pull  /sdcard/DCIM ./backup [--verify]
//!   abt push  ./foto        /sdcard/Pictures
//!   abt sync  /sdcard/DCIM  ./backup/DCIM [--delete]
//!   abt apk list | abt apk pull com.whatsapp ./apk
//!   abt sms ./sms.jsonl | abt calllog ./chiamate.jsonl
//!   abt appdata com.example ./appdata

#[macro_use]
mod i18n;
mod i18n_en;
mod actions;
mod adb;
mod app;
mod device_fs;
mod transfer;
mod places;
mod ui;
mod util;

use adb::Adb;
use anyhow::{bail, Result};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use transfer::Sink;
use util::humanize;

fn main() {
    if let Err(e) = run() {
        eprintln!("{} {e:#}", t!("errore:"));
        std::process::exit(1);
    }
}

fn adb_bin() -> String {
    match adb::locate() {
        Ok(b) => b,
        Err(b) => {
            eprintln!("{}", adb::missing_hint());
            b
        }
    }
}

/// Opzioni globali della CLI.
#[derive(Default)]
struct Opts {
    serial: Option<String>,
    verify: bool,
    delete: bool,
    yes: bool,
    long: bool,
    out: Option<String>,
}

/// Estrae le opzioni (`-s`, `--verify`, `--delete`, `-y`, `-l`, `-o`) dagli argomenti.
fn parse_args(args: Vec<String>) -> (Opts, Vec<String>) {
    let mut o = Opts::default();
    let mut rest = Vec::new();
    let mut it = args.into_iter().peekable();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-s" | "--serial" if it.peek().is_some() => o.serial = it.next(),
            "-o" | "--out" if it.peek().is_some() => o.out = it.next(),
            "--verify" => o.verify = true,
            "--delete" => o.delete = true,
            "-y" | "--yes" => o.yes = true,
            "-l" => o.long = true,
            _ => rest.push(a),
        }
    }
    (o, rest)
}

fn resolve_serial(adb: &mut Adb, want: Option<String>) -> Result<()> {
    if let Some(s) = want {
        adb.serial = Some(s);
        return Ok(());
    }
    let devs = adb.devices()?;
    let ok: Vec<_> = devs.iter().filter(|d| d.state == "device").collect();
    match ok.len() {
        0 => bail!(t!(
            "nessun device adb in stato 'device' — collega via USB con debug USB attivo, usa 'abt devices', oppure 'abt connect IP:5555'"
        )),
        1 => {
            adb.serial = Some(ok[0].serial.clone());
            Ok(())
        }
        n => bail!(t!(
            "{n} device collegati: specifica -s SERIAL ({list})",
            n = n,
            list = ok.iter().map(|d| d.serial.clone()).collect::<Vec<_>>().join(", ")
        )),
    }
}

fn new_flag() -> AtomicBool {
    AtomicBool::new(false)
}

/// Sink CLI: stampa progresso su stderr.
struct CliSink {
    label: String,
    total: AtomicU64,
    bytes: AtomicU64,
    last: Mutex<Instant>,
}

impl CliSink {
    fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            total: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            last: Mutex::new(Instant::now() - Duration::from_secs(1)),
        }
    }
    fn print(&self, force: bool) {
        let mut last = self.last.lock().unwrap();
        if !force && last.elapsed() < Duration::from_millis(400) {
            return;
        }
        *last = Instant::now();
        let b = self.bytes.load(Ordering::Relaxed);
        let t = self.total.load(Ordering::Relaxed);
        if t > 0 {
            eprint!(
                "\r{}  {}/{} ({:.0}%)   ",
                self.label,
                humanize(b),
                humanize(t),
                b * 100 / t.max(1)
            );
        } else {
            eprint!("\r{}  {}   ", self.label, humanize(b));
        }
    }
}

impl Sink for CliSink {
    fn set_total(&self, t: u64) {
        self.total.store(t, Ordering::Relaxed);
    }
    fn set_bytes(&self, b: u64) {
        self.bytes.store(b, Ordering::Relaxed);
        self.print(false);
    }
    fn add_bytes(&self, n: u64) {
        self.bytes.fetch_add(n, Ordering::Relaxed);
        self.print(false);
    }
    fn msg(&self, m: &str) {
        eprint!("\r{}  — {m}   ", self.label);
    }
}

fn run() -> Result<()> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.is_empty() {
        return tui();
    }
    let (opts, rest) = parse_args(argv);
    let (serial, verify, delete) = (opts.serial.clone(), opts.verify, opts.delete);
    let cmd = rest.first().cloned().unwrap_or_else(|| "help".into());
    let rest: Vec<String> = rest.into_iter().skip(1).collect();

    match cmd.as_str() {
        "help" | "--help" | "-h" => {
            print_help();
            Ok(())
        }
        "version" | "--version" | "-V" => {
            println!("{}", version_line());
            Ok(())
        }
        "install-adb" => {
            use std::sync::Arc;
            let st = Arc::new(Mutex::new(adb::DlState::default()));
            let st2 = st.clone();
            let h = std::thread::spawn(move || {
                let r = adb::download_platform_tools(&st2).map_err(|e| format!("{e:#}"));
                st2.lock().unwrap().result = Some(r);
            });
            loop {
                std::thread::sleep(Duration::from_millis(300));
                let g = st.lock().unwrap();
                if g.result.is_some() {
                    break;
                }
                if g.extracting {
                    eprint!("\r{}   ", t!("estrazione in corso…"));
                } else if g.total > 0 {
                    eprint!("\r{} / {} ({}%)   ", humanize(g.done), humanize(g.total), g.done * 100 / g.total.max(1));
                } else {
                    eprint!("\r{} {}   ", t!("download in corso…"), humanize(g.done));
                }
            }
            let _ = h.join();
            eprintln!();
            let res = st.lock().unwrap().result.take().unwrap();
            match res {
                Ok(bin) => {
                    adb::save_path(&bin);
                    println!("{}", t!("adb scaricato: {bin}", bin = bin));
                    Ok(())
                }
                Err(e) => bail!(e),
            }
        }
        "speedtest" => {
            const MBS: u64 = 256;
            let mut a = Adb::new(adb_bin());
            resolve_serial(&mut a, serial)?;
            println!("{}", t!("Test di velocità ({mb} MB per prova)…", mb = MBS));

            let link_cmd = format!("toybox dd if=/dev/zero bs=1048576 count={MBS} 2>/dev/null");
            let (_, link) = transfer::bench_exec_out(&a, &link_cmd)?;
            println!("  {:<46} {:>7.1} MB/s", t!("Collegamento telefono → PC (USB/Wi-Fi)"), link);

            // file grande sul telefono: indicato dall'utente oppure cercato in DCIM/Download/Movies
            let file = match rest.first() {
                Some(f) => Some(f.clone()),
                None => a
                    .shell("toybox find /sdcard/DCIM /sdcard/Download /sdcard/Movies -type f -size +100M 2>/dev/null | toybox head -n 1")
                    .ok()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty()),
            };
            let storage = match &file {
                Some(f) => {
                    let cmd = format!("toybox dd if={} bs=1048576 count={MBS} 2>/dev/null", util::sh_quote(f));
                    let (n, v) = transfer::bench_exec_out(&a, &cmd)?;
                    println!("  {:<46} {:>7.1} MB/s  ({})", t!("Lettura memoria del telefono"), v, humanize(n));
                    Some(v)
                }
                None => {
                    println!("  {}", t!("nessun file grande trovato sul telefono: indicane uno con 'abt speedtest <file>'"));
                    None
                }
            };

            let dir = match opts.out.clone() {
                Some(d) => d,
                None => util::default_backup_dir().map_err(|e| anyhow::anyhow!(e))?,
            };
            let disk = transfer::bench_local_write(&dir, MBS)?;
            println!("  {:<46} {:>7.1} MB/s  ({dir})", t!("Scrittura sul disco del PC"), disk);

            let mut parts = vec![("link", link), ("disk", disk)];
            if let Some(v) = storage {
                parts.push(("storage", v));
            }
            let (slow, _) = parts.iter().cloned().fold(("", f64::MAX), |m, p| if p.1 < m.1 { p } else { m });
            println!();
            match slow {
                "link" => {
                    println!("{}", t!("Collo di bottiglia: il collegamento."));
                    println!("  {}", t!("Cavo o porta USB 2.0, oppure Wi-Fi? Prova un cavo USB 3 / un'altra porta (meglio sul retro del PC), modalità «trasferimento file» sul telefono, niente hub."));
                }
                "storage" => {
                    println!("{}", t!("Collo di bottiglia: la memoria del telefono."));
                    println!("  {}", t!("La lettura dal telefono è lenta (memoria piena o molti file piccoli): trasferire una cartella alla volta può andare meglio di tanti file singoli."));
                }
                _ => {
                    println!("{}", t!("Collo di bottiglia: il disco del PC."));
                    println!("  {}", t!("Disco lento o antivirus in tempo reale: prova un SSD o escludi la cartella di backup da Windows Defender."));
                }
            }
            Ok(())
        }
        "tui" | "ui" => tui(),
        "devices" => {
            let a = Adb::new(adb_bin());
            for d in a.devices()? {
                let model = d.model.unwrap_or_default();
                println!("{:<24} {:<12} {model}", d.serial, d.state);
            }
            Ok(())
        }
        "connect" => {
            let addr = rest
                .first()
                .ok_or_else(|| anyhow::anyhow!(t!("uso: abt connect IP:5555")))?;
            let a = Adb::new(adb_bin());
            let out = a.connect(addr)?;
            println!("{out}");
            Ok(())
        }
        "wifi" => {
            let mut a = Adb::new(adb_bin());
            resolve_serial(&mut a, serial)?;
            let out = a.tcpip(5555)?;
            println!("{out}");
            println!("{}", t!("ora collega via Wi-Fi:  abt connect <IP-DEL-TELEFONO>:5555"));
            Ok(())
        }
        "pull" => {
            // abt pull <remoto>... [<locale>]   (-o DIR per la destinazione; default ~/Android_Backup/Backup)
            let (srcs, dst) = split_dest(&rest, opts.out.clone(), true)?;
            let mut a = Adb::new(adb_bin());
            resolve_serial(&mut a, serial)?;
            let flag = new_flag();
            for src in &srcs {
                let sink = CliSink::new(format!("pull {src}"));
                if device_fs::is_dir(&a, src)? {
                    transfer::pull_dir(&a, src, &dst, &flag, &sink)?;
                } else {
                    let name = src.rsplit('/').next().unwrap_or(src);
                    let target = format!("{}/{}", dst.trim_end_matches('/'), name);
                    transfer::pull_file(&a, src, &target, verify, &flag, &sink)?;
                }
                sink.print(true);
                eprintln!();
            }
            println!("{}", t!("{n} elementi → {dst}", n = srcs.len(), dst = dst));
            Ok(())
        }
        "push" => {
            // abt push <locale>... <cartella-remota>
            if rest.len() < 2 {
                bail!(t!("uso: abt push <locale>... <cartella-remota>"));
            }
            let (dst, srcs) = rest.split_last().unwrap();
            let mut a = Adb::new(adb_bin());
            resolve_serial(&mut a, serial)?;
            device_fs::mkdir_device(&a, dst)?;
            let flag = new_flag();
            for src in srcs {
                let sink = CliSink::new(format!("push {src}"));
                if std::fs::metadata(src)?.is_dir() {
                    transfer::push_dir(&a, src, dst, &flag, &sink)?;
                } else {
                    transfer::push_file(&a, src, dst, &flag, &sink)?;
                }
                sink.print(true);
                eprintln!();
            }
            println!("{}", t!("{n} elementi → {dst}", n = srcs.len(), dst = dst));
            Ok(())
        }
        "ls" => {
            let path = rest.first().cloned().unwrap_or_else(|| "/sdcard".into());
            let mut a = Adb::new(adb_bin());
            resolve_serial(&mut a, serial)?;
            let mut v = device_fs::list(&a, &path)?;
            v.retain(|e| e.name != "..");
            for e in v {
                if opts.long {
                    println!(
                        "{} {:>9}  {}  {}{}",
                        if e.is_dir { "d" } else { "-" },
                        if e.is_dir { "—".to_string() } else { humanize(e.size) },
                        util::fmt_date(e.mtime),
                        e.name,
                        if e.is_dir { "/" } else { "" }
                    );
                } else {
                    println!("{}{}", e.name, if e.is_dir { "/" } else { "" });
                }
            }
            Ok(())
        }
        "disks" => {
            for d in places::mounted_disks() {
                println!(
                    "{:<24} {:>10} {} {:>10}  {}",
                    d.name,
                    humanize(d.avail),
                    t!("liberi di"),
                    humanize(d.total),
                    d.mount
                );
            }
            Ok(())
        }
        "mkdir" => {
            let p = rest.first().ok_or_else(|| anyhow::anyhow!(t!("uso: abt mkdir <remoto>")))?;
            let mut a = Adb::new(adb_bin());
            resolve_serial(&mut a, serial)?;
            device_fs::mkdir_device(&a, p)?;
            println!("{}", t!("creata {p}", p = p));
            Ok(())
        }
        "mv" | "rename" => {
            let (old, new) = two_args(&rest)?;
            let mut a = Adb::new(adb_bin());
            resolve_serial(&mut a, serial)?;
            device_fs::rename_device(&a, &old, &new)?;
            println!("{old} → {new}");
            Ok(())
        }
        "rm" => {
            if rest.is_empty() {
                bail!(t!("uso: abt rm [-y] <remoto>..."));
            }
            let mut a = Adb::new(adb_bin());
            resolve_serial(&mut a, serial)?;
            if !opts.yes {
                eprint!("{}", t!("Eliminare DAL DEVICE {list} (irreversibile)? [s/N] ", list = rest.join(", ")));
                let mut l = String::new();
                std::io::stdin().read_line(&mut l)?;
                if !matches!(l.trim().to_lowercase().as_str(), "s" | "si" | "sì" | "y" | "yes") {
                    bail!(transfer::Cancelled);
                }
            }
            for p in &rest {
                device_fs::rm_device(&a, p)?;
                println!("{}", t!("eliminato {p}", p = p));
            }
            Ok(())
        }
        "backup" => {
            // abt backup [-o DIR] [<remoto>...]  — sync incrementale delle cartelle comuni + SMS/chiamate
            let out = match opts.out.clone() {
                Some(o) => o,
                None => util::default_backup_dir().map_err(|e| anyhow::anyhow!(e))?,
            };
            let defaults = ["/sdcard/DCIM", "/sdcard/Pictures", "/sdcard/Documents", "/sdcard/Download", "/sdcard/WhatsApp"];
            let srcs: Vec<String> = if rest.is_empty() {
                defaults.iter().map(|s| s.to_string()).collect()
            } else {
                rest.clone()
            };
            let mut a = Adb::new(adb_bin());
            resolve_serial(&mut a, serial)?;
            let flag = new_flag();
            for src in &srcs {
                if !device_fs::is_dir(&a, src).unwrap_or(false) {
                    eprintln!("{}", t!("salto {src}: non esiste", src = src));
                    continue;
                }
                let name = src.rsplit('/').next().unwrap_or(src);
                let dst = format!("{}/{name}", out.trim_end_matches('/'));
                let sink = CliSink::new(format!("backup {src}"));
                let st = transfer::sync_mirror(&a, src, &dst, delete, &flag, &sink)?;
                sink.print(true);
                eprintln!();
                println!("{}", t!("{src}: {files} file nuovi/aggiornati, {size}", src = src, files = st.files, size = humanize(st.bytes)));
            }
            if rest.is_empty() {
                for (c, f) in [("sms", "sms.jsonl"), ("calllog", "calllog.jsonl")] {
                    let path = format!("{}/{f}", out.trim_end_matches('/'));
                    let sink = CliSink::new(c);
                    let r = if c == "sms" {
                        actions::export_sms(&a, &path, &flag, &sink)
                    } else {
                        actions::export_calllog(&a, &path, &flag, &sink)
                    };
                    match r {
                        Ok(n) => println!("{}", t!("{c}: {n} record → {path}", c = c, n = n, path = path)),
                        Err(e) => eprintln!("{c}: {e}"),
                    }
                }
            }
            println!("{}", t!("backup in {out}", out = out));
            Ok(())
        }
        "sync" => {
            let (src, dst) = two_args(&rest)?;
            let mut a = Adb::new(adb_bin());
            resolve_serial(&mut a, serial)?;
            let sink = CliSink::new(format!("sync {src}"));
            let flag = new_flag();
            let st = transfer::sync_mirror(&a, &src, &dst, delete, &flag, &sink)?;
            sink.print(true);
            eprintln!();
            println!(
                "{}",
                t!(
                    "sync completato: {files} file, {deleted} rimossi, {size}",
                    files = st.files,
                    deleted = st.deleted,
                    size = humanize(st.bytes)
                )
            );
            Ok(())
        }
        "apk" => match rest.first().map(|s| s.as_str()) {
            Some("list") => {
                let mut a = Adb::new(adb_bin());
                resolve_serial(&mut a, serial)?;
                for apk in actions::apk_list(&a)? {
                    println!("{}  {}", apk.package, t!("({n} file)", n = apk.paths.len()));
                }
                Ok(())
            }
            Some("pull") => {
                let (pkg, out) = two_args(&rest[1..])?;
                let mut a = Adb::new(adb_bin());
                resolve_serial(&mut a, serial)?;
                let sink = CliSink::new(format!("apk {pkg}"));
                let flag = new_flag();
                let n = actions::apk_pull(&a, &pkg, &out, &flag, &sink)?;
                sink.print(true);
                eprintln!();
                println!("{}", t!("apk {pkg}: {n} file → {out}/{pkg}", pkg = pkg, n = n, out = out));
                Ok(())
            }
            _ => bail!(t!("uso: abt apk list | abt apk pull <pkg> <outdir>")),
        },
        "sms" | "calllog" => {
            let out = rest
                .first()
                .ok_or_else(|| anyhow::anyhow!(t!("uso: abt {cmd} <out.jsonl>", cmd = cmd)))?;
            let mut a = Adb::new(adb_bin());
            resolve_serial(&mut a, serial)?;
            let sink = CliSink::new(cmd.clone());
            let flag = new_flag();
            let n = if cmd == "sms" {
                actions::export_sms(&a, out, &flag, &sink)?
            } else {
                actions::export_calllog(&a, out, &flag, &sink)?
            };
            println!("{}", t!("{cmd}: {n} record → {out}", cmd = cmd, n = n, out = out));
            Ok(())
        }
        "appdata" => {
            let (pkg, out) = two_args(&rest)?;
            let mut a = Adb::new(adb_bin());
            resolve_serial(&mut a, serial)?;
            let sink = CliSink::new(format!("appdata {pkg}"));
            let flag = new_flag();
            actions::appdata_pull(&a, &pkg, &out, &flag, &sink)?;
            println!("appdata {pkg} → {out}/{pkg}");
            Ok(())
        }
        other => bail!(t!("comando sconosciuto: {other} (usa 'abt help')", other = other)),
    }
}

/// Separa sorgenti e destinazione: ultimo argomento = destinazione se ≥ 2 argomenti,
/// altrimenti `-o` o la cartella di default.
fn split_dest(rest: &[String], out: Option<String>, local: bool) -> Result<(Vec<String>, String)> {
    if rest.is_empty() {
        bail!(t!("uso: abt pull <remoto>... [<locale>]"));
    }
    if let Some(o) = out {
        return Ok((rest.to_vec(), o));
    }
    if rest.len() >= 2 {
        let (d, s) = rest.split_last().unwrap();
        return Ok((s.to_vec(), d.clone()));
    }
    let d = if local {
        util::default_backup_dir().map_err(|e| anyhow::anyhow!(e))?
    } else {
        "/sdcard".into()
    };
    Ok((rest.to_vec(), d))
}

fn two_args(rest: &[String]) -> Result<(String, String)> {
    if rest.len() < 2 {
        bail!(t!("servono due argomenti <sorgente> <destinazione>"));
    }
    Ok((rest[0].clone(), rest[1].clone()))
}

/// Versione e copyright, ad es. `abt 0.3.0 — © 2026 ZioZoni95 — licenza MIT`.
fn version_line() -> String {
    format!("abt {} — © 2026 ZioZoni95 — {}", env!("CARGO_PKG_VERSION"), t!("licenza MIT"))
}

fn print_help() {
    for l in util::LOGO {
        println!("\x1b[32m{l}\x1b[0m");
    }
    println!("{}\n\n{}", t!("abt — Android Backup Tool (TUI + CLI via adb, alternativa a MTP)

USO:
  abt                                file manager TUI
  abt devices                        elenco device collegati
  abt connect IP:5555                collega device via Wi-Fi
  abt wifi                           adbd del device -> modalità tcpip (via USB)
  abt install-adb                    scarica adb (platform-tools) per questo sistema
  abt speedtest [<file>]             misura la velocità: collegamento, memoria del telefono, disco del PC

FILE E CARTELLE (sul device):
  abt ls [-l] [<remoto>]             elenco (default /sdcard)
  abt disks                          dischi montati sul computer (destinazioni per i backup)
  abt pull <remoto>... [<locale>]    copia dal device (default ~/Android_Backup/Backup) [--verify]
  abt push <locale>... <remoto>      copia sul device (cartella creata se manca)
  abt sync <remoto> <locale>         mirror incrementale [--delete]
  abt mkdir <remoto> · mv <da> <a> · rm [-y] <remoto>...

BACKUP:
  abt backup [-o DIR] [<remoto>...]  sync di DCIM/Pictures/Documents/Download/WhatsApp + SMS/chiamate
  abt apk list | apk pull <pkg> <outdir>     backup APK
  abt sms <out.jsonl> | calllog <out.jsonl>  backup SMS / registro chiamate
  abt appdata <pkg> <outdir>                 dati privati app (root/run-as)

Opzioni: -s SERIAL · -o DIR (destinazione) · -y (niente conferma) · ADB_BIN=/percorso/adb
Cartella di default: $ABT_BACKUP_DIR oppure ~/Android_Backup/Backup
Lingua: automatica dal sistema, oppure ABT_LANG=it|en

TUI: premi ? per l'elenco completo dei comandi"), version_line());
}

/// Oltre questa soglia di caratteri arrivati "tutti insieme" si tratta di un incolla, non di digitazione.
const PASTE_BURST: usize = 16;

/// Alcuni terminali (es. VS Code su Windows) incollano gli appunti al click destro: il testo arriva come
/// una raffica di tasti che la TUI interpreterebbe come comandi (`d`, `y`, `p`, Invio…).
/// Se in un colpo solo arrivano molti caratteri li scarta, tranne quando si sta scrivendo in un campo di testo.
/// Frecce, Esc e mouse non vengono toccati.
fn drop_paste(batch: Vec<crossterm::event::Event>, text_input: bool) -> Vec<crossterm::event::Event> {
    use crossterm::event::{Event, KeyCode, KeyEventKind};
    let is_text = |e: &Event| {
        matches!(e, Event::Key(k) if k.kind == KeyEventKind::Press && matches!(k.code, KeyCode::Char(_) | KeyCode::Enter | KeyCode::Tab))
    };
    if text_input || batch.iter().filter(|e| is_text(e)).count() < PASTE_BURST {
        return batch;
    }
    batch.into_iter().filter(|e| !is_text(e)).collect()
}

fn tui() -> Result<()> {
    use crossterm::event::{self, Event};
    use crossterm::{execute, terminal};
    use ratatui::backend::CrosstermBackend;
    use ratatui::Terminal;
    use std::io::{self, Write};

    let mut stdout = io::stdout();
    terminal::enable_raw_mode()?;
    execute!(stdout, terminal::EnterAlternateScreen, crossterm::event::EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let mut app = app::App::new();
    // ABT_DEBUG_MOUSE=1: mostra nel log i click ricevuti (utile se il terminale intercetta il click destro).
    let mouse_debug = std::env::var_os("ABT_DEBUG_MOUSE").is_some();
    let res = (|| -> Result<()> {
        loop {
            terminal.draw(|f| ui::draw(f, &mut app))?;
            if event::poll(Duration::from_millis(60))? {
                // Raccoglie tutti gli eventi già pronti: serve a riconoscere un "incolla" dal terminale.
                let mut batch = vec![event::read()?];
                while batch.len() < 4096 && event::poll(Duration::ZERO)? {
                    batch.push(event::read()?);
                }
                for ev in drop_paste(batch, app.accepts_text()) {
                    match ev {
                        Event::Key(k) => app.on_key(k),
                        Event::Mouse(m) => {
                            if mouse_debug && matches!(m.kind, event::MouseEventKind::Down(_) | event::MouseEventKind::Up(_)) {
                                app.log(format!("mouse: {:?} {:?} ({}, {})", m.kind, m.modifiers, m.column, m.row));
                            }
                            app.on_mouse(m)
                        }
                        _ => {}
                    }
                }
            }
            app.poll();
            if app.quit {
                break;
            }
        }
        Ok(())
    })();

    terminal::disable_raw_mode()?;
    execute!(terminal.backend_mut(), crossterm::event::DisableMouseCapture, terminal::LeaveAlternateScreen)?;
    let _ = io::stdout().flush();
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    fn key(c: KeyCode) -> Event {
        Event::Key(KeyEvent::new(c, KeyModifiers::NONE))
    }

    fn pasted(text: &str) -> Vec<Event> {
        text.chars().map(|c| key(KeyCode::Char(c))).collect()
    }

    #[test]
    fn paste_burst_is_dropped_outside_text_input() {
        let mut batch = pasted("rm -rf /sdcard/DCIM && yes please delete everything");
        batch.push(key(KeyCode::Enter));
        assert!(drop_paste(batch, false).is_empty());
    }

    #[test]
    fn paste_burst_is_kept_in_text_input() {
        let batch = pasted("C:/Users/me/Downloads/platform-tools");
        let n = batch.len();
        assert_eq!(drop_paste(batch, true).len(), n);
    }

    #[test]
    fn normal_typing_arrows_and_mouse_are_untouched() {
        let mut batch = pasted("dy"); // pochi tasti = digitazione reale
        batch.push(key(KeyCode::Down));
        batch.push(Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Right),
            column: 3,
            row: 4,
            modifiers: KeyModifiers::NONE,
        }));
        assert_eq!(drop_paste(batch, false).len(), 4);
    }

    #[test]
    fn right_click_survives_a_paste_burst() {
        let mut batch = vec![Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Right),
            column: 3,
            row: 4,
            modifiers: KeyModifiers::NONE,
        })];
        batch.extend(pasted("some clipboard text that is long enough"));
        batch.push(key(KeyCode::Up));
        let out = drop_paste(batch, false);
        assert_eq!(out.len(), 2);
        assert!(matches!(out[0], Event::Mouse(_)));
        assert!(matches!(out[1], Event::Key(k) if k.code == KeyCode::Up));
    }
}
