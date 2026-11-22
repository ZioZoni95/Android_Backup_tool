# abt — architecture

`abt` is a single Rust binary that backs up and manages Android storage over **adb** instead of MTP.
It has two front ends sharing one engine: a two-pane **TUI** (`ratatui`) and a **CLI** (`abt <command>`).

## Why not MTP

MTP is request-oriented: every file and even every directory listing is a separate transaction, so
throughput collapses on thousands of small files. `abt` uses the adb channel (USB or Wi-Fi) with a
**binary `tar` stream** instead:

- pull: `adb exec-out 'toybox tar -cf - -C <dir> <name>'` → local `tar -xf -`
- push: local `tar -cf -` → `adb exec-in 'toybox tar -xf - -C <dir>'`
- single files: `toybox dd` on the device, with **resume** through a local `.part` file (pull only)
- one continuous stream = few round trips; the practical ceiling is the USB/Wi-Fi link itself
  (see [Performance](#performance)).

## Overview

```
┌───────────────────────────── TUI (ratatui) ──────────────────────────────┐
│  LOCAL pane (std::fs)          │   DEVICE pane (toybox over adb)          │
│  selection, marks, filters     │   selection, marks, filters              │
└───────────────┬────────────────┴──────────────────┬───────────────────────┘
                │         Queue (worker thread)      │        CLI (main.rs)
                ▼                                    ▼               │
      pull_file / pull_dir / push_file / push_dir / sync_mirror ◄────┘
                │                                    │
                ▼                                    ▼
      adb exec-out / exec-in               adb shell (toybox: ls/find/stat/du/dd/tar/md5sum)
      (raw binary streaming)               short commands, text output
```

## Modules (`src/`)

| Module | Responsibility |
|---|---|
| `main.rs` | CLI dispatch, `abt help`, `install-adb`, `speedtest`; TUI event loop (also drops clipboard-paste key bursts, `drop_paste`) |
| `app.rs` | All TUI state: screens (`Screen`), the two `Pane`s, key/mouse handling, modal dialogs, job submission, background tasks (device polling, adb download) |
| `ui.rs` | Pure `ratatui` rendering of that state (blue = local, green = device) |
| `transfer.rs` | Transfer engine (`tar`/`dd`, MD5 verify, incremental sync), job `Queue` with one worker thread, `Cancelled` error, speed benchmarks |
| `device_fs.rs` | Device listing/mkdir/rm/rename via toybox (`find -print0` + batched `stat`), local listing, path guards |
| `actions.rs` | APK backup (`pm path` + stream), SMS / call-log export (`content query`), app data (`su` / `run-as`) |
| `adb.rs` | adb wrapper (devices, connect/tcpip, shell, `exec-out`/`exec-in`), **adb discovery** and **platform-tools download** |
| `places.rs` | "Go to" menu data: places, favorites/recents persisted in `places.txt`, mounted disks (Linux `/proc`, macOS `/Volumes`, Windows WinAPI), config dir |
| `i18n.rs`, `i18n_en.rs` | Language detection and the `t!()` translation macro / English catalog |
| `util.rs` | Path helpers (Windows normalization), formatting, quoting |

## Transfer queue

`Queue::spawn` starts one worker thread that runs jobs sequentially. A `JobSpec` (direction, kind
`File`/`Dir`/`Sync`, source, destination) becomes a shared `JobView` whose mutable `JobState`
(status, bytes, total, message, start/end time) is updated by the engine through the `Sink` trait.
The UI only reads `Queue::snapshot()` each frame, so the worker never touches UI state.

- Cancellation: a per-job `AtomicBool` checked inside the copy loops; the typed `transfer::Cancelled`
  error is detected with `downcast_ref`, never by comparing (translated) message text.
- `x` cancels the running job, `X` (`cancel_queued`) cancels every job still queued.
- The CLI reuses the same engine with a `CliSink` that prints progress to stderr.

## Incremental sync (`s` in the TUI, `abt sync`)

1. Recursive remote listing: `find -type f -print0 | xargs -0 stat -c '%s|%Y|%n'`.
2. Compare with the local tree by relative path.
3. Transfer only files that are **missing**, have a **different size**, or are **newer** on the device
   (2 s mtime tolerance). `--delete` removes local files that no longer exist on the device.
4. Interrupted single-file pulls resume from whole-MB offsets (`dd skip`).

## adb discovery and download

`adb::locate()` tries, in order: `ADB_BIN` → path saved in `<config>/abt/adb_path.txt` → `PATH` →
the managed copy in `<config>/abt/platform-tools` → the executable's own folder → common locations
(Desktop/Downloads, Android SDK, Homebrew, Scoop, Chocolatey, …). A path found this way is saved.

If nothing works, the TUI shows a confirmation dialog (`ConfirmAction::DownloadAdb`) with per-OS manual
instructions. Accepting runs `adb::download_platform_tools` on a background thread (`ureq` + `zip`):
download into memory (300 MB cap), zip-slip-safe extraction, `adb version` check. Progress lives in
`App::dl` and is drawn by `Screen::Download`; `abt install-adb` is the CLI equivalent.

## Localization

`i18n::lang()` is detected once (`ABT_LANG`, then `LC_ALL`/`LC_MESSAGES`/`LANGUAGE`/`LANG`, or
`GetUserDefaultLocaleName` on Windows). Source strings are Italian and act as the lookup key:
`t!("testo")` / `t!("testo {n}", n = x)`; translations live in `i18n_en.rs` and a missing key falls back
to Italian. Tests verify there are no duplicate keys and that placeholders match.

## Platforms

- **Linux** — reference platform.
- **Windows** — paths are normalized to `/` internally (`util::norm_local`), drives via WinAPI, config in
  `%APPDATA%\abt`, local `tar` is `System32\tar.exe` (bsdtar). Device files whose names are invalid on
  Windows (`: ? *` …) cannot be extracted.
- **macOS** — experimental (CI builds and tests, never run on real hardware); mounted disks come from
  `/Volumes` + `df -k -P`.

## Permission model (no root)

- `/sdcard` and microSD (`/storage/XXXX-XXXX`): readable and writable by the adb `shell` user.
- `/sdcard/Android/data` on Android 11+: denied by the system (partial transfer, the rest still copies).
- `/data/data/<pkg>`: needs root (`su`) or a debuggable app (`run-as`).
- SMS / call log: the `shell` user may run `content query`.
- APKs: readable through `pm path`.

## Safety

- `rm -rf` on the device only after an explicit on-screen confirmation (with a summary).
- Guards reject `/`, empty paths and any path containing `..`.
- Deletions only target items that were actually listed or confirmed; rename never overwrites;
  push overwrites files with the same name.

## Performance

Throughput is normally limited by the connection, not by `abt`: a bad cable or a USB 2.0 link can cap
adb at ~10 MB/s while a good USB 3 link reaches 30–60 MB/s. `abt speedtest` measures the adb link, the
phone's storage read and the PC's disk write separately and names the bottleneck.

## Known limits

- Requires toybox on the device (Android 6+); no busybox fallback.
- Non-UTF-8 file names are converted lossily in listings.
- The `content query` parser is heuristic: free-text columns (SMS body) must stay last in the projection.
- Push has no resume; MD5 verification covers single files only.
- One transfer runs at a time (jobs are queued, not parallel).

## Ideas

Parallel jobs, native in-process `tar` extraction, compression for Wi-Fi, ignore files (`.abtignore`),
guided restore, backup profiles.
