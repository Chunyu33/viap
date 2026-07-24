# Viap

[English](README.md)

<p align="center">
  <img src="src-tauri/icons/icon.png" width="128" height="128" alt="Viap 图标">
</p>

<p align="center"><strong>Windows 应用与数据迁移工具</strong></p>
<p align="center">将应用和指定数据迁移到其他磁盘，同时保留原路径访问能力。</p>

Viap 是一款基于 Tauri、React、TypeScript 和 Rust 的 Windows 桌面工具，通过 NTFS Junction 或符号链接重定向存储，不改变应用原有路径。

## 功能

- 将已安装应用迁移到其他本地磁盘，并支持恢复。
- 支持系统文件夹、指定应用数据目录和自定义文件夹迁移。
- 迁移或恢复前检测运行进程和文件占用。
- 复制完成并校验数据后，才切换原路径为 Junction 或符号链接。
- 保存迁移历史，支持恢复、链接健康检查、导入和导出。
- 卸载应用并扫描、清理相关残留。
- 显示磁盘使用情况、应用快照、懒加载图标和后台扫描进度。
- 识别机械硬盘冷启动场景，应用数据目录由用户主动加载，避免页面长时间卡顿。
- 支持浅色/深色主题、字号设置、便携版和 WebView2 离线安装版。
- 使用 GitHub Release 官方签名校验当前运行的 exe 文件。

## 安全须知

Viap 会尽量保证迁移切换完成并验证成功后才清理源目录，但任何文件操作都无法抵御硬件故障或意外断电。迁移重要数据前请先做好备份。

- Windows 创建链接或访问受保护目录时，请以管理员身份运行 Viap。
- 迁移前请关闭目标应用。Viap 会检测进程和文件占用，但部分软件还可能使用服务、驱动或内存映射文件。
- 不建议整体迁移 `AppData`、`Local` 或 `Roaming`，应按单个软件选择具体数据目录。
- 带许可证绑定、系统服务、内核驱动、自修复机制或硬编码路径的软件，迁移后可能无法正常运行。
- 迁移记录存在时不要手动删除目标目录；如需卸载，建议先恢复应用。
- Microsoft Store、UWP、MSIX、系统组件、浏览器安装目录和 GPU 驱动应使用 Windows 或软件厂商提供的迁移方式。
- 请使用本地 NTFS 磁盘，网络驱动器和不支持的文件系统不能作为迁移目标。

## 便携版与安装版

- **普通安装版**：安装运行，支持自动更新。
- **WebView2 离线安装版**：安装包内置 WebView2 运行环境，适合无法联网下载安装运行环境的电脑。
- **便携版 ZIP**：解压后直接运行，无需安装。用户数据默认保存在程序目录下的 `data` 文件夹；便携版不会自动检查更新，请从 [GitHub Releases](https://github.com/Chunyu33/viap/releases) 或[夸克网盘](https://pan.quark.cn/s/4761ee4ba698)手动下载新版本。

便携版首次启动时可以从已有 Viap 安装中复制缺失数据，不会删除原安装版数据。

## 数据与设置

- 安装版默认使用 `%APPDATA%\viap`。
- 便携版默认使用程序目录下的 `data` 文件夹。
- 设置页可以更改 Viap 数据目录，切换前会先复制受管理的数据。
- 主题、字号、默认迁移路径、回收站设置和扫描设置会随用户数据保存。

## 文件完整性校验

打开设置页，点击“校验文件完整性”。Viap 会从 GitHub 获取当前版本的签名文件，并使用官方公钥校验当前运行的 exe。

- 只有签名和文件内容完全一致时才提示校验通过。
- 签名不匹配时提示文件可能被篡改。
- 网络失败、签名缺失和签名格式错误会分别提示，不会默认判定为文件被篡改。

## 开发

### 环境要求

- Windows 10 或更高版本
- Node.js 18 或更高版本
- Rust 及 Tauri 所需开发环境

### 安装与运行

```bash
npm install
npm run tauri dev
```

### 构建与验证

```bash
npm run build
cargo check --manifest-path src-tauri/Cargo.toml --no-default-features
git diff --check
```

### 生成图标

图标源文件为 `src-tauri/icons/icon.svg`。脚本会生成应用所需的各尺寸 PNG，包括 `48x48.png`，并将 32、48、128、256 像素图层写入 `icon.ico`。

```bash
node scripts/generate-icons.js
node scripts/generate-ico.js
```

## 许可证

MIT License，详见 [LICENSE](LICENSE)。

## 贡献

欢迎提交 Issue 和 Pull Request。反馈迁移或扫描问题时，请提供应用名称、源盘和目标盘类型，以及界面显示的错误信息；请勿提交私人路径或个人数据。
