// 特殊目录检测与迁移模块
// 负责动态检测聊天类应用数据目录，并提供安全迁移入口

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sysinfo::System;

use crate::models::MigrationResult;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

#[cfg(windows)]
use winreg::enums::HKEY_CURRENT_USER;
#[cfg(windows)]
use winreg::RegKey;

/// 特殊文件夹状态
/// 前端用于展示检测结果和目录路径；大小由 folder_manager 懒加载计算。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpecialFolder {
    pub name: String,
    pub current_path: String,
    pub is_detected: bool,
    pub size_mb: f64,
}

/// 微信 4.x 的数据目录名。新版用 xwechat_files，旧版是 WeChat Files；
/// 4.x 的配置里只记录存储根目录，真实数据在根目录下的这个子目录。
const XWECHAT_FILES_DIR: &str = "xwechat_files";

/// 微信 3.x 的数据目录名。
const LEGACY_WECHAT_FILES_DIR: &str = "WeChat Files";

/// 动态检测聊天应用数据目录
///
/// 规则：按候选路径的优先级，取第一个真实存在的目录。候选来源见
/// [`special_path_candidates`]，例如微信会依次尝试
/// 3.x 注册表 → 4.x 的 `%APPDATA%\Tencent\xwechat\config\*.ini` → 文档下的默认目录。
///
/// 改成「扫全部候选」是因为这些应用的数据目录会随版本变化：只认一条固定路径时，
/// 用户明明装了、数据只是挪了位置，界面上却显示成未找到。
pub fn detect_chat_app_data(app_name: &str) -> Option<PathBuf> {
    #[cfg(windows)]
    {
        special_path_candidates(&normalize_app_name(app_name))
            .into_iter()
            .find(|path| path.is_dir())
    }

    #[cfg(not(windows))]
    {
        let _ = app_name;
        None
    }
}

/// 获取特殊目录状态列表（聊天应用 + 开发工具 + 浏览器缓存）
pub fn get_special_folders_status() -> Result<Vec<SpecialFolder>, String> {
    #[cfg(windows)]
    {
        let mut result = Vec::new();

        // 聊天应用
        for app_name in ["wechat", "qq", "tim", "wxwork", "dingtalk", "feishu"] {
            result.push(folder_status(app_name));
        }

        // 开发工具与浏览器缓存
        for app_name in ["chrome_cache", "edge_cache", "vscode_extensions", "npm_global"] {
            result.push(folder_status(app_name));
        }

        Ok(result)
    }

    #[cfg(not(windows))]
    {
        Ok(Vec::new())
    }
}

/// 获取单个特殊目录的状态（只做路径检测，不递归读取目录）。
#[cfg(windows)]
fn folder_status(app_name: &str) -> SpecialFolder {
    let detected = detect_chat_app_data(app_name);
    let fallback = default_special_path(app_name);

    let (current_path, is_detected) = match detected {
        Some(path) => {
            (path.to_string_lossy().to_string(), true)
        }
        None => {
            let current_path = fallback
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            (current_path, false)
        }
    };

    SpecialFolder {
        name: app_name.to_string(),
        current_path,
        is_detected,
        // 首屏只返回路径元数据，避免 HDD 上递归遍历应用数据目录。
        size_mb: 0.0,
    }
}

/// 迁移特殊目录（安全工作流）
/// 1. 进程预检（目标应用必须已退出）
/// 2. 复用 migration::migrate_app 执行原子迁移与目录联接
pub fn migrate_special_folder(
    app_name: String,
    source_path: String,
    target_dir: String,
    cancel_flag: &Arc<AtomicBool>,
    app_handle: &tauri::AppHandle,
    force_overwrite: bool,
    user_confirmed_warning: bool,
) -> Result<MigrationResult, String> {
    #[cfg(windows)]
    {
        ensure_app_not_running(&app_name)?;
        crate::migration::migrate_app(app_name, source_path, target_dir, cancel_flag, app_handle, crate::models::MigrationRecordType::LargeFolder, force_overwrite, user_confirmed_warning)
    }

    #[cfg(not(windows))]
    {
        let _ = (app_name, source_path, target_dir, cancel_flag, app_handle);
        Ok(MigrationResult {
            success: false,
            message: "此功能仅支持 Windows 系统".to_string(),
            new_path: None,
        })
    }
}

#[cfg(windows)]
fn normalize_app_name(app_name: &str) -> String {
    app_name.trim().to_lowercase()
}

