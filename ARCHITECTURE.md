# Android Backup Tool (abt)

File manager TUI + tool di backup per Android che usa **adb invece di MTP**.

## Perché non MTP

MTP è un protocollo orientato a richieste per-oggetto: ogni file e persino ogni
listing è una transazione separata, con latenza elevata e cattivo parallelismo.
Su migliaia di file (es. fotocamera) la velocità effettiva crolla a pochi MB/s.

`abt` usa il canale adb (USB o Wi-Fi) con **streaming tar binario**:

- pull: `adb exec-out 'toybox tar -cf - -C <dir> <nome>'` → `tar -xf -` locale
- push: `tar -cf -` locale → `adb exec-in 'toybox tar -xf - -C <dir>'`
- singoli file: `dd` sul device con **resume** automatico su `.part`
- un solo flusso continuo = poche latenze, throughput tipicamente 30–60 MB/s su USB

## Architettura

```
┌──────────────────────────── TUI (ratatui) ────────────────────────────┐
│  pannello LOCAL (std::fs)      │   pannello DEVICE (toybox via adb)  │
│  selezioni, marcature          │   selezioni, marcature              │
└──────────────┬─────────────────┴───────────────┬─────────────────────┘
               │      Queue (worker thread)      │
               ▼                                 ▼
      pull_file / pull_dir / push_file / push_dir / sync_mirror
               │                                 │
               ▼                                 ▼
     adb exec-out / exec-in          adb shell (toybox)
   (streaming binario raw)        (ls/find/stat/du/dd/tar/md5sum)
```

Moduli:

| Modulo        | Responsabilità                                              |
|---------------|-------------------------------------------------------------|
| `adb.rs`      | wrapper binario adb: devices, connect/tcpip, shell, exec-out/in |
| `device_fs.rs`| listing device (find+NUL, stat batch), mkdir/rm, listing locale |
| `transfer.rs` | motore trasferimenti (tar/dd/cat), coda job con worker, sync |
| `actions.rs`  | backup APK, SMS/call log (content query), app-data (su/run-as) |
| `app.rs`      | stato TUI: schermate, pannelli, tasti, job                   |
| `ui.rs`       | rendering ratatui                                            |

## Modello di permessi (senza root)

- `/sdcard` (storage condiviso): leggibile e scrivibile dall'utente `shell` di adb
- `/storage/XXXX-XXXX` (microSD): idem
- `/data/data/<pkg>`: **richiede root** (`su`) o app debuggabile (`run-as`)
- SMS / registro chiamate: l'utente `shell` ha i privilegi per `content query`
- APK: leggibili via `pm path` + `cat`

## Sync incrementale (`s` in TUI, `abt sync` in CLI)

1. elenco ricorsivo remoto: `find -type f -print0 | xargs -0 stat -c '%s|%Y|%n'`
2. confronto con l'albero locale per percorso relativo
3. trasferisce solo file **mancanti**, **con dimensione diversa** o **remoto più
   recente** (skew di 2s di tolleranza)
4. `--delete` rimuove in locale ciò che non esiste più sul device
5. resume: i file parziali `.part` riprendono da blocchi MB interi (`dd skip`)

## Sicurezza

- `rm -rf` sul device solo dopo conferma esplicita a schermo
- guardia anti-percorso-pericoloso: mai `/`, `..`, stringhe vuote
- cancellazioni solo su elementi effettivamente elencati o confermati

## Limitazioni note (v0.1)

- richiede toybox (presente su Android 6+); nessun fallback busybox
- nomi file con caratteri non UTF-8 vengono "lossy" nel listing
- parser `content query` euristico: i campi di testo libero (body SMS) devono
  stare in coda alla proiezione (già così) e newline nel valore sono gestiti;
  valori contenenti la sequenza `", <col>="` potrebbero confondere il parser
- tar su alberi con permessi negati (es. `/sdcard/Android/data` su Android 11+)
  riporta errori parziali: il resto dei file viene comunque trasferito
- push senza resume (i pull hanno resume); verifica MD5 solo su file singoli

## Roadmap

- v0.2: parallelismo multi-job, verifica MD5 estesa, compressione (gzip pipe),
  filtri/esclusioni (`.abtignore`), rename, ricerca con `/`
- v0.3: profili backup (DCIM+WhatsApp+Download), restore guidato, statistiche
