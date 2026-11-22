//! Piccole utility condivise (quoting, percorsi device, formattazioni).

/// Racchiude un percorso tra apici singoli per la shell del device
/// (escape POSIX di `'`).
pub fn sh_quote(p: &str) -> String {
    let mut q = String::with_capacity(p.len() + 2);
    q.push('\'');
    for c in p.chars() {
        if c == '\'' {
            q.push_str("'\\''");
        } else {
            q.push(c);
        }
    }
    q.push('\'');
    q
}

/// Join di due componenti di percorso **sul device** (sempre `/`).
pub fn dev_join(base: &str, name: &str) -> String {
    let base = base.trim_end_matches('/');
    if base.is_empty() {
        format!("/{name}")
    } else {
        format!("{base}/{name}")
    }
}

/// Directory padre di un percorso sul device. "/" -> "/".
pub fn dev_parent(p: &str) -> String {
    let t = p.trim_end_matches('/');
    match t.rfind('/') {
        Some(0) => "/".to_string(),
        Some(i) => t[..i].to_string(),
        None => "/".to_string(),
    }
}

/// Ultimo componente di un percorso sul device.
pub fn dev_basename(p: &str) -> &str {
    let t = p.trim_end_matches('/');
    match t.rfind('/') {
        Some(i) => &t[i + 1..],
        None => t,
    }
}

/// Dimensione leggibile (B/KB/MB/GB/TB).
pub fn humanize(b: u64) -> String {
    const K: f64 = 1024.0;
    let x = b as f64;
    if b < 1024 {
        format!("{b} B")
    } else if b < 1024 * 1024 {
        format!("{:.1} KB", x / K)
    } else if b < 1024_u64.pow(3) {
        format!("{:.1} MB", x / (K * K))
    } else if b < 1024_u64.pow(4) {
        format!("{:.1} GB", x / (K * K * K))
    } else {
        format!("{:.1} TB", x / (K * K * K * K))
    }
}

/// Data leggibile da un timestamp epoch (senza dipendenze esterne).
pub fn fmt_date(ts: i64) -> String {
    if ts <= 0 {
        return "-".into();
    }
    let days = ts.div_euclid(86_400);
    let secs = ts.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}",
        secs / 3600,
        (secs % 3600) / 60
    )
}

/// Algoritmo di Howard Hinnant (giorni dal 1970-01-01 -> data civile).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Timestamp corrente in secondi.
pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Logo Android (ASCII/blocchi), usato da TUI e CLI.
pub const LOGO: [&str; 11] = [
    r"      ╲    ╱      ",
    r"     ▄██████▄     ",
    r"     █ ████ █     ",
    r"     ▀▀▀▀▀▀▀▀     ",
    r"  ▄█ ████████ █▄  ",
    r"  ██ ████████ ██  ",
    r"  ██ ████████ ██  ",
    r"  ▀█ ████████ █▀  ",
    r"     ▀██▀▀██▀     ",
    r"      ██  ██      ",
    r"      ▀▀  ▀▀      ",
];

/// Cartella home dell'utente (`HOME` su Unix, `USERPROFILE` su Windows), con `/` come separatore.
pub fn home_dir() -> Option<String> {
    let v = std::env::var("HOME")
        .ok()
        .filter(|h| !h.is_empty())
        .or_else(|| std::env::var("USERPROFILE").ok().filter(|h| !h.is_empty()))?;
    Some(norm_local(&v))
}

/// Normalizza un percorso locale: su Windows usa sempre `/` e porta `C:` a `C:/`.
/// Su Unix il percorso resta invariato (`\` è un carattere valido nei nomi).
pub fn norm_local(p: &str) -> String {
    if cfg!(windows) {
        let mut s = p.replace('\\', "/");
        let b = s.as_bytes();
        if b.len() == 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
            s.push('/');
        }
        s
    } else {
        p.to_string()
    }
}

/// Directory padre locale; alla radice (`/` o `C:/`) restituisce il percorso stesso.
pub fn local_parent(p: &str) -> String {
    std::path::Path::new(p)
        .parent()
        .map(|x| norm_local(&x.to_string_lossy()))
        .filter(|x| !x.is_empty())
        .unwrap_or_else(|| p.to_string())
}

/// true se `p` è la radice di un filesystem locale (`/` o `C:/`).
pub fn is_local_root(p: &str) -> bool {
    let t = p.trim();
    if t == "/" {
        return true;
    }
    let b = t.as_bytes();
    cfg!(windows) && b.len() >= 2 && b.len() <= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b.len() == 2 || b[2] == b'/')
}

/// Cartella locale di default per i backup: `$ABT_BACKUP_DIR` oppure
/// `~/Android_Backup/Backup`. La crea se manca.
pub fn default_backup_dir() -> Result<String, String> {
    let d = std::env::var("ABT_BACKUP_DIR")
        .ok()
        .filter(|v| !v.is_empty())
        .map(|v| norm_local(&v))
        .or_else(|| home_dir().map(|h| format!("{}/Android_Backup/Backup", h.trim_end_matches('/'))))
        .ok_or_else(|| t!("HOME/USERPROFILE non impostata").to_string())?;
    std::fs::create_dir_all(&d).map_err(|e| format!("{d}: {e}"))?;
    Ok(d)
}

/// Numero con separatore delle migliaia (es. 1.234.567).
pub fn fmt_int(n: u64) -> String {
    let d = n.to_string();
    let mut out = String::new();
    for (i, c) in d.chars().enumerate() {
        if i > 0 && (d.len() - i) % 3 == 0 {
            out.push('.');
        }
        out.push(c);
    }
    out
}

/// Somma ricorsiva locale: (byte, file, cartelle).
pub fn local_totals(path: &std::path::Path) -> (u64, u64, u64) {
    let Ok(md) = std::fs::symlink_metadata(path) else { return (0, 0, 0) };
    if md.is_file() {
        return (md.len(), 1, 0);
    }
    if !md.is_dir() {
        return (0, 0, 0);
    }
    let (mut b, mut f, mut d) = (0, 0, 0);
    if let Ok(rd) = std::fs::read_dir(path) {
        for e in rd.flatten() {
            let (b2, f2, d2) = local_totals(&e.path());
            b += b2;
            f += f2;
            d += d2;
        }
    }
    (b, f, d + 1)
}
