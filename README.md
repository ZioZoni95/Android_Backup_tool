# abt — Android Backup Tool

Backup e file manager per Android via **adb** (TUI two-pane), al posto del
lento MTP. Vedi [ARCHITECTURE.md](ARCHITECTURE.md) per design e limiti.

## Requisiti

- Linux/macOS, Rust 1.80+ (`cargo build --release`)
- `adb` nel PATH (Debian/Ubuntu: `sudo apt install adb`; Fedora: `sudo dnf install android-tools`)
  oppure `ADB_BIN=/percorso/adb`
- Telefono: debug USB attivo; per Wi-Fi: prima connessione USB poi `abt wifi`

## Build

```sh
cargo build --release
# binario: target/release/abt
```

## TUI

```sh
./target/release/abt
```

1. scegli il device (Invio) — da USB o da `adb connect IP:5555`
2. due pannelli: **Locale** ↔ **Device** (parte da `/sdcard`)

| Tasto            | Azione                                        |
|------------------|-----------------------------------------------|
| `Tab`            | cambia pannello attivo                        |
| `↑↓` / `j k`     | naviga (`g`/`G` inizio/fine)                  |
| `Invio`          | apre directory; su un file = trasferimento     |
| `h` / `Backspace`| directory padre                               |
| `Spazio`         | marca/deseleziona (`a` = tutto)               |
| `y`              | pull selezionati/marcati → pannello locale    |
| `p`              | push selezionati/marcati → directory device   |
| `s`              | mirror incrementale della dir device → locale |
| `d`              | elimina (con conferma)                        |
| `n`              | nuova cartella                                |
| `r`              | aggiorna pannello                             |
| `S`              | esporta SMS + registro chiamate (JSONL)       |
| `x`              | annulla il trasferimento attivo                |
| `q`              | esci                                          |

Trasferimenti con progresso, coda e resume su `.part` in fondo alla schermata.

## CLI

```sh
abt devices                                  # elenco
abt connect 192.168.1.5:5555                 # Wi-Fi adb
abt wifi                                     # adbd → tcpip (serve USB)
abt pull  /sdcard/DCIM ./backup --verify     # directory o file singolo
abt push  ./foto /sdcard/Pictures
abt sync  /sdcard/WhatsApp ./bk/WhatsApp     # mirror incrementale
abt apk list                                 # pacchetti utente
abt apk pull com.whatsapp ./apk              # base + split APK
abt sms ./sms.jsonl                          # SMS → JSONL
abt calllog ./chiamate.jsonl                 # registro chiamate → JSONL
abt appdata com.example ./appdata            # dati privati (root o run-as)
```

Multi-device: aggiungi `-s SERIAL` a qualsiasi comando.

## Risoluzione problemi

- `unauthorized` → sblocca il telefono e conferma la finestra RSA
- `offline` → `adb kill-server && adb start-server`
- Wi-Fi lento/disconnesso → meglio USB; il Wi-Fi adb resta comunque più
  reattivo di MTP per molti file piccoli
