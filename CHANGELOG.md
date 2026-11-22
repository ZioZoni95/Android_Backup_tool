# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed
- README: explicit requirements table (adb platform-tools, `tar`, USB debugging, Android 6+/toybox, 24-bit terminal, Rust only for source builds) in both languages.

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

[Unreleased]: https://github.com/ZioZoni95/Android_Backup_tool/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/ZioZoni95/Android_Backup_tool/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/ZioZoni95/Android_Backup_tool/compare/1ec11bf...v0.2.0
[0.1.0]: https://github.com/ZioZoni95/Android_Backup_tool/commit/1ec11bf
