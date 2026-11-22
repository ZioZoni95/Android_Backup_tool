//! Backup di alto livello: APK, SMS/chiamate (content provider) e app-data
//! (root o run-as). Usati dalla CLI e dall'azione rapida `S` della TUI.

use crate::adb::Adb;
use crate::transfer::{pull_file, MB, Sink};
use crate::util::sh_quote;
use anyhow::{bail, Context, Result};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

/// APK installati (utente): pacchetto + percorsi base/split.
#[derive(Debug, Clone)]
pub struct Apk {
    pub package: String,
    pub paths: Vec<String>,
}

pub fn apk_list(adb: &Adb) -> Result<Vec<Apk>> {
    let out = adb.shell("pm list packages -3")?;
    let mut v = Vec::new();
    for line in out.lines() {
        let Some(pkg) = line.trim().strip_prefix("package:") else {
            continue;
        };
        let pkg = pkg.trim();
        if pkg.is_empty() {
            continue;
        }
        let paths = apk_paths(adb, pkg)?;
        v.push(Apk {
            package: pkg.to_string(),
            paths,
        });
    }
    Ok(v)
}

fn apk_paths(adb: &Adb, pkg: &str) -> Result<Vec<String>> {
    let out = adb.shell(&format!("pm path {}", sh_quote(pkg)))?;
    Ok(out
        .lines()
        .filter_map(|l| l.trim().strip_prefix("package:").map(|s| s.trim().to_string()))
        .filter(|s| !s.is_empty())
        .collect())
}

/// Scarica gli APK di un pacchetto in `<out_dir>/<pkg>/`.
pub fn apk_pull(adb: &Adb, pkg: &str, out_dir: &str, flag: &AtomicBool, sink: &dyn Sink) -> Result<usize> {
    let paths = apk_paths(adb, pkg)?;
    if paths.is_empty() {
        bail!(t!("nessun APK trovato per {pkg}", pkg = pkg));
    }
    let dst_root = format!("{}/{}", out_dir.trim_end_matches('/'), pkg);
    fs::create_dir_all(&dst_root)?;
    let mut total: u64 = 0;
    for p in &paths {
        total += adb
            .shell(&format!("toybox stat -c '%s' {}", sh_quote(p)))?
            .trim()
            .parse::<u64>()
            .unwrap_or(0);
    }
    sink.set_total(total);
    for (i, p) in paths.iter().enumerate() {
        if flag.load(Ordering::Relaxed) {
            bail!(crate::transfer::Cancelled);
        }
        let name = p.rsplit('/').next().unwrap_or(p);
        let fname = if i == 0 && name == "base.apk" {
            "base.apk".to_string()
        } else {
            format!("split_{i}_{name}")
        };
        let dst = format!("{dst_root}/{fname}");
        pull_file(adb, p, &dst, false, flag, sink)?;
    }
    Ok(paths.len())
}

/// Esporta un content provider (SMS o call log) in JSONL.
pub fn export_content(
    adb: &Adb,
    uri: &str,
    cols: &[&str],
    out_path: &str,
    flag: &AtomicBool,
    sink: &dyn Sink,
) -> Result<usize> {
    let proj = cols.join(",");
    let cmd = format!("content query --user 0 --uri {uri} --projection {proj}");
    sink.msg(t!("query del content provider…"));
    let text = adb.shell(&cmd).context(t!("content query non riuscita (permessi shell negati?)"))?;

    // Ogni riga logica inizia con "Row: N "; le righe successive sono
    // continuazioni del valore precedente (es. corpo SMS con newline).
    let mut rows: Vec<serde_json::Value> = Vec::new();
    let mut cur: Option<String> = None;
    let flush = |cur: &mut Option<String>, rows: &mut Vec<serde_json::Value>| {
        if let Some(t) = cur.take() {
            let vals = parse_row(&t, cols);
            let mut obj = serde_json::Map::new();
            for (c, v) in cols.iter().zip(vals) {
                obj.insert((*c).to_string(), serde_json::Value::String(v));
            }
            rows.push(serde_json::Value::Object(obj));
            sink.set_bytes(rows.len() as u64);
        }
    };
    for line in text.lines() {
        if flag.load(Ordering::Relaxed) {
            bail!(crate::transfer::Cancelled);
        }
        let line = line.trim_end_matches('\r');
        if line.starts_with("Row: ") {
            flush(&mut cur, &mut rows);
            let rest = line.splitn(3, ' ').nth(2).unwrap_or("");
            cur = Some(rest.to_string());
        } else if let Some(c) = cur.as_mut() {
            c.push('\n');
            c.push_str(line);
        }
    }
    flush(&mut cur, &mut rows);

    let mut f = fs::File::create(out_path).with_context(|| t!("creazione di {path}", path = out_path))?;
    for r in &rows {
        let mut s = serde_json::to_string(r)?;
        s.push('\n');
        f.write_all(s.as_bytes())?;
    }
    Ok(rows.len())
}