/// 特殊目录的所有候选路径，按优先级排列。
///
/// 一个应用的数据目录常常随版本变化（微信 3.x 与 4.x 就完全不同），
/// 只认一条固定路径会导致「装了却检测不到」。这里列出全部已知位置：
/// 检测取第一个真实存在的，未检测到时用第一个作为展示用的默认路径。
#[cfg(windows)]
fn special_path_candidates(app_name: &str) -> Vec<PathBuf> {
    match app_name {
        "wechat" => {
            let mut candidates = Vec::new();
            // 4.x 放在注册表之前：4.x 已经不用注册表了，那边可能残留着升级前的旧路径，
            // 先看 ini 才能拿到当前真正在用的目录。
            // 4.x 的存储根目录写在 %APPDATA%\Tencent\xwechat\config\<hash>.ini 里（纯文本），
            // 用户改过存储位置时只有这条线索能找到。
            if let Some(root) = wechat4_config_root() {
                push_wechat_data_candidate(&mut candidates, &root, XWECHAT_FILES_DIR);
            }
            // 3.x：注册表 FileSavePath
            if let Some(root) = wechat_registry_save_root() {
                push_wechat_data_candidate(&mut candidates, &root, LEGACY_WECHAT_FILES_DIR);
            }
            if let Some(documents) = dirs::document_dir() {
                candidates.push(documents.join(XWECHAT_FILES_DIR));
                candidates.push(documents.join(LEGACY_WECHAT_FILES_DIR));
            }
            candidates
        }
        "qq" | "tim" => {
            let mut candidates = Vec::new();
            if let Some(documents) = dirs::document_dir() {
                candidates.push(documents.join("Tencent Files"));
            }
            candidates.extend(qq_registry_candidates());
            candidates
        }
        "wxwork" => {
            let mut candidates = Vec::new();
            if let Some(documents) = dirs::document_dir() {
                candidates.push(documents.join("WXWork"));
                // 部分版本使用中文目录名
                candidates.push(documents.join("企业微信文件"));
            }
            candidates
        }
        "dingtalk" => {
            let mut candidates = Vec::new();
            if let Some(data) = dirs::data_dir() {
                candidates.push(data.join("DingTalk"));
            }
            // 7.5.16 起可在设置里改「保存位置」，其默认值是文档下的 DingTalk Files
            if let Some(documents) = dirs::document_dir() {
                candidates.push(documents.join("DingTalk Files"));
            }
            candidates
        }
        "feishu" | "lark" => {
            // Roaming 优先，其次 Local；每个基目录下三种可能的产品名都试一遍
            let mut candidates = Vec::new();
            for base in [dirs::data_dir(), dirs::data_local_dir()].into_iter().flatten() {
                for name in ["LarkShell", "Feishu", "Lark"] {
                    candidates.push(base.join(name));
                }
            }
            candidates
        }
        "chrome_cache" => path_of(dirs::data_local_dir(), r"Google\Chrome\User Data\Default\Cache"),
        "edge_cache" => path_of(dirs::data_local_dir(), r"Microsoft\Edge\User Data\Default\Cache"),
        "vscode_extensions" => path_of(dirs::home_dir(), r".vscode\extensions"),
        "npm_global" => npm_global_candidates(),
        _ => Vec::new(),
    }
}

/// 单条候选路径的便捷写法：基目录不可用时返回空列表。
#[cfg(windows)]
fn path_of(base: Option<PathBuf>, relative: &str) -> Vec<PathBuf> {
    base.map(|dir| vec![dir.join(relative)]).unwrap_or_default()
}

/// npm 全局包目录：优先读 `npm config get prefix`（支持用户自定义全局安装路径），
/// 再回退到默认的 `%APPDATA%\npm\node_modules`。
#[cfg(windows)]
fn npm_global_candidates() -> Vec<PathBuf> {
    let from_npm = std::process::Command::new("npm")
        .args(["config", "get", "prefix"])
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|prefix| prefix.trim().to_string())
        .filter(|prefix| !prefix.is_empty() && prefix != "undefined")
        .map(|prefix| PathBuf::from(prefix).join("node_modules"));

    from_npm
        .into_iter()
        .chain(dirs::data_dir().map(|dir| dir.join("npm").join("node_modules")))
        .collect()
}

/// 微信 3.x：注册表里的文件保存根目录。
#[cfg(windows)]
fn wechat_registry_save_root() -> Option<PathBuf> {
    let reg_path = r"Software\Tencent\WeChat";
    let key = RegKey::predef(HKEY_CURRENT_USER).open_subkey(reg_path).ok()?;

    let raw_path: String = key.get_value("FileSavePath").ok()?;
    parse_wechat_save_root(&raw_path, dirs::document_dir().as_deref())
}

