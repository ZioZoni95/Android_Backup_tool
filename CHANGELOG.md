# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed
- `abt speedtest` no longer blames phone storage when its read speed is within 15% of the link speed (the read test goes through the same link, so the link is the real limit).
- Terminals that paste the clipboard on right click (VS Code's integrated terminal on Windows) no longer turn the pasted text into commands: the event loop drains all ready events and `drop_paste` discards bursts of ≥ 16 character/Enter/Tab key events unless a text input is open (`App::accepts_text`); arrows, Esc and mouse events are kept, so the context menu still opens. Unit-tested.
- Panic (`min > max` in `i64::clamp`) when pressing arrow keys on an empty list — an empty folder in a pane or no device on the selection screen: the upper bound was `len.saturating_sub(1)`, which is `-1` for signed integers. Fixed in `App::move_sel` and `App::dev_move`, with a regression test.
- Context menu no longer depends on the terminal forwarding the right button: middle click and Ctrl+left click open it too (some terminals, e.g. VS Code's integrated terminal and Windows Terminal, intercept right click for copy/paste); the `m` key still works. Covered by a unit test on `App::on_mouse`.

### Added
- `abt speedtest [<file>]`: measures 256 MB through three separate paths and names the bottleneck — adb link (`toybox dd if=/dev/zero` over `exec-out`), phone storage read (`dd` from a large file, auto-picked in DCIM/Download/Movies or given as argument) and local disk write (`transfer::bench_local_write`, pseudo-random blocks, fsynced, temp file removed). `-o DIR` selects the disk to test.
- Device list auto-refresh: on the device selection screen `App::poll_devices` runs `adb devices -l` on a background thread every 2 s and updates the list (keeping the selection in range), so a phone plugged in after startup appears without pressing `r`. Unit-tested with a fake adb script.
- `ABT_DEBUG_MOUSE=1` logs received mouse button events in the Log panel to diagnose terminal mouse handling.
- macOS support (experimental, untested on real hardware): `places::mounted_disks()` for macOS lists the `/Volumes/*` mounts using `df -k -P` (`parse_df_kib`, unit-tested); Linux keeps the `/proc/self/mounts` implementation (now gated with `cfg(all(unix, not(target_os = "macos")))`).
- CI job `macos` (build + test) and release assets `abt-macos-arm64.tar.gz` / `abt-macos-x86_64.tar.gz` (the Intel build is cross-compiled on the arm64 runner with `--target x86_64-apple-darwin`); the release workflow now builds with an optional per-matrix `target`.
- adb lookup also checks Homebrew locations (`/opt/homebrew/bin`, `/usr/local/bin`) on macOS; `Movies` is recognised as the local video folder.

- adb auto-install: when adb cannot be found the TUI offers to download Android platform-tools from `dl.google.com` (OS-specific URL via `adb::platform_tools_url`) into `<config>/abt/platform-tools`, with per-OS manual instructions (Windows / macOS `brew` / Linux `apt`, `dnf`, `pacman`). Key `d` on the device screen re-opens the offer; `Esc` during the download cancels it.
- `abt install-adb` CLI command doing the same download/extraction with progress on stderr.
- `adb::download_platform_tools` (`ureq` + `zip`, no external tools): streams into memory with a 300 MB cap, extracts with `enclosed_name()` (zip-slip safe), restores Unix modes, runs `adb kill-server` on a previous managed copy first (Windows file lock), verifies with `adb version`.
- adb lookup now also checks the managed copy and the executable's own folder (`./adb`, `./platform-tools/adb`).
- New dependencies: `ureq` 2 (rustls), `zip` 2 (deflate only).
- README: documents the automatic adb download in the requirements table and install steps (EN/IT).

## [0.4.0] - 2026-11-22

### Added
- Multi-language UI: `src/i18n.rs` detects the system language at startup (`LC_ALL`/`LC_MESSAGES`/`LANGUAGE`/`LANG` on Unix, `GetUserDefaultLocaleName` on Windows via `windows-sys` feature `Win32_Globalization`); `ABT_LANG=it|en` overrides it. Italian locales get Italian, every other locale gets English.
- `t!()` macro: the Italian source text is the lookup key, translations live in `src/i18n_en.rs`; named placeholders (`t!("... {n}", n = x)`); a missing key falls back to Italian. Unit tests check duplicate keys and placeholder consistency across languages.
- `transfer::Cancelled`: typed cancellation error, detected with `downcast_ref` instead of matching the (now translated) message text.

### Changed
- All TUI, CLI (`abt help`, messages, errors) and place-label strings go through `t!()`.
- Release workflow: the GitHub release body is now the matching `CHANGELOG.md` section (English) plus download/requirements notes, instead of auto-generated commit links; `workflow_dispatch` with a `tag` input rebuilds and refreshes an existing release.
- README: explicit requirements table (adb platform-tools, `tar`, USB debugging, Android 6+/toybox, 24-bit terminal, Rust only for source builds) in both languages; documents `ABT_LANG`.
- `Cargo.toml` version 0.3.0 → 0.4.0.

## [0.3.0] - 2026-11-22

### Added
- MIT `LICENSE` file; `license` and `repository` fields in `Cargo.toml`.
- Release archives now ship the `LICENSE` file next to the binary.
- `abt version` / `--version` / `-V` prints version and copyright; the same line is appended to `abt help`.
- `transfer::Queue::cancel_queued()` and TUI key `X`: cancels every job still in `Queued` state; the running job is untouched.
- `JobState` now records `started`/`ended` (`Instant`); `JobSnap` exposes `elapsed` (seconds, frozen once the job ends).
- Transfers panel: aggregate progress bar (finished jobs + fraction of the running one), per-status counters (`✔ ▶ … ✖ ⊘`), per-job average speed and ETA for running jobs, elapsed time for completed jobs.

### Changed
- Transfers panel height is now dynamic (`1 + min(jobs, 12)` rows plus borders, clamped to 5..40% of the terminal height) instead of a fixed 5 rows that showed only the last 3 jobs.
- Visible job rows are chosen by priority (running, failed, queued, then most recent finished); overflow is collapsed into a `… altri N` row.
- Log panel height 4 → 6 rows and it renders as many lines as fit, instead of a hard-coded 3.
- The worker skips jobs that were cancelled while still queued.

## [0.2.0] - 2026-11-22

### Added
- Windows support: local paths normalized to `/` (`util::norm_local`), drive-root detection (`C:/`), `USERPROFILE` as home fallback, native MD5 (`md-5` crate), mounted-drive listing through `windows-sys` (`GetLogicalDrives`, `GetDiskFreeSpaceExW`, `GetVolumeInformationW`).
- adb discovery (`adb::locate`): `ADB_BIN` → saved path (`<config>/abt/adb_path.txt`) → `PATH` → common locations (Desktop/Downloads/Documents subfolders, Android SDK, `ANDROID_HOME`/`ANDROID_SDK_ROOT`, Scoop, Chocolatey, `C:\platform-tools`). A path found in a common location is persisted.
- TUI prompt (`InputAction::AdbPath`) opened at startup when adb cannot be found, and via `a` on the device screen; accepts either `adb[.exe]` or the directory containing it, validated with `adb version`.
- `places::cfg_dir()` shared config directory (`XDG_CONFIG_HOME` → `%APPDATA%` → `~/.config`, plus `abt/`).
- `.github/workflows/release.yml`: on `v*` tags builds Linux (`abt-linux-x86_64.tar.gz`) and Windows (`abt-windows-x86_64.zip`) binaries and attaches them to a GitHub release.
- `CLAUDE.md` with build commands and architecture notes.
- `docs/USAGE.it.md`: full Italian usage guide (moved out of the README).

### Changed
- CI now builds and tests on Linux and Windows; the Windows job is no longer `continue-on-error`.
- README rewritten as a short bilingual (EN/IT) document with install and usage sections and a compact banner.
- `Cargo.toml` version 0.1.0 → 0.2.0.

## [0.1.0] - 2026-11-14

### Added
- Two-pane `ratatui` TUI (local ↔ device) and CLI in a single `abt` binary.
- Transfer engine over a single binary `tar` stream (`adb exec-out` / `exec-in`); single files via `dd` with `.part` resume on pull; optional MD5 verification for single files.
- Background job queue with one worker thread, progress reporting and cancellation (`x`).
- Incremental sync (`s` / `abt sync`) comparing relative path, size and mtime (2 s tolerance), with `--delete`.
- Device file operations through toybox (`ls`/`find`/`stat`/`mkdir`/`mv`/`rm`), with guards against `/`, empty and `..` paths and mandatory delete confirmation.
- Whole-folder transfer (`T`, or `y`/`p` on `..`), multi-selection with editable destination.
- Mouse support, right-click context menu with recursive size calculation, clickable breadcrumb, "Go to" menu (places, favorites, recents persisted in `places.txt`, mounted disks).
- `abt disks`, `abt backup`, `abt apk`, `abt sms`, `abt calllog`, `abt appdata` (`su`/`run-as`).

[Unreleased]: https://github.com/ZioZoni95/Android_Backup_tool/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/ZioZoni95/Android_Backup_tool/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/ZioZoni95/Android_Backup_tool/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/ZioZoni95/Android_Backup_tool/compare/1ec11bf...v0.2.0
[0.1.0]: https://github.com/ZioZoni95/Android_Backup_tool/commit/1ec11bf
