//! Posizioni rapide: luoghi predefiniti, preferiti, recenti e path cliccabile.

use crate::adb::Adb;
use crate::app::Side;
use std::path::Path;

/// Voce di "Vai a…": etichetta e percorso (percorso vuoto = digitazione manuale).
pub type Place = (String, String);

/// Segmenti cliccabili di un percorso: (inizio, fine, percorso di destinazione),
/// con offset in caratteri relativi all'inizio del percorso.
pub fn crumbs(cwd: &str) -> Vec<(usize, usize, String)> {
    let mut v = Vec::new();
    let mut off = 0usize;
    let mut acc = String::new();
    for (i, part) in cwd.split('/').enumerate() {
        let len = part.chars().count();
        if i == 0 {
            if part.is_empty() {
                v.push((0, 1, "/".to_string()));
            } else if part.len() == 2 && part.ends_with(':') {
                // unità Windows (C:)
                v.push((0, len, format!("{part}/")));
                acc.push_str(part);
            }
        } else if !part.is_empty() {
            acc.push('/');
            acc.push_str(part);
            v.push((off, off + len, acc.clone()));
        }
        off += len + 1;
    }
    v
}

fn home() -> Option<String> {
    crate::util::home_dir()
}

/// Luoghi predefiniti del computer (solo quelli esistenti).
pub fn local_places() -> Vec<Place> {
    let mut v: Vec<Place> = Vec::new();
    let Some(h) = home() else {
        return vec![(t!("Radice").into(), if cfg!(windows) { "C:/" } else { "/" }.into())];
    };
    v.push(("Home".into(), h.clone()));
    if let Ok(b) = crate::util::default_backup_dir() {
        v.push(("Backup".into(), b));
    }
    for (label, names) in [
        (t!("Scrivania"), &["Desktop", "Scrivania"][..]),
        (t!("Documenti"), &["Documents", "Documenti"][..]),
        ("Download", &["Downloads", "Download", "Scaricati"][..]),
        (t!("Immagini"), &["Pictures", "Immagini"][..]),
        ("Video", &["Videos", "Video", "Movies"][..]),
        (t!("Musica"), &["Music", "Musica"][..]),
    ] {
        if let Some(n) = names.iter().find(|n| Path::new(&format!("{h}/{n}")).is_dir()) {
            v.push((label.into(), format!("{h}/{n}")));
        }
    }
    for d in mounted_disks() {
        let space = if d.total > 0 {
            t!(" · {avail} liberi di {total}", avail = crate::util::humanize(d.avail), total = crate::util::humanize(d.total))
        } else {
            String::new()
        };
        v.push((t!("⛁ Disco «{name}»{space}", name = d.name, space = space), d.mount));
    }
    if !cfg!(windows) {
        v.push((t!("Radice (/)").into(), "/".into()));
    }
    v
}

/// Disco montato: punto di mount, nome leggibile, totale e libero (byte).
pub struct Disk {
    pub mount: String,
    pub name: String,
    pub total: u64,
    pub avail: u64,
}

