//! Wrapper minimale attorno al binario `adb`.

use anyhow::{Context, Result};
use std::process::{Child, Command, Stdio};

/// Device visto da `adb devices -l`.
#[derive(Debug, Clone)]
pub struct Device {
    pub serial: String,
    pub state: String,
    pub model: Option<String>,
}

/// Proprietà del device corrente.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub model: String,
    pub android: String,
    pub abi: String,
}

/// True se `bin` è un adb eseguibile (`adb version` termina con successo).
pub fn works(bin: &str) -> bool {
    Command::new(bin)
        .arg("version")
        .stdin(Stdio::null())
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn saved_file() -> Option<String> {
    Some(format!("{}/adb_path.txt", crate::places::cfg_dir()?))
}

/// Percorso di adb salvato in precedenza dall'utente.
pub fn saved_path() -> Option<String> {
    let t = std::fs::read_to_string(saved_file()?).ok()?;
    let t = t.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// Salva il percorso di adb scelto dall'utente.
pub fn save_path(bin: &str) {
    let Some(f) = saved_file() else { return };
    if let Some(d) = std::path::Path::new(&f).parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(f, bin);
}

/// Se `p` è un file adb, o una cartella che lo contiene, restituisce il percorso dell'eseguibile.
pub fn resolve_user_path(p: &str) -> String {
    let p = p.trim().trim_matches('"');
    let exe = if cfg!(windows) { "adb.exe" } else { "adb" };
    let path = std::path::Path::new(p);
    if path.is_dir() {
        path.join(exe).to_string_lossy().into_owned()
    } else {
        p.to_string()
    }
}

/// Posizioni tipiche in cui si trova adb (platform-tools scaricato a mano, Android SDK, ...).
pub fn candidates() -> Vec<String> {
    let exe = if cfg!(windows) { "adb.exe" } else { "adb" };
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let mut dirs: Vec<std::path::PathBuf> = Vec::new();
    let home = crate::util::home_dir();

    if let Some(h) = &home {
        // Cartelle utente: adb direttamente o in una sottocartella tipo "ADB TOOLS", "platform-tools".
        for sub in ["Desktop", "Downloads", "Documents", ""] {
            let base = std::path::PathBuf::from(if sub.is_empty() { h.clone() } else { format!("{h}/{sub}") });
            dirs.push(base.clone());
            if let Ok(rd) = std::fs::read_dir(&base) {
                for e in rd.flatten() {
                    let n = e.file_name().to_string_lossy().to_lowercase();
                    if e.path().is_dir() && (n.contains("adb") || n.contains("platform") || n.contains("android")) {
                        dirs.push(e.path());
                        dirs.push(e.path().join("platform-tools"));
                    }
                }
            }
        }
        dirs.push(format!("{h}/Android/Sdk/platform-tools").into());
        dirs.push(format!("{h}/Library/Android/sdk/platform-tools").into());
        dirs.push(format!("{h}/scoop/apps/adb/current/platform-tools").into());
        dirs.push(format!("{h}/scoop/shims").into());
    }
    if cfg!(windows) {
        if let Some(l) = env("LOCALAPPDATA") {
            dirs.push(format!("{l}/Android/Sdk/platform-tools").into());
        }
        for v in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(p) = env(v) {
                dirs.push(format!("{p}/Android/platform-tools").into());
                dirs.push(format!("{p}/platform-tools").into());
            }
        }
        for d in ["C:/platform-tools", "C:/adb", "C:/ADB", "C:/Android/platform-tools", "C:/ProgramData/chocolatey/bin"] {
            dirs.push(d.into());
        }
    }
    for v in ["ANDROID_HOME", "ANDROID_SDK_ROOT"] {
        if let Some(s) = env(v) {
            dirs.push(format!("{s}/platform-tools").into());
        }
    }

    let mut out: Vec<String> = Vec::new();
    for d in dirs {
        let f = d.join(exe);
        if f.is_file() {
            let s = f.to_string_lossy().into_owned();
            if !out.contains(&s) {
                out.push(s);
            }
        }
    }
    out
}

/// Trova adb: `ADB_BIN` → percorso salvato → PATH → posizioni tipiche.
/// Se nulla funziona restituisce `Err(nome)` con il default `adb` (da cui l'errore d'avvio).
pub fn locate() -> std::result::Result<String, String> {
    if let Ok(b) = std::env::var("ADB_BIN") {
        if !b.is_empty() {
            return Ok(b);
        }
    }
    if let Some(s) = saved_path() {
        if works(&s) {
            return Ok(s);
        }
    }
    if works("adb") {
        return Ok("adb".into());
    }
    for c in candidates() {
        if works(&c) {
            save_path(&c);
            return Ok(c);
        }
    }
    Err("adb".into())
}

/// Titolo del prompt in cui l'utente indica dove si trova adb.
pub fn path_prompt() -> String {
    if cfg!(windows) {
        t!("Percorso di adb.exe o della cartella platform-tools (es. C:\\platform-tools) — Esc per saltare").into()
    } else {
        t!("Percorso di adb o della cartella che lo contiene — Esc per saltare").into()
    }
}

/// Messaggio con i suggerimenti per installare/indicare adb.
pub fn missing_hint() -> String {
    if cfg!(windows) {
        t!("adb non trovato. Scarica platform-tools da https://developer.android.com/tools/releases/platform-tools, estrai la cartella ovunque e indicane il percorso (oppure aggiungila al PATH / imposta ADB_BIN).")
            .into()
    } else {
        t!("adb non trovato: installalo (es. `apt install adb`) o imposta ADB_BIN=/percorso/adb.").into()
    }
}

/// Wrapper del binario adb (default `adb`, sovrascrivibile con `ADB_BIN`).
#[derive(Debug, Clone)]
pub struct Adb {
    pub bin: String,
    pub serial: Option<String>,
}

impl Adb {
    pub fn new(bin: impl Into<String>) -> Self {
        Self {
            bin: bin.into(),
            serial: None,
        }
    }

    fn command(&self) -> Command {
        let mut c = Command::new(&self.bin);
        if let Some(s) = &self.serial {
            c.arg("-s").arg(s);
        }
        c
    }

    /// `adb devices -l`.
    pub fn devices(&self) -> Result<Vec<Device>> {
        let out = Command::new(&self.bin)
            .arg("devices")
            .arg("-l")
            .output()
            .with_context(|| t!("impossibile avviare '{bin} devices'", bin = self.bin))?;
        let text = String::from_utf8_lossy(&out.stdout);
        let mut v = Vec::new();
        for line in text.lines().skip(1) {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let mut it = line.split_whitespace();
            let serial = it.next().unwrap_or_default();
            let state = it.next().unwrap_or_default();
            if serial.is_empty() || state.is_empty() || serial == "*" {
                continue;
            }
            let model = it.find_map(|t| t.strip_prefix("model:")).map(|m| m.to_string());
            v.push(Device {
                serial: serial.to_string(),
                state: state.to_string(),
                model,
            });
        }
        Ok(v)
    }

    /// `adb connect ip:port` (Wi-Fi).
    pub fn connect(&self, addr: &str) -> Result<String> {
        let out = Command::new(&self.bin)
            .arg("connect")
            .arg(addr)
            .output()
            .with_context(|| t!("impossibile eseguire '{bin} connect'", bin = self.bin))?;
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    /// `adb tcpip <port>`: riavvia adbd del device (collegato via USB) in modalità Wi-Fi.
    pub fn tcpip(&self, port: u16) -> Result<String> {
        let out = self
            .command()
            .arg("tcpip")
            .arg(port.to_string())
            .output()
            .with_context(|| t!("impossibile eseguire '{bin} tcpip'", bin = self.bin))?;
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    /// Comando shell sul device: stdout testuale (exit code non verificato).
    pub fn shell(&self, cmd: &str) -> Result<String> {
        let out = self
            .command()
            .arg("shell")
            .arg(cmd)
            .output()
            .with_context(|| t!("adb shell non riuscito"))?;
        Ok(String::from_utf8_lossy(&out.stdout)
            .trim_end_matches(['\r', '\n'])
            .to_string())
    }

    /// `exec-out`: output binario raw, senza PTY (nessuna conversione CRLF).
    pub fn exec_out(&self, cmd: &str) -> Result<Child> {
        self.command()
            .arg("exec-out")
            .arg(cmd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context(t!("adb exec-out non riuscito"))
    }

    /// `exec-out` raccolto interamente in memoria.
    pub fn exec_out_collect(&self, cmd: &str) -> Result<Vec<u8>> {
        let child = self.exec_out(cmd)?;
        let out = child
            .wait_with_output()
            .context(t!("attesa adb exec-out"))?;
        Ok(out.stdout)
    }

    /// `exec-in`: stdin locale inviato binario allo stdout del comando sul device.
    pub fn exec_in(&self, cmd: &str) -> Result<Child> {
        self.command()
            .arg("exec-in")
            .arg(cmd)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .context(t!("adb exec-in non riuscito (serve adb recente)"))
    }

    /// Proprietà del device corrente.
    pub fn info(&self) -> Result<DeviceInfo> {
        let model = self.shell("getprop ro.product.model").unwrap_or_default();
        let android = self.shell("getprop ro.build.version.release").unwrap_or_default();
        let abi = self.shell("getprop ro.product.cpu.abi").unwrap_or_default();
        Ok(DeviceInfo { model, android, abi })
    }
}
