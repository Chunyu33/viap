# Changelog

> English is the default changelog. See the [Chinese changelog](CHANGELOG-zh.md).

## v1.1.11

### Highlights

- Switched the copy engine to the native CopyFileExW API: preserves file timestamps and NTFS alternate data streams, and significantly speeds up small-file-heavy migrations.
- Tuned copy concurrency: large files copy sequentially for peak SSD throughput, small files copy on a bounded 8-thread pool.
- Fixed migration backups not being removed: application directories now also get file-lock detection before migration (e.g. OneDrive shell-extension DLLs loaded by Explorer), backup deletion cleans up as much as possible, and leftover backups are filtered from the app scanner so they are no longer misidentified as new apps.

## v1.1.10

### Highlights

- Added categorized, collapsible application data management with lazy size scanning.
- Added focused application data templates for Cursor, Devin, VS Code, Ollama, and ComfyUI.
- Refined application data categories and removed non-core templates from the default list.
- Moved template management and folder size scanning into dedicated Rust modules.
- Locked migration actions during uninstall and optimized leftover scanning responsiveness.
- Simplified the application data list styling to match the standard row layout.


## v1.1.9 - 2026-10-14

- Fixed release signing for NSIS installers after Tauri bundle processing.

## v1.1.8 - 2026-07-24

- Added on-demand application data scanning to reduce HDD startup stalls.
- Improved migration safety, rollback behavior, and broken junction handling.
- Improved file integrity verification for signed release artifacts.

## v1.1.7 - 2026-07-16

- Added portable-mode settings and data compatibility.
- Preserved existing installation data when switching to portable mode or changing the data directory.

## v1.1.6 - 2026-07-16

- Added release artifact integrity verification.
- Improved forced uninstall safety and cleanup result reporting.

## v1.1.5 - 2026-07-14

- Added the offline WebView2 Windows installer.
- Added the portable ZIP release and portable data directory support.

## v1.1.4 - 2026-06-17

- Improved migration error messages, notifications, and developer directory detection.
- Added an update log entry in Settings.

## v1.1.3 - 2026-06-16

- Added configurable application font size with consistent list row scaling.

## v1.1.2 - 2026-06-16

- Improved startup rendering and application list snapshot performance.
- Improved migration, restore, and progress reporting reliability.

## v1.1.1 - 2026-06-08

- Added junction-based same-disk migration without administrator privileges.
- Improved cross-disk copy safety and rollback handling.

## v1.1.0 - 2026-06-08

- Added parallel file copying and fast same-disk moves.
- Improved forced deletion safeguards and migration error handling.

## v1.0.9 - 2026-06-07

- Fixed incorrect oversized results when portable applications were detected inside shared parent directories.
