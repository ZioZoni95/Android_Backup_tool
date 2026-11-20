//! Operazioni di filesystem sul device (toybox) e in locale.

use crate::adb::Adb;
use crate::util::{dev_parent, sh_quote};
use anyhow::{bail, Result};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

/// Voce di directory (device o locale).
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Entry {
    pub name: String,
    pub full: String,
    pub is_dir: bool,
    pub size: u64,
    pub mtime: i64,
    pub remote: bool,
}

impl Entry {
    fn dot(parent: String, remote: bool) -> Self {
        Self {
            name: "..".into(),
            full: parent,
            is_dir: true,
            size: 0,
            mtime: 0,
            remote,
        }
    }
}

/// Percorsi pericolosi da non toccare mai (rm/delete/mkdir).
pub fn safe_path(p: &str) -> Result<()> {
    let t = p.trim();
    if t.is_empty() || crate::util::is_local_root(t) || t.split('/').any(|c| c == "..") {
        bail!("percorso non consentito: {t:?}");
    }
    Ok(())
}

/// true se sul device il percorso è una directory.
pub fn is_dir(adb: &Adb, p: &str) -> Result<bool> {
    let out = adb.shell(&format!("toybox test -d {} && echo __Y__", sh_quote(p)))?;
    Ok(out.contains("__Y__"))
}

fn sort_entries(v: &mut [Entry]) {
    v.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });
}

fn nul_split(bytes: &[u8]) -> Vec<String> {
    bytes
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect()
}

fn with_dot(mut v: Vec<Entry>, dir: &str, remote: bool) -> Vec<Entry> {
    let d = dir.trim_end_matches('/');
    if remote {
        if !d.is_empty() {
            v.insert(0, Entry::dot(dev_parent(d), remote));
        }
    } else if !crate::util::is_local_root(dir) && !d.is_empty() {
        v.insert(0, Entry::dot(crate::util::local_parent(dir), remote));
    }
    v
}

/// Elenco di una directory sul device.
pub fn list(adb: &Adb, dir: &str) -> Result<Vec<Entry>> {
    let dir = dir.trim_end_matches('/');
    let dir = if dir.is_empty() { "/" } else { dir };
    if !is_dir(adb, dir)? {
        bail!("directory non esistente o non accessibile: {dir}");
    }

    let all = nul_split(&adb.exec_out_collect(&format!(
        "toybox find -H {} -maxdepth 1 -print0",
        sh_quote(dir)
    ))?);
    let dirs: HashSet<String> = nul_split(&adb.exec_out_collect(&format!(
        "toybox find -L {} -maxdepth 1 -type d -print0",
        sh_quote(dir)
    ))?)
    .into_iter()
    .collect();
    let stat_raw = adb.exec_out_collect(&format!(
        "toybox find -H {} -maxdepth 1 -print0 | toybox xargs -0 -n 32 toybox stat -c '%s|%Y|%n'",
        sh_quote(dir)
    ))?;
    let mut detail: HashMap<String, (u64, i64)> = HashMap::new();
    for line in String::from_utf8_lossy(&stat_raw).lines() {
        let line = line.trim_end_matches('\r');
        let mut it = line.splitn(3, '|');
        let (Some(s), Some(m), Some(p)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        let size = s.trim().parse().unwrap_or(0);
        let mtime = m.trim().parse().unwrap_or(0);
        detail.insert(p.trim().to_string(), (size, mtime));
    }

    let mut v = Vec::with_capacity(all.len());
    for full in all {
        if full == dir {
            continue;
        }
        let name = full.rsplit('/').next().unwrap_or(full.as_str()).to_string();
        if name.is_empty() {
            continue;
        }
        let is_dir = dirs.contains(&full);
        let (size, mtime) = detail.get(&full).copied().unwrap_or((0, 0));
        v.push(Entry {
            name,
            full,
            is_dir,
            size,
            mtime,
            remote: true,
        });
    }
    sort_entries(&mut v);
    Ok(with_dot(v, dir, true))
}

/// Elenco di una directory locale.
pub fn list_local(dir: &str) -> Result<Vec<Entry>> {
    let mut v = Vec::new();
    let rd = fs::read_dir(dir).map_err(|e| anyhow::anyhow!("lettura di {dir} fallita: {e}"))?;
    for e in rd {
        let Ok(e) = e else { continue };
        let full = crate::util::norm_local(&e.path().to_string_lossy());
        let name = e.file_name().to_string_lossy().into_owned();
        let md = fs::metadata(&full).or_else(|_| e.metadata()).ok();
        let (is_dir, size, mtime) = match md {
            Some(m) => (m.is_dir(), m.len(), mtime_secs(&m)),
            None => (false, 0, 0),
        };
        v.push(Entry {
            name,
            full,
            is_dir,
            size,
            mtime,
            remote: false,
        });
    }
    sort_entries(&mut v);
    Ok(with_dot(v, dir, false))
}

fn mtime_secs(m: &fs::Metadata) -> i64 {
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// mkdir -p sul device.
pub fn mkdir_device(adb: &Adb, p: &str) -> Result<()> {
    adb.shell(&format!("toybox mkdir -p {}", sh_quote(p)))?;
    Ok(())
}

/// rm -rf sul device (con guardia anti-percorso-pericoloso).
pub fn rm_device(adb: &Adb, p: &str) -> Result<()> {
    safe_path(p)?;
    adb.shell(&format!("toybox rm -r -f {}", sh_quote(p)))?;
    Ok(())
}

/// mkdir -p locale.
pub fn mkdir_local(p: &str) -> Result<()> {
    fs::create_dir_all(p)?;
    Ok(())
}

/// Cancellazione locale (file o directory) con guardia.
pub fn delete_local(p: &str) -> Result<()> {
    safe_path(p)?;
    let path = Path::new(p);
    let md = fs::metadata(path)?;
    if md.is_dir() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}

/// Rinomina/sposta sul device (rifiuta la sovrascrittura).
pub fn rename_device(adb: &Adb, old: &str, new: &str) -> Result<()> {
    safe_path(old)?;
    safe_path(new)?;
    let out = adb.shell(&format!(
        "[ -e {n} ] && echo __EXISTS__ || toybox mv {o} {n}",
        o = sh_quote(old),
        n = sh_quote(new)
    ))?;
    if out.contains("__EXISTS__") {
        bail!("esiste già un elemento con quel nome");
    }
    Ok(())
}

/// Rinomina locale (rifiuta la sovrascrittura).
pub fn rename_local(old: &str, new: &str) -> Result<()> {
    safe_path(old)?;
    safe_path(new)?;
    if Path::new(new).exists() {
        bail!("esiste già un elemento con quel nome");
    }
    fs::rename(old, new)?;
    Ok(())
}