/// 把微信配置里的「保存位置」归一成真实的数据目录。
///
/// 配置里存的通常是**上级目录**（微信 4.x 的 ini 与 3.x 的注册表都是这套语义，
/// 例如 ini 写 `E:\data`，数据在 `E:\data\xwechat_files`），
/// 但也见过直接把数据目录写进去的情况，用末级目录名区分这两种写法。
fn push_wechat_data_candidate(candidates: &mut Vec<PathBuf>, root: &Path, dir_name: &str) {
    if is_named(root, dir_name) {
        candidates.push(root.to_path_buf());
    } else {
        candidates.push(root.join(dir_name));
    }
}

/// 路径的最后一段是否等于 `name`（忽略大小写）。
fn is_named(path: &Path, name: &str) -> bool {
    path.file_name()
        .and_then(|segment| segment.to_str())
        .map(|segment| segment.eq_ignore_ascii_case(name))
        .unwrap_or(false)
}

/// 微信 4.x 的存储根目录。
///
/// 4.x 不再把路径写进注册表，而是放在
/// `%APPDATA%\Tencent\xwechat\config\<hash>.ini`（纯文本，可能带换行）。
/// 返回的是根目录本身，调用方需要再拼上 `xwechat_files`。
#[cfg(windows)]
fn wechat4_config_root() -> Option<PathBuf> {
    let config_dir = dirs::data_dir()?
        .join("Tencent")
        .join("xwechat")
        .join("config");
    wechat4_config_root_from(&config_dir, dirs::document_dir().as_deref())
}

/// 从指定配置目录读取微信 4.x 的存储根目录。
/// 单独抽出来是为了能脱离真实用户目录做单元测试。
fn wechat4_config_root_from(config_dir: &Path, documents: Option<&Path>) -> Option<PathBuf> {
    for entry in std::fs::read_dir(config_dir).ok()?.flatten() {
        let path = entry.path();
        // 同目录下还有 hardlink.dat 之类的二进制文件，只认 .ini
        let is_ini = path
            .extension()
            .map(|extension| extension.eq_ignore_ascii_case("ini"))
            .unwrap_or(false);
        if !is_ini {
            continue;
        }

        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Some(root) = parse_wechat_save_root(&raw, documents) {
            return Some(root);
        }
    }

    None
}

/// 解析微信保存路径的原始值。
///
/// 注册表 `FileSavePath` 与 4.x 的 ini 用的是同一套写法：
/// - `MyDocument:` 或 `MyDocument:\子目录` —— 相对「文档」目录
/// - 绝对路径 —— 直接使用
fn parse_wechat_save_root(raw: &str, documents: Option<&Path>) -> Option<PathBuf> {
    let trimmed = raw.trim().trim_matches('"');
    if trimmed.is_empty() {
        return None;
    }

    if let Some(rest) = trimmed.strip_prefix("MyDocument:") {
        let documents = documents?;
        let relative = rest.trim_start_matches(['\\', '/']);
        if relative.is_empty() {
            return Some(documents.to_path_buf());
        }
        return Some(documents.join(relative));
    }

    Some(PathBuf::from(trimmed))
}

/// QQ/TIM：老版本会把 `Tencent Files` 建在安装目录旁，
/// 用它作为「文档目录」之外的补充线索。
#[cfg(windows)]
fn qq_registry_candidates() -> Vec<PathBuf> {
    let key = match RegKey::predef(HKEY_CURRENT_USER).open_subkey(r"Software\Tencent\QQ2012") {
        Ok(key) => key,
        Err(_) => return Vec::new(),
    };

    let mut candidates = Vec::new();
    for value_name in ["InstallPath", "Install", "QQPath", "Executable"] {
        let install: String = key.get_value(value_name).unwrap_or_default();
        let install = install.trim().trim_matches('"');
        if install.is_empty() {
            continue;
        }

        let install_path = PathBuf::from(install);
        candidates.push(install_path.join("Tencent Files"));
        candidates.push(install_path);
    }

    candidates
}

/// 未检测到时的回退展示路径：取第一个候选，与各应用原本的默认值一致。
#[cfg(windows)]
fn default_special_path(app_name: &str) -> Option<PathBuf> {
    special_path_candidates(app_name).into_iter().next()
}

#[cfg(windows)]
fn expected_process_names(app_name: &str) -> &'static [&'static str] {
    match normalize_app_name(app_name).as_str() {
        "wechat" => &["wechat.exe"],
        "qq" => &["qq.exe"],
        "tim" => &["tim.exe"],
        "wxwork" => &["wxwork.exe"],
        "dingtalk" => &["dingtalk.exe"],
        "feishu" | "lark" => &["feishu.exe", "lark.exe"],
        "chrome_cache" => &["chrome.exe"],
        "edge_cache" => &["msedge.exe"],
        "vscode_extensions" => &["code.exe"],
        _ => &[],
    }
}