#[cfg(all(unix, not(target_os = "macos")))]
fn unescape_mount(s: &str) -> String {
    // /proc/mounts codifica spazi e altri caratteri come \040 (ottale)
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len() && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c)) {
            out.push((b[i + 1] - b'0') * 64 + (b[i + 2] - b'0') * 8 + (b[i + 3] - b'0'));
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Interpreta l'output di `df -k -P` (blocchi da 1 KiB): mount point → (totale, libero) in byte.
/// Il mount point può contenere spazi, quindi si riuniscono i campi dal sesto in poi.
#[cfg(any(test, target_os = "macos"))]
fn parse_df_kib(out: &str) -> std::collections::HashMap<String, (u64, u64)> {
    let mut m = std::collections::HashMap::new();
    for line in out.lines().skip(1) {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() >= 6 {
            if let (Ok(total), Ok(avail)) = (f[1].parse::<u64>(), f[3].parse::<u64>()) {
                m.insert(f[5..].join(" "), (total * 1024, avail * 1024));
            }
        }
    }
    m
}

#[cfg(target_os = "macos")]
/// Volumi montati in `/Volumes` (chiavette, dischi esterni, condivisioni di rete) con spazio totale/libero.
pub fn mounted_disks() -> Vec<Disk> {
    let space = std::process::Command::new("df")
        .args(["-k", "-P"])
        .output()
        .map(|o| parse_df_kib(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default();
    let mut v: Vec<Disk> = Vec::new();
    for (mp, (total, avail)) in &space {
        let Some(name) = mp.strip_prefix("/Volumes/") else { continue };
        if name.is_empty() || !Path::new(mp).is_dir() {
            continue;
        }
        v.push(Disk { mount: mp.clone(), name: name.to_string(), total: *total, avail: *avail });
    }
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

#[cfg(all(unix, not(target_os = "macos")))]
/// Dischi/partizioni/condivisioni attualmente montati (esclusi filesystem di sistema).
pub fn mounted_disks() -> Vec<Disk> {
    let Ok(txt) = std::fs::read_to_string("/proc/self/mounts") else { return Vec::new() };
    // etichette dei volumi: /dev/disk/by-label/<label> -> /dev/sdXN
    let mut labels: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    if let Ok(rd) = std::fs::read_dir("/dev/disk/by-label") {
        for e in rd.flatten() {
            if let Ok(t) = std::fs::canonicalize(e.path()) {
                let raw = e.file_name().to_string_lossy().replace("\\x20", " ");
                labels.insert(t.to_string_lossy().into_owned(), raw);
            }
        }
    }
    // spazio totale/libero per tutti i mount con una sola chiamata a df
    let mut space: std::collections::HashMap<String, (u64, u64)> = std::collections::HashMap::new();
    if let Ok(out) = std::process::Command::new("df").args(["-P", "-B1"]).output() {
        for line in String::from_utf8_lossy(&out.stdout).lines().skip(1) {
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.len() >= 6 {
                space.insert(f[5..].join(" "), (f[1].parse().unwrap_or(0), f[3].parse().unwrap_or(0)));
            }
        }
    }
    const SKIP_PREFIX: [&str; 9] = ["/boot", "/snap", "/var", "/etc", "/usr", "/proc", "/sys", "/dev", "/run/user"];
    const NET: [&str; 6] = ["nfs", "nfs4", "cifs", "smb3", "fuse.sshfs", "davfs"];
    let mut v: Vec<Disk> = Vec::new();
    for line in txt.lines() {
        let f: Vec<&str> = line.split(' ').collect();
        if f.len() < 3 {
            continue;
        }
        let (dev, mp, fstype) = (f[0], unescape_mount(f[1]), f[2]);
        let block = dev.starts_with("/dev/") && !matches!(fstype, "squashfs" | "iso9660" | "udf");
        let wanted = block || NET.contains(&fstype) || matches!(fstype, "fuse.exfat" | "fuseblk" | "ntfs3" | "fuse.ntfs-3g");
        if !wanted || mp == "/" || SKIP_PREFIX.iter().any(|p| mp == *p || mp.starts_with(&format!("{p}/"))) {
            continue;
        }
        if !Path::new(&mp).is_dir() || v.iter().any(|d| d.mount == mp) {
            continue;
        }
        let real = std::fs::canonicalize(dev).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| dev.to_string());
        let name = labels
            .get(&real)
            .cloned()
            .unwrap_or_else(|| mp.rsplit('/').next().filter(|n| !n.is_empty()).unwrap_or(&mp).to_string());
        let (total, avail) = space.get(&mp).copied().unwrap_or((0, 0));
        v.push(Disk { mount: mp, name, total, avail });
    }
    v
}

#[cfg(windows)]
/// Unità Windows presenti (A:–Z:) con etichetta e spazio.
pub fn mounted_disks() -> Vec<Disk> {
    use windows_sys::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetLogicalDrives, GetVolumeInformationW};
    let mask = unsafe { GetLogicalDrives() };
    let mut v = Vec::new();
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = (b'A' + i as u8) as char;
        let root: Vec<u16> = format!("{letter}:\\").encode_utf16().chain(std::iter::once(0)).collect();
        let mut label = [0u16; 261];
        let ok = unsafe {
            GetVolumeInformationW(root.as_ptr(), label.as_mut_ptr(), label.len() as u32, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), 0)
        };
        if ok == 0 {
            continue; // unità non pronta (lettore vuoto, ecc.)
        }
        let n = label.iter().position(|&c| c == 0).unwrap_or(0);
        let lbl = String::from_utf16_lossy(&label[..n]);
        let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
        unsafe { GetDiskFreeSpaceExW(root.as_ptr(), &mut avail, &mut total, &mut free) };
        let name = if lbl.is_empty() { format!("{letter}:") } else { format!("{lbl} ({letter}:)") };
        v.push(Disk { mount: format!("{letter}:/"), name, total, avail });
    }
    v
}

