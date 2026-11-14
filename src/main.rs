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
        eprintln!("errore: {e:#}");
        std::process::exit(1);
    }
}

fn adb_bin() -> String {
    std::env::var("ADB_BIN").unwrap_or_else(|_| "adb".into())
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
        0 => bail!(
            "nessun device adb in stato 'device' — collega via USB con debug USB attivo, \
             usa 'abt devices', oppure 'abt connect IP:5555'"
        ),
        1 => {
            adb.serial = Some(ok[0].serial.clone());
            Ok(())
        }
        n => bail!(
            "{n} device collegati: specifica -s SERIAL ({})",
            ok.iter()
                .map(|d| d.serial.clone())
                .collect::<Vec<_>>()
                .join(", ")
        ),
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
                .ok_or_else(|| anyhow::anyhow!("uso: abt connect IP:5555"))?;
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
            println!("ora collega via Wi-Fi:  abt connect <IP-DEL-TELEFONO>:5555");
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
            println!("{} elementi → {dst}", srcs.len());
            Ok(())
        }
        "push" => {
            // abt push <locale>... <cartella-remota>
            if rest.len() < 2 {
                bail!("uso: abt push <locale>... <cartella-remota>");
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
            println!("{} elementi → {dst}", srcs.len());
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
        "mkdir" => {
            let p = rest.first().ok_or_else(|| anyhow::anyhow!("uso: abt mkdir <remoto>"))?;
            let mut a = Adb::new(adb_bin());
            resolve_serial(&mut a, serial)?;
            device_fs::mkdir_device(&a, p)?;
            println!("creata {p}");
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
                bail!("uso: abt rm [-y] <remoto>...");
            }
            let mut a = Adb::new(adb_bin());
            resolve_serial(&mut a, serial)?;
            if !opts.yes {
                eprint!("Eliminare DAL DEVICE {} (irreversibile)? [s/N] ", rest.join(", "));
                let mut l = String::new();
                std::io::stdin().read_line(&mut l)?;
                if !matches!(l.trim().to_lowercase().as_str(), "s" | "si" | "sì" | "y" | "yes") {
                    bail!("annullato");
                }
            }
            for p in &rest {
                device_fs::rm_device(&a, p)?;
                println!("eliminato {p}");
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
                    eprintln!("salto {src}: non esiste");
                    continue;
                }
                let name = src.rsplit('/').next().unwrap_or(src);
                let dst = format!("{}/{name}", out.trim_end_matches('/'));
                let sink = CliSink::new(format!("backup {src}"));
                let st = transfer::sync_mirror(&a, src, &dst, delete, &flag, &sink)?;
                sink.print(true);
                eprintln!();
                println!("{src}: {} file nuovi/aggiornati, {}", st.files, humanize(st.bytes));
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
                        Ok(n) => println!("{c}: {n} record → {path}"),
                        Err(e) => eprintln!("{c}: {e}"),
                    }
                }
            }
            println!("backup in {out}");
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
                "sync completato: {} file, {} rimossi, {}",
                st.files,
                st.deleted,
                humanize(st.bytes)
            );
            Ok(())
        }
        "apk" => match rest.first().map(|s| s.as_str()) {
            Some("list") => {
                let mut a = Adb::new(adb_bin());
                resolve_serial(&mut a, serial)?;
                for apk in actions::apk_list(&a)? {
                    println!("{}  ({} file)", apk.package, apk.paths.len());
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
                println!("apk {pkg}: {n} file → {out}/{pkg}");
                Ok(())
            }
            _ => bail!("uso: abt apk list | abt apk pull <pkg> <outdir>"),
        },
        "sms" | "calllog" => {
            let out = rest
                .first()
                .ok_or_else(|| anyhow::anyhow!("uso: abt {cmd} <out.jsonl>"))?;
            let mut a = Adb::new(adb_bin());
            resolve_serial(&mut a, serial)?;
            let sink = CliSink::new(cmd.clone());
            let flag = new_flag();
            let n = if cmd == "sms" {
                actions::export_sms(&a, out, &flag, &sink)?
            } else {
                actions::export_calllog(&a, out, &flag, &sink)?
            };
            println!("{cmd}: {n} record → {out}");
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
        other => bail!("comando sconosciuto: {other} (usa 'abt help')"),
    }
}

/// Separa sorgenti e destinazione: ultimo argomento = destinazione se ≥ 2 argomenti,
/// altrimenti `-o` o la cartella di default.
fn split_dest(rest: &[String], out: Option<String>, local: bool) -> Result<(Vec<String>, String)> {
    if rest.is_empty() {
        bail!("uso: abt pull <remoto>... [<locale>]");
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
        bail!("servono due argomenti <sorgente> <destinazione>");
    }
    Ok((rest[0].clone(), rest[1].clone()))
}

fn print_help() {
    for l in util::LOGO {
        println!("\x1b[32m{l}\x1b[0m");
    }
    println!(
        "abt — Android Backup Tool (TUI + CLI via adb, alternativa a MTP)\n\
\n\
USO:\n\
  abt                                file manager TUI\n\
  abt devices                        elenco device collegati\n\
  abt connect IP:5555                collega device via Wi-Fi\n\
  abt wifi                           adbd del device -> modalità tcpip (via USB)\n\
\n\
FILE E CARTELLE (sul device):\n\
  abt ls [-l] [<remoto>]             elenco (default /sdcard)\n\
  abt pull <remoto>... [<locale>]    copia dal device (default ~/Android_Backup/Backup) [--verify]\n\
  abt push <locale>... <remoto>      copia sul device (cartella creata se manca)\n\
  abt sync <remoto> <locale>         mirror incrementale [--delete]\n\
  abt mkdir <remoto> · mv <da> <a> · rm [-y] <remoto>...\n\
\n\
BACKUP:\n\
  abt backup [-o DIR] [<remoto>...]  sync di DCIM/Pictures/Documents/Download/WhatsApp + SMS/chiamate\n\
  abt apk list | apk pull <pkg> <outdir>     backup APK\n\
  abt sms <out.jsonl> | calllog <out.jsonl>  backup SMS / registro chiamate\n\
  abt appdata <pkg> <outdir>                 dati privati app (root/run-as)\n\
\n\
Opzioni: -s SERIAL · -o DIR (destinazione) · -y (niente conferma) · ADB_BIN=/percorso/adb\n\
Cartella di default: $ABT_BACKUP_DIR oppure ~/Android_Backup/Backup\n\
\n\
TUI: premi ? per l'elenco completo dei comandi"
    );
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
    let res = (|| -> Result<()> {
        loop {
            terminal.draw(|f| ui::draw(f, &mut app))?;
            if event::poll(Duration::from_millis(60))? {
                match event::read()? {
                    Event::Key(k) => app.on_key(k),
                    Event::Mouse(m) => app.on_mouse(m),
                    _ => {}
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
