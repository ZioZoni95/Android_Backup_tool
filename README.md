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

- Linux, macOS o Windows 10+ (per compilare: Rust 1.80+; sulle [Release](https://github.com/ZioZoni95/Android_Backup_tool/releases) trovi i binari pronti)
- `adb` nel PATH (Debian/Ubuntu: `sudo apt install adb`; Fedora: `sudo dnf install android-tools`)
  oppure `ADB_BIN=/percorso/adb`
- Telefono con **debug USB** attivo (accetta la chiave RSA al primo collegamento).
  Per il Wi-Fi: prima USB, poi `abt wifi`.

### Windows

> Testato su Windows 11. Segnalazioni e issue sono benvenute.

- Installa [platform-tools](https://developer.android.com/tools/releases/platform-tools) e
  aggiungi la cartella al PATH (oppure `set ADB_BIN=C:\\percorso\\adb.exe`).
- Serve `tar` (incluso in Windows 10 1803+ come `C:\\Windows\\System32\\tar.exe`).
- Consigliato **Windows Terminal** (la console legacy può mostrare male icone e box-drawing).
- Config in `%APPDATA%\\abt\\places.txt`; backup di default in `%USERPROFILE%\\Android_Backup\\Backup`.
- I percorsi locali si scrivono con `/` o `\\` (`D:\\Foto`); le unità sono nel menu «Vai a».
- File del device con nomi non validi su Windows (`:`, `?`, `*`, …) non si possono estrarre.

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
| `↑↓` `j k` `PgUp` `PgDn`  | muovi il cursore (`Home`/`G` inizio/fine)                   |
| `→` `l`                   | entra nella cartella                                     |
| `g` / click sul titolo    | **Vai a…**: luoghi, preferiti, recenti, percorso a mano  |
| click sul path in alto    | salta a quel segmento (`/sdcard/DCIM` → clic su `sdcard`)|
| `B`                       | aggiungi/rimuovi la cartella corrente dai preferiti      |
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
| `T`                       | trasferisce l'**intera cartella corrente** (anche `y`/`p` sulla voce `..`) |
| `s`                       | mirror incrementale della cartella device → locale       |
| `n` / `F7`                | nuova cartella                                           |
| `R` / `F2`                | rinomina                                                 |
| `d` / `Canc`              | elimina (con riepilogo e conferma)                       |
| `S`                       | esporta SMS + registro chiamate (JSONL)                  |
| `x`                       | annulla il trasferimento attivo                          |
| `q` / `Ctrl+C`            | esci                                                     |

**Aspetto.** I due pannelli hanno colori distinti: **blu = computer (Locale)**, **verde = device
Android**. Il pannello attivo ha il titolo evidenziato (`LOCALE`/`DEVICE`), il bordo colorato e il
cursore pieno `▌`; quello inattivo resta tenue con un cursore appena visibile. In alto a destra
trovi sempre quale pannello è attivo. I file hanno icona e colore per tipo (cartelle, immagini,
video, audio, archivi, APK, documenti), le voci marcate sono in giallo con `●`, e i
trasferimenti mostrano una barra di avanzamento. Serve un terminale con colori a 24 bit
(praticamente tutti quelli moderni).

**Mouse.** Click sinistro seleziona (doppio click: apre la cartella o trasferisce il file),
la rotella scorre. **Click destro** (o `m` da tastiera) apre il menu contestuale sulla voce
sotto il cursore: apri, copia (→ altro pannello), marca, rinomina, elimina; e sulla cartella
corrente: copia l'intera cartella, sync incrementale, nuova cartella, seleziona tutto,
ordina, file nascosti, aggiorna. Il menu mostra in basso la **dimensione reale** (ricorsiva, in byte, con numero di file e cartelle) della voce, delle voci marcate o della cartella corrente, calcolata in background. Nel menu: click, `↑↓` + Invio, `Esc` per chiudere.
Con il mouse attivo, per selezionare testo nel terminale tieni premuto `Shift`.

**Cambiare percorso senza scrivere.** Il percorso nel titolo di ogni pannello è cliccabile
a segmenti. `g` (o un click sul titolo) apre "Vai a…" con:

- **Luoghi**: sul computer Home, Backup, Scrivania, Documenti, Download, Immagini… ; sul device
  Memoria interna, Fotocamera (DCIM), Immagini, Download, Documenti, Musica, WhatsApp e la
  scheda SD (solo quelli che esistono davvero)
- **⛁ Dischi montati**: chiavette, hard disk, partizioni e condivisioni di rete (nome del volume, spazio libero/totale),
  aggiornati ogni volta che apri "Vai a…": scegli un disco nel pannello Locale e scarica lì i dati del device
- **★ Preferiti**: `B` aggiunge/rimuove la cartella corrente
- **↺ Recenti**: le ultime 8 cartelle visitate per pannello
- **✎ Digita un percorso…** per inserirlo a mano

Preferiti e recenti sono salvati in `~/.config/abt/places.txt`.

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
abt disks                                    # dischi montati sul computer
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
