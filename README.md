# Viap

[简体中文](README.zh-CN.md)

<p align="center">
  <img src="src-tauri/icons/icon.png" width="128" height="128" alt="Viap logo">
</p>

<p align="center"><strong>Windows application and data migration tool</strong></p>
<p align="center">Move applications and selected data to another drive while keeping their original paths available.</p>

Viap is a Windows desktop application built with Tauri, React, TypeScript, and Rust. It uses NTFS directory junctions or symbolic links to redirect storage without changing application paths.

## Features

- Migrate installed applications to another local drive and restore them later.
- Migrate system folders, selected application data folders, and custom folders.
- Check running processes and file locks before migration or restore.
- Verify copied data before switching the original path to a junction or link.
- Keep migration history with restore, link health checks, import, and export.
- Uninstall applications and scan or clean related leftovers.
- Show disk usage, application snapshots, lazy-loaded icons, and background scan progress.
- Detect HDD cold-start cases and let users manually load slow application-data directories.
- Support light and dark themes, font-size settings, portable mode, and WebView2 offline installers.
- Verify the running executable against the official GitHub Release signature.

## Safety Notes

Viap is designed to keep a usable data copy until the migration switch has been verified. However, no file operation is completely safe against hardware failure or unexpected power loss. Back up important data before migrating.

- Run Viap with administrator privileges when Windows requires permission to create links or access protected folders.
- Close the application being migrated. Viap performs process and lock checks, but some software may use services, drivers, or memory-mapped files.
- Do not migrate the entire `AppData`, `Local`, or `Roaming` directory. Select data folders for one application at a time.
- Software with license binding, system services, kernel drivers, self-repair, or hard-coded paths may not work correctly after redirection.
- Do not manually delete the target directory while a migration record is active. Restore the application first when possible.
- Microsoft Store, UWP, MSIX, system components, browser installations, and GPU drivers should be handled by Windows or their vendor tools instead.
- Use local NTFS drives. Network drives and unsupported file systems are not migration targets.

## Portable and Installer Builds

- **Standard installer**: installed application with update support.
- **WebView2 offline installer**: includes the WebView2 runtime for systems that cannot download it during installation.
- **Portable ZIP**: extract and run without installation. User data is stored beside the application in `data`; portable builds do not check for updates automatically. Download new versions manually from [GitHub Releases](https://github.com/Chunyu33/viap/releases) or [Quark Drive](https://pan.quark.cn/s/4761ee4ba698).

On first launch, the portable build can copy missing data from an existing Viap installation. Existing installation data is not deleted.

## Data and Settings

- Installed builds normally use `%APPDATA%\viap`.
- Portable builds normally use the application directory's `data` folder.
- The Settings page can change Viap's data directory and copies managed data before switching to the new location.
- Themes, font size, default migration paths, recycle-bin preference, and scan settings are persisted with the user data.

## Integrity Verification

Open Settings and choose **Verify file integrity**. Viap downloads the signature for the current release from GitHub and verifies the running executable with the official public key.

- A success message is shown only when the signature and file content match exactly.
- A signature mismatch indicates that the file may have been modified.
- Network failures, missing signatures, and malformed signatures are reported separately and are not treated as tampering.

## Development

### Requirements

- Windows 10 or later
- Node.js 18 or later
- Rust and the Tauri prerequisites

### Install and Run

```bash
npm install
npm run tauri dev
```

### Build and Verify

```bash
npm run build
cargo check --manifest-path src-tauri/Cargo.toml --no-default-features
git diff --check
```

### Generate Icons

The icon source is `src-tauri/icons/icon.svg`. The scripts generate the PNG sizes used by the application, including `48x48.png`, and add 32, 48, 128, and 256 pixel layers to `icon.ico`.

```bash
node scripts/generate-icons.js
node scripts/generate-ico.js
```

## License

MIT License. See [LICENSE](LICENSE).

## Contributing

Issues and pull requests are welcome. When reporting a migration or scan problem, include the application name, source and target drive types, and the visible error message. Do not include private paths or personal data.