/// Parsing di una riga `col=val, col=val…`: i campi di testo libero devono
/// stare in coda alla proiezione (body/name per ultimi), così i valori che
/// contengono ", " vengono riuniti nell'ultimo campo.
fn parse_row(text: &str, cols: &[&str]) -> Vec<String> {
    let mut vals = vec![String::new(); cols.len()];
    let mut idx = 0usize;
    for p in text.split(", ") {
        if idx < cols.len() {
            let pref = format!("{}=", cols[idx]);
            if let Some(v) = p.strip_prefix(&pref) {
                vals[idx] = v.to_string();
                idx += 1;
                continue;
            }
        }
        // leftover: fa parte del valore precedente (o del primo)
        let target = if idx > 0 { idx - 1 } else { 0 };
        if !vals[target].is_empty() {
            vals[target].push_str(", ");
        }
        vals[target].push_str(p);
    }
    vals
}

pub const SMS_COLS: &[&str] = &["_id", "date", "address", "type", "body"];
pub const CALLLOG_COLS: &[&str] = &["_id", "date", "number", "duration", "type", "new", "name"];

/// SMS -> JSONL.
pub fn export_sms(adb: &Adb, out_path: &str, flag: &AtomicBool, sink: &dyn Sink) -> Result<usize> {
    export_content(adb, "content://sms", SMS_COLS, out_path, flag, sink)
}

/// Call log -> JSONL.
pub fn export_calllog(adb: &Adb, out_path: &str, flag: &AtomicBool, sink: &dyn Sink) -> Result<usize> {
    export_content(adb, "content://call_log/calls", CALLLOG_COLS, out_path, flag, sink)
}

/// Dati privati di un'app: prova root (`su`), poi `run-as` (solo app debuggabili).
pub fn appdata_pull(adb: &Adb, pkg: &str, out_dir: &str, flag: &AtomicBool, sink: &dyn Sink) -> Result<()> {
    let dst_root = format!("{}/{}", out_dir.trim_end_matches('/'), pkg);
    fs::create_dir_all(&dst_root)?;

    // 1) root
    let cmd = format!("su -c 'toybox tar -cf - -C /data/data {}'", sh_quote(pkg));
    match extract_from_adb(adb, &cmd, &dst_root, flag, sink) {
        Ok(()) => return Ok(()),
        Err(e) => {
            // 2) run-as (app debuggabili)
            let cmd2 = format!("run-as {} toybox tar -cf - .", sh_quote(pkg));
            match extract_from_adb(adb, &cmd2, &dst_root, flag, sink) {
                Ok(()) => Ok(()),
                Err(_) => bail!(t!(
                    "app-data di {pkg} non estraibile: serve root o app debuggabile ({e})",
                    pkg = pkg,
                    e = e
                )),
            }
        }
    }
}

/// Esegue `cmd` via exec-out (deve produrre un tar) e lo estrae in `dst`.
fn extract_from_adb(adb: &Adb, cmd: &str, dst: &str, flag: &AtomicBool, sink: &dyn Sink) -> Result<()> {
    use std::io::Read;
    let mut c1 = adb.exec_out(cmd)?;
    let mut c2 = Command::new("tar")
        .arg("-xf")
        .arg("-")
        .arg("-C")
        .arg(dst)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context(t!("avvio tar locale"))?;
    let mut out1 = c1.stdout.take().context(t!("stdout adb assente"))?;
    let mut in2 = c2.stdin.take().context(t!("stdin tar locale"))?;
    let mut buf = vec![0u8; MB as usize];
    let mut n = 0usize;
    loop {
        if flag.load(Ordering::Relaxed) {
            let _ = c1.kill();
            let _ = c2.kill();
            bail!(crate::transfer::Cancelled);
        }
        match out1.read(&mut buf) {
            Ok(0) => break,
            Ok(k) => {
                in2.write_all(&buf[..k])?;
                sink.add_bytes(k as u64);
                n += 1;
                if n % 64 == 0 {
                    sink.msg(t!("estrazione in corso…"));
                }
            }
            Err(e) => {
                let _ = c1.kill();
                let _ = c2.kill();
                return Err(e.into());
            }
        }
    }
    drop(in2);
    let s1 = c1.wait()?;
    let o2 = c2.wait_with_output()?;
    if flag.load(Ordering::Relaxed) {
        bail!(crate::transfer::Cancelled);
    }
    if !s1.success() {
        bail!(t!("tar remoto fallito (exit {s1})", s1 = s1));
    }
    if !o2.status.success() {
        bail!(t!("estrazione fallita"));
    }
    let _ = Path::new(dst);
    Ok(())
}
