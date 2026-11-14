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

/// Estrae `-s SERIAL` / `--serial SERIAL` e `--verify` / `--delete`.
fn parse_args(mut args: Vec<String>) -> (Option<String>, bool, bool, Vec<String>) {
    let mut serial = None;
    let mut verify = false;
    let mut delete = false;
    let mut rest = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-s" | "--serial" if i + 1 < args.len() => {
                serial = Some(args[i + 1].clone());
                i += 2;
                continue;
            }
            "--verify" => {
                verify = true;
                i += 1;
                continue;
            }
            "--delete" => {
                delete = true;
                i += 1;
                continue;
            }
            _ => {}
        }
        rest.push(args.remove(i));
    }
    (serial, verify, delete, rest)
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
    let (serial, verify, delete, rest) = parse_args(argv);
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
            let (src, dst) = two_args(&rest)?;
            let mut a = Adb::new(adb_bin());
            resolve_serial(&mut a, serial)?;
            let sink = CliSink::new(format!("pull {src}"));
            let flag = new_flag();
            if device_fs::is_dir(&a, &src)? {
                transfer::pull_dir(&a, &src, &dst, &flag, &sink)?;
            } else {
                let dst = if std::path::Path::new(&dst).is_dir() {
                    let name = src.rsplit('/').next().unwrap_or(&src);
                    format!("{}/{}", dst.trim_end_matches('/'), name)
                } else {
                    dst.clone()
                };
                transfer::pull_file(&a, &src, &dst, verify, &flag, &sink)?;
            }
            sink.print(true);
            eprintln!();
            Ok(())
        }
        "push" => {
            let (src, dst) = two_args(&rest)?;
            let mut a = Adb::new(adb_bin());
            resolve_serial(&mut a, serial)?;
            let sink = CliSink::new(format!("push {src}"));
            let flag = new_flag();
            let md = std::fs::metadata(&src)?;
            if md.is_dir() {
                transfer::push_dir(&a, &src, &dst, &flag, &sink)?;
            } else {
                transfer::push_file(&a, &src, &dst, &flag, &sink)?;
            }
            sink.print(true);
            eprintln!();
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

fn two_args(rest: &[String]) -> Result<(String, String)> {
    if rest.len() < 2 {
        bail!("servono due argomenti <sorgente> <destinazione>");
    }
    Ok((rest[0].clone(), rest[1].clone()))
}

fn print_help() {
    println!(
        "abt — Android Backup Tool (TUI + CLI via adb, alternativa a MTP)\n\
\n\
USO:\n\
  abt                                file manager TUI\n\
  abt devices                        elenco device collegati\n\
  abt connect IP:5555                collega device via Wi-Fi\n\
  abt wifi                           adbd del device -> modalità tcpip (via USB)\n\
  abt pull  <remoto> <locale> [--verify]     copia dal device\n\
  abt push  <locale> <remoto>                copia sul device\n\
  abt sync  <remoto> <locale> [--delete]     mirror incrementale\n\
  abt apk  list | apk pull <pkg> <outdir>    backup APK\n\
  abt sms <out.jsonl> | calllog <out.jsonl>  backup SMS / registro chiamate\n\
  abt appdata <pkg> <outdir>                 dati privati app (root/run-as)\n\
\n\
Opzioni: -s SERIAL | --serial SERIAL   ·   ADB_BIN=/percorso/adb per binario custom\n\
\n\
TUI: Tab pannello · j/k o ↑↓ naviga · Invio apri (dir) / trasferisci (file) ·\n\
     Spazio marca · a tutto · y pull · p push · s sync · d elimina · n mkdir ·\n\
     r aggiorna · S SMS+chiamate · x annulla job · q esci"
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
    execute!(stdout, terminal::EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let mut app = app::App::new();
    let res = (|| -> Result<()> {
        loop {
            terminal.draw(|f| ui::draw(f, &mut app))?;
            if event::poll(Duration::from_millis(60))? {
                if let Event::Key(k) = event::read()? {
                    app.on_key(k);
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
    execute!(terminal.backend_mut(), terminal::LeaveAlternateScreen)?;
    let _ = io::stdout().flush();
    res
}
