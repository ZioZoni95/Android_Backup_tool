# abt — Android Backup Tool

```
   ╲ ╱
  ▄███▄     ▄▀█ █▄▄ ▀█▀
 ██████     █▀█ █▄█  █
  █▀▀█      Android Backup Tool
```

**English** · [Italiano](#italiano)

Fast backup and file manager for Android over **adb**, instead of slow MTP.
One binary, two interfaces: a two-pane **TUI** (Local ↔ Device) and a **CLI** for scripts.

## Install

1. Get a binary from [Releases](https://github.com/ZioZoni95/Android_Backup_tool/releases)
   (Linux, Windows) or build it: `cargo build --release` (Rust 1.80+, binary in `target/release/abt`).
2. Install **adb**: [platform-tools](https://developer.android.com/tools/releases/platform-tools)
   (Linux: `sudo apt install adb`). `abt` finds it by itself (`ADB_BIN`, PATH, Desktop/Downloads, Android SDK)
   and asks for the path if it can't. Windows also needs `tar` (built in since Windows 10).
3. On the phone enable **USB debugging** and accept the RSA key. For Wi-Fi: connect via USB once, then `abt wifi`.

## How to use

```sh
abt                                  # TUI: pick a device, work on two panes (press ? for keys)
abt devices                          # list devices
abt pull /sdcard/DCIM                # phone → ~/Android_Backup/Backup
abt push ./photos /sdcard/Pictures   # computer → phone
abt sync /sdcard/WhatsApp ./bk       # incremental mirror
abt backup                           # DCIM, Pictures, Documents, Download, WhatsApp + SMS and call log
abt help                             # all commands
```

Options: `-s SERIAL` · `-o DIR` · `-y` · `--verify` · `--delete`. Backup folder: `ABT_BACKUP_DIR`.
Full guide (Italian): [docs/USAGE.it.md](docs/USAGE.it.md) · Design: [ARCHITECTURE.md](ARCHITECTURE.md)

---

## Italiano

Backup e file manager veloce per Android via **adb**, al posto del lento MTP.
Un solo binario, due interfacce: una **TUI** a due pannelli (Locale ↔ Device) e una **CLI** per script.

### Installazione

1. Scarica un binario da [Releases](https://github.com/ZioZoni95/Android_Backup_tool/releases)
   (Linux, Windows) oppure compila: `cargo build --release` (Rust 1.80+, binario in `target/release/abt`).
2. Installa **adb**: [platform-tools](https://developer.android.com/tools/releases/platform-tools)
   (Linux: `sudo apt install adb`). `abt` lo trova da solo (`ADB_BIN`, PATH, Desktop/Download, Android SDK)
   e ti chiede il percorso se non ci riesce. Su Windows serve anche `tar` (incluso da Windows 10).
3. Sul telefono attiva il **debug USB** e accetta la chiave RSA. Per il Wi-Fi: collega prima via USB, poi `abt wifi`.

### Come si usa

```sh
abt                                  # TUI: scegli il device, lavora su due pannelli (? per i tasti)
abt devices                          # elenco device
abt pull /sdcard/DCIM                # telefono → ~/Android_Backup/Backup
abt push ./foto /sdcard/Pictures     # computer → telefono
abt sync /sdcard/WhatsApp ./bk       # mirror incrementale
abt backup                           # DCIM, Pictures, Documents, Download, WhatsApp + SMS e chiamate
abt help                             # tutti i comandi
```

Opzioni: `-s SERIAL` · `-o DIR` · `-y` · `--verify` · `--delete`. Cartella di backup: `ABT_BACKUP_DIR`.
Guida completa: [docs/USAGE.it.md](docs/USAGE.it.md) · Design: [ARCHITECTURE.md](ARCHITECTURE.md)
