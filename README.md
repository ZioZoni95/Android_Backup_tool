# abt — Android Backup Tool

```
      ╲    ╱
     ▄██████▄
     █ ████ █
     ▀▀▀▀▀▀▀▀
  ▄█ ████████ █▄
  ██ ████████ ██
  ██ ████████ ██
  ▀█ ████████ █▀
     ▀██▀▀██▀
      ██  ██
      ▀▀  ▀▀
```

Backup e file manager per Android via **adb**, al posto del lento MTP.
Due interfacce nello stesso binario: una **TUI** a due pannelli (Locale ↔ Device)
e una **CLI** per script e backup automatici.
Design e limiti: vedi [ARCHITECTURE.md](ARCHITECTURE.md).

## Requisiti

- Linux/macOS, Rust 1.80+
- `adb` nel PATH (Debian/Ubuntu: `sudo apt install adb`; Fedora: `sudo dnf install android-tools`)
  oppure `ADB_BIN=/percorso/adb`
- Telefono con **debug USB** attivo (accetta la chiave RSA al primo collegamento).
  Per il Wi-Fi: prima USB, poi `abt wifi`.

## Build

```sh
cargo build --release
# binario: target/release/abt
```

## Cartella di backup di default

I trasferimenti verso il computer finiscono in:

```
~/Android_Backup/Backup        (creata automaticamente)
```

Puoi cambiarla con la variabile `ABT_BACKUP_DIR`. Nella TUI è la cartella iniziale
del pannello Locale; nella CLI è la destinazione di `pull` e `backup` se non ne indichi una.

## TUI

```sh
./target/release/abt
```

Scegli il device (Invio, oppure `c` per `adb connect IP:5555`), poi lavori su due
pannelli: **Locale** e **Device** (parte da `/sdcard`). Premi **`?`** in qualsiasi
momento per l'elenco completo dei comandi.

| Tasto                     | Azione                                                   |
|---------------------------|----------------------------------------------------------|
| `Tab` / `Shift+Tab`       | cambia pannello attivo                                   |
| `↑↓` `j k` `PgUp` `PgDn`  | muovi il cursore (`g`/`G` inizio/fine)                   |
| `→` `l`                   | entra nella cartella                                     |
| `←` `h` `Backspace`       | cartella padre (il cursore torna sulla cartella lasciata)|
| `Invio`                   | cartella: entra · file: trasferisci nell'altro pannello  |
| `/`                       | filtra per nome (`Esc` rimuove il filtro)                |
| `o`                       | ordina per nome → dimensione → data                      |
| `.`                       | mostra/nasconde i file nascosti                          |
| `r`                       | aggiorna il pannello                                     |
| `Spazio` / `Ins`          | marca/smarca e scende                                    |
| `J` `K` / `Shift+↑↓`      | estende la selezione                                     |
| `a` · `*` · `Esc`         | seleziona tutto · inverti · azzera                       |
| `y`                       | pull device → locale                                     |
| `p`                       | push locale → device                                     |
| `s`                       | mirror incrementale della cartella device → locale       |
| `n` / `F7`                | nuova cartella                                           |
| `R` / `F2`                | rinomina                                                 |
| `d` / `Canc`              | elimina (con riepilogo e conferma)                       |
| `S`                       | esporta SMS + registro chiamate (JSONL)                  |
| `x`                       | annulla il trasferimento attivo                          |
| `q` / `Ctrl+C`            | esci                                                     |

**Selezione multipla e destinazione.** Marca più voci (anche cartelle) e premi `y`/`p`:
compare un riepilogo (cartelle, file, dimensione) e la **cartella di destinazione**,
precompilata con quella dell'altro pannello. Modificala se serve e conferma con Invio;
se non esiste viene creata. Il titolo del pannello mostra `[✓ N · dimensione]` delle
voci marcate.

I trasferimenti compaiono in basso con progresso, coda e resume su `.part`.

## CLI

```sh
abt devices                                  # elenco device
abt connect 192.168.1.5:5555                 # Wi-Fi adb
abt wifi                                     # adbd → tcpip (serve USB)
```

**File e cartelle sul device**

```sh
abt ls -l /sdcard/DCIM                       # elenco (default /sdcard)
abt pull /sdcard/DCIM/Camera                 # → ~/Android_Backup/Backup
abt pull /sdcard/DCIM /sdcard/Download ./bk  # più sorgenti, destinazione = ultimo argomento
abt pull -o ./bk /sdcard/DCIM --verify       # destinazione con -o, verifica MD5 sui file singoli
abt push ./foto ./video /sdcard/Pictures     # più sorgenti, cartella remota creata se manca
abt sync /sdcard/WhatsApp ./bk/WhatsApp      # mirror incrementale (--delete rimuove in locale)
abt mkdir /sdcard/Nuova
abt mv /sdcard/Nuova /sdcard/Rinominata
abt rm /sdcard/Vecchia                       # chiede conferma; -y per saltarla
```

**Backup**

```sh
abt backup                                   # DCIM, Pictures, Documents, Download, WhatsApp + SMS e chiamate
abt backup -o /mnt/disco /sdcard/Music       # cartelle e destinazione a scelta
abt apk list                                 # pacchetti utente
abt apk pull com.whatsapp ./apk              # base + split APK
abt sms ./sms.jsonl                          # SMS → JSONL
abt calllog ./chiamate.jsonl                 # registro chiamate → JSONL
abt appdata com.example ./appdata            # dati privati app (root o app debuggabile)
```

Opzioni: `-s SERIAL` (più device collegati) · `-o DIR` · `-y` · `--verify` · `--delete`.
`abt help` mostra il riepilogo.

## Come funziona

Un unico flusso `tar` binario su `adb exec-out`/`exec-in` invece di una transazione per file
(MTP), quindi throughput tipicamente 30–60 MB/s su USB. I file singoli usano `dd` con resume.
Il sync confronta percorso, dimensione e data e trasferisce solo il necessario.

## Permessi (senza root)

- `/sdcard` e microSD: lettura/scrittura OK
- `/sdcard/Android/data` su Android 11+: accesso negato dal sistema (trasferimento parziale)
- `/data/data/<pkg>`: serve root o app debuggabile
- SMS/chiamate: funzionano con l'utente `shell` di adb

## Sicurezza

Le eliminazioni chiedono sempre conferma (in TUI con riepilogo di cartelle, file e dimensione);
`/`, stringhe vuote e percorsi con `..` sono rifiutati. La rinomina non sovrascrive elementi esistenti
(il push invece sovrascrive i file con lo stesso nome).

## Limiti noti

Richiede toybox (Android 6+); nomi non UTF-8 vengono convertiti "lossy" nel listing; push senza
resume. Dettagli in [ARCHITECTURE.md](ARCHITECTURE.md).
