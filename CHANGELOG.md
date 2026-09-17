# Changelog

> English is the default changelog. See the [Chinese changelog](CHANGELOG-zh.md).

## v1.2.1

- Fixed migrations failing with "error code 3" for deeply nested folders such as Yarn and npm caches, and migrated portable apps no longer go missing from the app list.
- Safer migrations: drive roots, the user profile root, Program Files roots and other system locations, as well as folders that are already a link, are refused; nested links are recreated on the target drive, an interrupted migration is rolled back on the next launch, and overwriting a leftover target moves it to the recycle bin so a misjudged overwrite can be undone.
- Faster migrations and roomier pages: scanning and lock checking share a single pass, the lock check shows progress and can be turned off, same-drive moves skip the pointless pre-checks.
- Stronger uninstall: related processes are listed and can be closed with one click, files that stay locked are queued for the next reboot and verified on the next launch, and the flow ends with a copyable or savable before/after report including the freed space; leftover scanning covers Start Menu, LocalLow and Program Files, matching services, drivers and scheduled tasks are listed, and MS Store apps are removed through the system component interface.

## v1.2.0

- Lost migration records can be rebuilt: pick the original folder on the Migration Records page, confirm, and the records are regenerated. Recovering the same folder twice is detected and skipped, and rebuilt records fill in their size automatically.
- Original and target paths in the migration records can be clicked to open their folder.
- You control the migration-data backup: turn automatic backup off or back up on demand, and import the backup with one click if the data folder is deleted; Settings also shows where the backup and config files live.
- Other improvements: changing the data folder moves only the application's own data and cleans up the leftovers, portable builds no longer leave a cache on the system drive, the layout follows the window width instead of a fixed content width, the window size is remembered, and the font size can go up to 20px.

## v1.1.11

### Highlights

- Switched the migration engine to the native CopyFileExW API: preserves file timestamps and NTFS alternate data streams, and significantly speeds up small-file-heavy migrations.
- Tuned migration concurrency: large files copy sequentially for peak SSD throughput, small files copy on a bounded 8-thread pool.
- Fixed migration backups not being removed: application directories now also get file-lock detection before migration (e.g. OneDrive shell-extension DLLs loaded by Explorer), backup deletion cleans up as much as possible, leftover backups are scheduled for auto-removal on reboot, and leftover backups are filtered from the app scanner so they are no longer misidentified as new apps.
- Refactored the migration engine into a more modular structure.
- Fixed broken links after in-app updates (e.g. opencode, Antigravity): the history page now offers one-click "re-migrate" on broken records (reuses the migration engine to move the updated version from the original path back to the target and rebuild the link; confirms before overwriting a non-empty target). Also fixed the link-status misclassification where an empty target with a fresh real directory at the original path was reported as "data lost", and added updater-component detection (Squirrel / electron-updater) that warns after migration that updates will require re-migration.

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