/// Luoghi predefiniti del device (verifica l'esistenza con una sola chiamata adb).
pub fn remote_places(adb: &Adb) -> Vec<Place> {
    let cands = [
        (t!("Memoria interna"), "/sdcard"),
        (t!("Fotocamera"), "/sdcard/DCIM"),
        (t!("Immagini"), "/sdcard/Pictures"),
        ("Download", "/sdcard/Download"),
        (t!("Documenti"), "/sdcard/Documents"),
        (t!("Musica"), "/sdcard/Music"),
        ("Video", "/sdcard/Movies"),
        ("WhatsApp", "/sdcard/Android/media/com.whatsapp/WhatsApp"),
        (t!("WhatsApp (vecchio)"), "/sdcard/WhatsApp"),
        (t!("Registrazioni"), "/sdcard/Recordings"),
    ];
    let mut script = String::new();
    for (_, p) in &cands {
        script.push_str(&format!("[ -d '{p}' ] && echo '{p}'; "));
    }
    script.push_str("for d in /storage/*-*; do [ -d \"$d\" ] && echo \"$d\"; done");
    let out = adb.shell(&script).unwrap_or_default();
    let found: Vec<&str> = out.lines().map(|l| l.trim()).collect();
    let mut v: Vec<Place> = vec![(t!("Memoria interna").into(), "/sdcard".into())];
    for (l, p) in cands.iter().skip(1) {
        if found.contains(p) {
            v.push((l.to_string(), p.to_string()));
        }
    }
    for f in found.iter().filter(|f| f.starts_with("/storage/")) {
        v.push((t!("Scheda SD ({id})", id = f.rsplit('/').next().unwrap_or("")), f.to_string()));
    }
    v.push((t!("Radice (/)").into(), "/".into()));
    v
}

/// Preferiti e recenti persistenti (`~/.config/abt/places.txt`).
#[derive(Default)]
pub struct Saved {
    pub favs: Vec<(Side, String)>,
    pub recents: Vec<(Side, String)>,
}

/// Cartella di configurazione di abt (`.../abt`).
pub fn cfg_dir() -> Option<String> {
    let base = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| std::env::var("APPDATA").ok().filter(|v| !v.is_empty()).map(|v| v.replace('\\', "/")))
        .or_else(|| home().map(|h| format!("{h}/.config")))?;
    Some(format!("{base}/abt"))
}

fn cfg_path() -> Option<String> {
    Some(format!("{}/places.txt", cfg_dir()?))
}

fn side_ch(s: Side) -> &'static str {
    if s == Side::Local { "L" } else { "R" }
}

impl Saved {
    pub fn load() -> Self {
        let mut s = Saved::default();
        let Some(p) = cfg_path() else { return s };
        let Ok(txt) = std::fs::read_to_string(p) else { return s };
        for line in txt.lines() {
            let mut it = line.splitn(3, '\t');
            let (Some(kind), Some(side), Some(path)) = (it.next(), it.next(), it.next()) else { continue };
            let side = if side == "L" { Side::Local } else { Side::Remote };
            match kind {
                "fav" => s.favs.push((side, path.to_string())),
                "rec" => s.recents.push((side, path.to_string())),
                _ => {}
            }
        }
        s
    }

    pub fn save(&self) {
        let Some(p) = cfg_path() else { return };
        if let Some(d) = Path::new(&p).parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let mut out = String::new();
        for (s, p) in &self.favs {
            out.push_str(&format!("fav\t{}\t{p}\n", side_ch(*s)));
        }
        for (s, p) in &self.recents {
            out.push_str(&format!("rec\t{}\t{p}\n", side_ch(*s)));
        }
        let _ = std::fs::write(p, out);
    }

    pub fn is_fav(&self, side: Side, path: &str) -> bool {
        self.favs.iter().any(|(s, p)| *s == side && p == path)
    }

    /// Aggiunge/rimuove un preferito; true se ora è preferito.
    pub fn toggle_fav(&mut self, side: Side, path: &str) -> bool {
        let now = if self.is_fav(side, path) {
            self.favs.retain(|(s, p)| !(*s == side && p == path));
            false
        } else {
            self.favs.push((side, path.to_string()));
            true
        };
        self.save();
        now
    }

    /// Registra una cartella visitata (in testa, senza duplicati, max 8 per lato).
    pub fn visit(&mut self, side: Side, path: &str) {
        if self.recents.iter().find(|(s, _)| *s == side).map(|(_, p)| p.as_str()) == Some(path) {
            return;
        }
        self.recents.retain(|(s, p)| !(*s == side && p == path));
        self.recents.insert(0, (side, path.to_string()));
        let mut n = 0;
        self.recents.retain(|(s, _)| {
            if *s == side {
                n += 1;
                n <= 8
            } else {
                true
            }
        });
        self.save();
    }
}

#[cfg(test)]
mod tests {
    use super::parse_df_kib;

    #[test]
    fn df_output_with_spaces_in_mount_point() {
        let out = "Filesystem 1024-blocks Used Available Capacity Mounted on\n\
/dev/disk3s1s1 971350180 11000000 500000000 3% /\n\
/dev/disk4s1 15000000 5000000 10000000 34% /Volumes/MY USB\n";
        let m = parse_df_kib(out);
        assert_eq!(m["/Volumes/MY USB"], (15_000_000 * 1024, 10_000_000 * 1024));
        assert_eq!(m["/"].0, 971_350_180 * 1024);
    }
}
