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

## Requirements

| Needed | Why | How to get it |
|---|---|---|
| **adb** (Android platform-tools) | **required**: all communication with the phone goes through it | **`abt` can download it for you on first run** (no install needed). Or by hand — Windows/macOS/Linux: [platform-tools](https://developer.android.com/tools/releases/platform-tools) (unzip anywhere) · Debian/Ubuntu: `sudo apt install adb` · Fedora: `sudo dnf install android-tools` · macOS: `brew install android-platform-tools` |
| `tar` | streams folders to/from the phone | already on Linux/macOS; on Windows built in since Windows 10 1803 (`System32\tar.exe`) |
| Phone with **USB debugging** | lets adb connect | Settings → About phone → tap *Build number* 7 times → Developer options → USB debugging; accept the RSA key on first connection |
| Android 6+ on the phone | uses the phone's built-in `toybox` | — |
| Terminal with 24-bit colors | TUI look | any modern one; on Windows use **Windows Terminal** |
| Rust 1.80+ | **only to build from source** | [rustup.rs](https://rustup.rs) |

`abt` looks for adb by itself (`ADB_BIN`, saved path, `PATH`, its own folder, Desktop/Downloads, Android SDK).
If it can't find it, the TUI offers to **download platform-tools for you** (or to enter the path by hand);
on the device screen press `d` to download, `a` to set the path. From a script: `abt install-adb`.

## Install

1. Download the binary for your system from [Releases](https://github.com/ZioZoni95/Android_Backup_tool/releases)
   (Linux, Windows) and unzip it, or build it: `cargo build --release` (binary in `target/release/abt`).
2. Run `abt`. If adb is not installed it shows OS-specific instructions and offers to **download it
   automatically** (press `y`) — nothing else to install. Or install adb yourself (see above).
3. Plug in the phone with USB debugging on and pick it from the list.
   For Wi-Fi: connect via USB once, then `abt wifi`.

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
Language: automatic from your system (Italian or English); force it with `ABT_LANG=it` / `ABT_LANG=en`.
Full guide (Italian): [docs/USAGE.it.md](docs/USAGE.it.md) · Design: [ARCHITECTURE.md](ARCHITECTURE.md) · [Changelog](CHANGELOG.md)

## License

Copyright © 2026 ZioZoni95 — released under the [MIT License](LICENSE).

---

## Italiano

Backup e file manager veloce per Android via **adb**, al posto del lento MTP.
Un solo binario, due interfacce: una **TUI** a due pannelli (Locale ↔ Device) e una **CLI** per script.

### Requisiti

| Serve | Perché | Come ottenerlo |
|---|---|---|
| **adb** (Android platform-tools) | **obbligatorio**: tutta la comunicazione col telefono passa da lì | **`abt` può scaricarlo per te al primo avvio** (nessuna installazione). Oppure a mano — Windows/macOS/Linux: [platform-tools](https://developer.android.com/tools/releases/platform-tools) (estrai ovunque) · Debian/Ubuntu: `sudo apt install adb` · Fedora: `sudo dnf install android-tools` · macOS: `brew install android-platform-tools` |
| `tar` | trasferisce le cartelle in streaming | già presente su Linux/macOS; su Windows incluso da Windows 10 1803 (`System32\tar.exe`) |
| Telefono con **debug USB** | permette la connessione adb | Impostazioni → Info telefono → tocca 7 volte *Numero build* → Opzioni sviluppatore → Debug USB; accetta la chiave RSA al primo collegamento |
| Android 6+ sul telefono | usa `toybox`, incluso nel sistema | — |
| Terminale a 24 bit di colore | aspetto della TUI | qualsiasi terminale moderno; su Windows usa **Windows Terminal** |
| Rust 1.80+ | **solo per compilare dai sorgenti** | [rustup.rs](https://rustup.rs) |

`abt` cerca adb da solo (`ADB_BIN`, percorso salvato, `PATH`, la propria cartella, Desktop/Download, Android SDK).
Se non lo trova, la TUI ti propone di **scaricare platform-tools al posto tuo** (o di indicare il percorso a mano);
nella schermata device premi `d` per scaricarlo, `a` per indicare il percorso. Da script: `abt install-adb`.

### Installazione

1. Scarica il binario per il tuo sistema da [Releases](https://github.com/ZioZoni95/Android_Backup_tool/releases)
   (Linux, Windows) ed estrailo, oppure compila: `cargo build --release` (binario in `target/release/abt`).
2. Avvia `abt`. Se adb non è installato mostra le istruzioni per il tuo sistema e ti propone di
   **scaricarlo automaticamente** (premi `y`) — non serve installare altro. Oppure installa adb a mano (vedi sopra).
3. Collega il telefono con il debug USB attivo e sceglilo dalla lista.
   Per il Wi-Fi: collega prima via USB, poi `abt wifi`.

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
Lingua: automatica dal sistema (italiano o inglese); si forza con `ABT_LANG=it` / `ABT_LANG=en`.
Guida completa: [docs/USAGE.it.md](docs/USAGE.it.md) · Design: [ARCHITECTURE.md](ARCHITECTURE.md) · [Changelog](CHANGELOG.md)

### Licenza

Copyright © 2026 ZioZoni95 — rilasciato con [licenza MIT](LICENSE).