#[cfg(windows)]
fn ensure_app_not_running(app_name: &str) -> Result<(), String> {
    let expected = expected_process_names(app_name);
    if expected.is_empty() {
        return Ok(());
    }

    let mut system = System::new_all();
    system.refresh_all();

    let mut running: Vec<String> = Vec::new();
    for process in system.processes().values() {
        let name = process.name().to_string_lossy().to_lowercase();
        if expected.iter().any(|candidate| name == *candidate) {
            running.push(name);
        }
    }

    running.sort();
    running.dedup();

    if running.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "检测到应用仍在运行，请先关闭后再迁移: {}",
            running.join(", ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        is_named, parse_wechat_save_root, push_wechat_data_candidate, wechat4_config_root_from,
        LEGACY_WECHAT_FILES_DIR, XWECHAT_FILES_DIR,
    };
    use std::path::{Path, PathBuf};

    /// 建一个临时配置目录，返回路径；调用方负责删除。
    fn temp_config_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("创建临时目录失败");
        dir
    }

    #[test]
    fn wechat4_config_root_reads_save_path_from_ini() {
        let dir = temp_config_dir("viap_test_xwechat_config");
        // 真实环境里同目录下还有一个二进制文件，必须被忽略
        std::fs::write(dir.join("hardlink.dat"), b"\x00\x01").unwrap();
        // 内容与实测一致：绝对路径 + 换行
        std::fs::write(dir.join("51a1fffea11325a1e4104c6b3de47af7.ini"), "E:\\data\r\n").unwrap();

        assert_eq!(
            wechat4_config_root_from(&dir, None),
            Some(PathBuf::from("E:\\data"))
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wechat4_config_root_treats_blank_ini_as_missing() {
        let dir = temp_config_dir("viap_test_xwechat_blank");
        std::fs::write(dir.join("config.ini"), "   \r\n").unwrap();

        assert_eq!(wechat4_config_root_from(&dir, None), None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wechat4_config_root_returns_none_without_config_dir() {
        // 没装微信时不能panic
        assert_eq!(
            wechat4_config_root_from(Path::new("Z:\\not-exist-viap"), None),
            None
        );
    }

    #[test]
    fn wechat_data_candidate_resolves_parent_directory() {
        // ini/注册表里存的是上级目录时应拼上数据目录名
        // （本机实测：ini 写 E:\data，数据在 E:\data\xwechat_files）
        let mut candidates = Vec::new();
        push_wechat_data_candidate(&mut candidates, Path::new("E:\\data"), XWECHAT_FILES_DIR);
        assert_eq!(candidates, vec![PathBuf::from("E:\\data\\xwechat_files")]);
    }

    #[test]
    fn wechat_data_candidate_keeps_path_that_is_already_the_data_dir() {
        let mut candidates = Vec::new();
        push_wechat_data_candidate(
            &mut candidates,
            Path::new("D:\\Chats\\WeChat Files"),
            LEGACY_WECHAT_FILES_DIR,
        );
        assert_eq!(candidates, vec![PathBuf::from("D:\\Chats\\WeChat Files")]);
    }

    #[test]
    fn is_named_matches_last_segment_ignoring_case() {
        assert!(is_named(Path::new("D:\\a\\wechat FILES"), LEGACY_WECHAT_FILES_DIR));
        assert!(!is_named(Path::new("D:\\a"), LEGACY_WECHAT_FILES_DIR));
    }

    #[test]
    fn wechat_save_root_accepts_absolute_path() {
        // 微信 4.x 的 ini 里存的就是这种绝对路径（实测形如 "E:\\data"）
        assert_eq!(
            parse_wechat_save_root("E:\\data", None),
            Some(PathBuf::from("E:\\data"))
        );
    }

    #[test]
    fn wechat_save_root_resolves_mydocument_prefix() {
        let documents = Path::new("C:\\Users\\tester\\Documents");
        // MyDocument: 单独出现时表示「文档」目录本身
        assert_eq!(
            parse_wechat_save_root("MyDocument:", Some(documents)),
            Some(documents.to_path_buf())
        );
        // 带子目录时拼到文档目录下
        assert_eq!(
            parse_wechat_save_root("MyDocument:\\WeChatData", Some(documents)),
            Some(documents.join("WeChatData"))
        );
    }

    #[test]
    fn wechat_save_root_trims_quotes_and_newlines() {
        assert_eq!(
            parse_wechat_save_root("\"D:\\Chats\"\r\n", None),
            Some(PathBuf::from("D:\\Chats"))
        );
        assert_eq!(parse_wechat_save_root("   \r\n", None), None);
    }

    #[test]
    fn wechat_save_root_without_documents_dir_returns_none() {
        // 拿不到「文档」目录时不能panic，直接放弃这一条候选即可
        assert_eq!(parse_wechat_save_root("MyDocument:", None), None);
    }
}
