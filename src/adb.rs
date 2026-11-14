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
            .with_context(|| format!("impossibile avviare '{} devices'", self.bin))?;
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
            .with_context(|| format!("impossibile eseguire '{} connect'", self.bin))?;
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    /// `adb tcpip <port>`: riavvia adbd del device (collegato via USB) in modalità Wi-Fi.
    pub fn tcpip(&self, port: u16) -> Result<String> {
        let out = self
            .command()
            .arg("tcpip")
            .arg(port.to_string())
            .output()
            .with_context(|| format!("impossibile eseguire '{} tcpip'", self.bin))?;
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    /// Comando shell sul device: stdout testuale (exit code non verificato).
    pub fn shell(&self, cmd: &str) -> Result<String> {
        let out = self
            .command()
            .arg("shell")
            .arg(cmd)
            .output()
            .with_context(|| "adb shell non riuscito".to_string())?;
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
            .context("adb exec-out non riuscito".to_string())
    }

    /// `exec-out` raccolto interamente in memoria.
    pub fn exec_out_collect(&self, cmd: &str) -> Result<Vec<u8>> {
        let child = self.exec_out(cmd)?;
        let out = child
            .wait_with_output()
            .context("attesa adb exec-out".to_string())?;
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
            .context("adb exec-in non riuscito (serve adb recente)".to_string())
    }

    /// Proprietà del device corrente.
    pub fn info(&self) -> Result<DeviceInfo> {
        let model = self.shell("getprop ro.product.model").unwrap_or_default();
        let android = self.shell("getprop ro.build.version.release").unwrap_or_default();
        let abi = self.shell("getprop ro.product.cpu.abi").unwrap_or_default();
        Ok(DeviceInfo { model, android, abi })
    }
}
