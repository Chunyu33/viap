// 应用卸载模块
// 负责执行卸载流程、扫描残留项、执行清理

use serde::{Deserialize, Serialize};

#[cfg(windows)]
use std::collections::HashSet;
#[cfg(windows)]
use std::fs;
#[cfg(windows)]
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::process::Command;
#[cfg(windows)]
use std::thread;
#[cfg(windows)]
use std::time::Duration;
#[cfg(windows)]
use walkdir::WalkDir;
#[cfg(windows)]
use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ};
#[cfg(windows)]
use winreg::{HKEY, RegKey};

#[cfg(windows)]
const BLACKLIST: &[&str] = &["microsoft", "windows", "common files", "tauri", "webview2"];

/// 卸载请求参数
/// 支持按 app_id（通常传显示名）或 registry_path 定位应用
#[derive(Debug, Deserialize)]
pub struct UninstallInput {
    pub app_id: Option<String>,
    pub registry_path: Option<String>,
    /// 前端传入的安装路径（scanner 可能从 DisplayIcon 推导得出，
    /// 此时注册表中的 InstallLocation 实际为空，强删需要这个字段才能定位目录）
    pub install_location: Option<String>,
    /// 是否使用回收站（null/None 默认 true，即移入回收站而非彻底删除）
    pub use_recycle_bin: Option<bool>,
}

#[cfg(windows)]
fn sanitize_search_text(raw: &str) -> String {
    raw.chars()
        .filter(|ch| !ch.is_control())
        .collect::<String>()
        .trim()
        .trim_matches('"')
        .to_string()
}

#[cfg(windows)]
#[derive(Debug, Clone)]
struct StrictScanContext {
    app_name_exact: String,
    app_folder_name: String,
    publisher_name: Option<String>,
    install_location: Option<String>,
    uninstall_path_hints: Vec<String>,
}

#[cfg(windows)]
fn normalize_match_text(raw: &str) -> String {
    sanitize_search_text(raw).to_lowercase()
}

#[cfg(windows)]
fn normalize_windows_path(raw: &str) -> String {
    normalize_match_text(raw).replace('/', r"\")
}

#[cfg(windows)]
fn extract_last_path_component(path: &str) -> Option<String> {
    let normalized = normalize_windows_path(path);
    if normalized.is_empty() {
        return None;
    }

    Path::new(&normalized)
        .file_name()
        .map(|v| normalize_match_text(&v.to_string_lossy()))
        .filter(|v| !v.is_empty())
}

#[cfg(windows)]
fn build_strict_scan_context(
    app_name: &str,
    publisher: Option<&str>,
    install_location: Option<&str>,
) -> Option<StrictScanContext> {
    let app_name_exact = normalize_match_text(app_name);
    if app_name_exact.is_empty() {
        return None;
    }

    let install_location = install_location
        .map(normalize_windows_path)
        .filter(|v| !v.is_empty());

    let app_folder_name = install_location
        .as_deref()
        .and_then(extract_last_path_component)
        .or_else(|| {
            app_name_exact
                .split(|c: char| matches!(c, '\\' | '/' | ':' | '"' | '*' | '?' | '<' | '>' | '|'))
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .max_by_key(|v| v.len())
                .map(|v| v.to_string())
        })?;

    let publisher_name = publisher
        .map(normalize_match_text)
        .filter(|v| !v.is_empty());

    let uninstall_path_hints = collect_uninstall_path_hints(&app_name_exact, install_location.as_deref());

    Some(StrictScanContext {
        app_name_exact,
        app_folder_name,
        publisher_name,
        install_location,
        uninstall_path_hints,
    })
}

#[cfg(windows)]
fn matches_keywords(text: &str, keywords: &[String]) -> bool {
    keywords.iter().any(|kw| keyword_matches(text, kw))
}

#[cfg(windows)]
fn keyword_matches(text: &str, keyword: &str) -> bool {
    if keyword.is_empty() {
        return false;
    }

    if keyword.chars().all(|ch| ch.is_ascii_alphanumeric()) {
        return contains_ascii_keyword_with_boundary(text, keyword);
    }

    text.contains(keyword)
}

#[cfg(windows)]
fn contains_ascii_keyword_with_boundary(text: &str, keyword: &str) -> bool {
    for (start, _) in text.match_indices(keyword) {
        let end = start + keyword.len();

        let before_is_word = if start == 0 {
            false
        } else {
            text[..start]
                .chars()
                .last()
                .map(|c| c.is_ascii_alphanumeric())
                .unwrap_or(false)
        };

        let after_is_word = if end >= text.len() {
            false
        } else {
            text[end..]
                .chars()
                .next()
                .map(|c| c.is_ascii_alphanumeric())
                .unwrap_or(false)
        };

        if !before_is_word && !after_is_word {
            return true;
        }
    }

    false
}

#[cfg(windows)]
fn is_safe_registry_cleanup_target(hkey: HKEY, sub_path: &str, keywords: &[String]) -> bool {
    let normalized = sub_path.trim().trim_matches('\\').to_lowercase();
    if normalized.is_empty() || normalized == "software" {
        return false;
    }

    if is_blacklisted_registry_path(hkey, &normalized) {
        return false;
    }

    // 仅允许清理 Software 子树，避免触碰系统关键分支
    if !normalized.starts_with("software\\") {
        return false;
    }

    // 顶层供应商目录风险高（如 Software\Microsoft），至少要求二级路径
    if normalized.split('\\').count() < 3 {
        return false;
    }

    let key = match RegKey::predef(hkey).open_subkey_with_flags(sub_path, KEY_READ) {
        Ok(v) => v,
        Err(_) => return false,
    };

    // 空键可安全删除（常见于卸载后残留空壳）
    if is_registry_key_empty(&key) {
        return true;
    }

    if keywords.is_empty() {
        return false;
    }

    registry_key_belongs_to_app(&key, &normalized, keywords)
}

#[cfg(windows)]
fn is_registry_key_empty(key: &RegKey) -> bool {
    key.enum_keys().next().is_none() && key.enum_values().next().is_none()
}

#[cfg(windows)]
fn registry_key_belongs_to_app(key: &RegKey, sub_path: &str, keywords: &[String]) -> bool {
    if matches_keywords(sub_path, keywords) {
        return true;
    }

    let display_name: String = key.get_value("DisplayName").unwrap_or_default();
    let publisher: String = key.get_value("Publisher").unwrap_or_default();
    let install_location: String = key.get_value("InstallLocation").unwrap_or_default();
    let uninstall_string: String = key.get_value("UninstallString").unwrap_or_default();

    [display_name, publisher, install_location, uninstall_string]
        .into_iter()
        .map(|v| sanitize_search_text(&v).to_lowercase())
        .any(|v| !v.is_empty() && matches_keywords(&v, keywords))
}

#[cfg(windows)]
fn wait_until_uninstalled(input: &UninstallInput) -> bool {
    // 卸载进程已退出，等待子进程启动（Inno Setup 等会 fork 自身到临时目录再执行）
    thread::sleep(Duration::from_millis(2000));

    for _ in 0..60 {
        if !is_application_still_installed(input) {
            return true;
        }
        thread::sleep(Duration::from_millis(1000));
    }
    false
}

#[cfg(windows)]
fn is_application_still_installed(input: &UninstallInput) -> bool {
    // 1. 按精确 registry_path 检查注册表键
    if let Some(registry_path) = input.registry_path.as_ref().filter(|p| !p.trim().is_empty()) {
        if let Some((hkey, sub_path)) = parse_registry_path(registry_path) {
            if RegKey::predef(hkey).open_subkey_with_flags(sub_path, KEY_READ).is_ok() {
                return true;
            }
        }
    }

    // 2. 按 DisplayName 搜索注册表
    if let Some(app_id) = input.app_id.as_ref() {
        if find_uninstall_by_display_name(app_id).is_some() {
            return true;
        }
    }

    // 3. 按 InstallLocation 搜索注册表（DisplayName 不匹配时回退）
    if let Some(location) = input.install_location.as_ref().filter(|l| !l.trim().is_empty()) {
        if !find_uninstall_commands_by_install_location(location).is_empty() {
            return true;
        }
        // 4. 文件系统兜底：安装目录中仍存在 exe/dll 则视为卸载未完成
        if directory_contains_executables(Path::new(location)) {
            return true;
        }
    }

    false
}

/// 检查目录顶层是否仍包含 exe/dll 文件，用于判断卸载器是否已执行清理
#[cfg(windows)]
fn directory_contains_executables(dir: &Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                let lower = ext.to_lowercase();
                if lower == "exe" || lower == "dll" {
                    return true;
                }
            }
        }
    }
    false
}

/// 残留项结构
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct LeftoverItem {
    pub path: String,
    pub item_type: String, // Folder | File | Registry
    pub size_mb: f64,
    pub selected: bool,
}

/// 卸载命令返回结果
#[derive(Debug, Serialize, Deserialize)]
pub struct UninstallResult {
    pub success: bool,
    pub message: String,
    pub command: Option<String>,
    pub leftovers: Vec<LeftoverItem>,
}

/// 清理执行结果
#[derive(Debug, Serialize, Deserialize)]
pub struct CleanupResult {
    pub success: bool,
    pub message: String,
    pub cleaned_count: usize,
    pub failed_items: Vec<String>,
}

/// 卸载命令预览结果
#[derive(Debug, Serialize, Deserialize)]
pub struct UninstallPreview {
    pub commands: Vec<String>,
}

/// 预览卸载命令（不执行）
/// 供前端在确认对话框中展示即将运行的卸载命令
pub fn preview_uninstall(input: UninstallInput) -> Result<UninstallPreview, String> {
    #[cfg(windows)]
    {
        let commands = resolve_uninstall_commands(&input)?;
        Ok(UninstallPreview { commands })
    }

    #[cfg(not(windows))]
    {
        let _ = input;
        Ok(UninstallPreview { commands: vec!["仅支持 Windows".to_string()] })
    }
}

/// 强制删除（跳过卸载器）
/// 用于卸载程序已损坏/缺失的场景，直接删除安装目录并清理注册表
/// 返回被删除的路径列表，供前端决定是否继续残留扫描
pub fn force_remove_application(input: UninstallInput) -> Result<UninstallResult, String> {
    #[cfg(windows)]
    {
        let app_name = input
            .app_id
            .as_deref()
            .unwrap_or("未知应用")
            .to_string();
        let use_recycle = input.use_recycle_bin.unwrap_or(true);

        let result = execute_force_remove(&input, use_recycle)?;
        let (deleted_files, deleted_registry, install_location) = result;

        // 写入操作日志（审计追溯）
        crate::storage::operation_log::add_operation_log(
            &app_name,
            "force_remove",
            "success",
            &format!("强制删除完成，文件已删除:{}, 注册表已清理:{}", deleted_files, deleted_registry),
            Some(if use_recycle { "recycle_bin" } else { "permanent" }),
        );

        // 清理该应用的迁移记录和兜底元数据（避免已卸载的应用仍显示为"已迁移"）
        if let Some(ref loc) = install_location {
            crate::storage::history::delete_migration_record_by_path(loc);
        }

        let parts: Vec<&str> = vec![
            if deleted_files { Some("文件已删除") } else { None },
            if deleted_registry { Some("注册表已清理") } else { None },
        ]
        .into_iter()
        .flatten()
        .collect();

        let method_label = if use_recycle { "（已移入回收站）" } else { "" };

        Ok(UninstallResult {
            success: true,
            message: format!(
                "强制删除完成：{}。{}{}",
                parts.join("，"),
                if install_location.is_some() {
                    "建议运行残留扫描彻底清理。"
                } else {
                    ""
                },
                method_label,
            ),
            command: Some("force_remove".to_string()),
            leftovers: Vec::new(),
        })
    }

    #[cfg(not(windows))]
    {
        let _ = input;
        Ok(UninstallResult {
            success: false,
            message: "强制删除仅支持 Windows".to_string(),
            command: None,
            leftovers: Vec::new(),
        })
    }
}

#[cfg(windows)]
fn execute_force_remove(
    input: &UninstallInput,
    use_recycle_bin: bool,
) -> Result<(bool, bool, Option<String>), String> {
    let mut deleted_files = false;
    let mut deleted_registry = false;

    let app_id = input
        .app_id
        .as_deref()
        .unwrap_or("");

    // 优先使用前端传入的安装路径（scanner 可能从 DisplayIcon 推导）
    let mut install_location: Option<String> = input
        .install_location
        .as_ref()
        .map(|v| sanitize_search_text(v))
        .filter(|v| !v.is_empty());

    // 按 registry_path 定位安装目录（补充 install_location + 删除注册表键）
    if let Some(registry_path) = input.registry_path.as_ref().filter(|p| !p.trim().is_empty()) {
        if let Some((hkey, sub_path)) = parse_registry_path(registry_path) {
            if let Ok(key) = RegKey::predef(hkey).open_subkey_with_flags(sub_path, KEY_READ) {
                // 如果前端没传安装路径，尝试从注册表读取（含 DisplayIcon 回退推导）
                if install_location.is_none() {
                    install_location = read_install_location_with_fallback(&key);
                }
            }

            // 删除注册表键
            if is_safe_registry_cleanup_target(hkey, sub_path, &[]) {
                deleted_registry = RegKey::predef(hkey)
                    .delete_subkey_all(sub_path)
                    .is_ok();
            }
        }
    }

    // 按 app_id 回退查找安装目录和注册表
    if let Some(app_id_val) = input.app_id.as_ref() {
        let registry_roots: [(HKEY, &str); 3] = [
            (HKEY_LOCAL_MACHINE, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall"),
            (HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall"),
            (HKEY_CURRENT_USER, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall"),
        ];

        for (hkey, root) in registry_roots {
            let uninstall_key = match RegKey::predef(hkey).open_subkey_with_flags(root, KEY_READ) {
                Ok(v) => v,
                Err(_) => continue,
            };

            for subkey_name in uninstall_key.enum_keys().filter_map(|x| x.ok()) {
                let subkey = match uninstall_key.open_subkey_with_flags(&subkey_name, KEY_READ) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let dn: String = subkey.get_value("DisplayName").unwrap_or_default();
                if dn.trim().to_lowercase() != app_id_val.trim().to_lowercase() {
                    continue;
                }

                // 如果前端没传安装路径，尝试从注册表读取（含 DisplayIcon 回退推导）
                if install_location.is_none() {
                    install_location = read_install_location_with_fallback(&subkey);
                }

                // 删除注册表键（如果还没删过）
                if !deleted_registry {
                    let full_path = format!(r"{}\{}", root, subkey_name);
                    if is_safe_registry_cleanup_target(hkey, &full_path, &[]) {
                        deleted_registry = RegKey::predef(hkey)
                            .delete_subkey_all(&full_path)
                            .is_ok();
                    }
                }
                break; // 找到匹配项，退出内层循环
            }
        }
    }

    // 删除安装目录（多重安全检查）
    if let Some(ref loc) = install_location {
        let install_path = Path::new(loc);
        if install_path.exists() {
            // 安全检查：验证目标目录确实是当前应用的目录，而非误识别的上级/无关目录
            validate_deletion_target(install_path, app_id)?;

            if use_recycle_bin {
                // 移入回收站（默认，可恢复）
                trash::delete(install_path)
                    .map_err(|e| format!("移入回收站失败: {}。已拒绝直接删除以确保安全。", e))?;
            } else {
                // 彻底删除
                force_delete_path(install_path)?;
            }
            deleted_files = true;
        }
    }

    if !deleted_files && !deleted_registry {
        return Err("未找到可清理的文件或注册表项。应用可能已被完全卸载。".to_string());
    }

    Ok((deleted_files, deleted_registry, install_location))
}

/// 删除目标安全校验：确保不会误删无关目录
/// 多层防护：黑名单 → 路径深度 → 目录名匹配
#[cfg(windows)]
fn validate_deletion_target(target: &Path, app_id: &str) -> Result<(), String> {
    // 第 0 层：必须存在
    if !target.exists() {
        return Err(format!("目标路径不存在: {}", target.display()));
    }

    // 第 1 层：黑名单（系统目录 / 通用父目录）
    if is_blacklisted_path(target) {
        return Err(format!(
            "拒绝删除系统/受保护目录: {}。该目录在黑名单中。",
            target.display()
        ));
    }

    // 第 2 层：路径深度检测，拒绝盘符根或浅层目录
    // 至少需要 3 级深度（例如 C:\Users\xxx\AppName），防止误删 C:\Users\xxx\AppData
    let depth = target.components().count();
    if depth < 3 {
        return Err(format!(
            "拒绝删除顶层/浅层目录 (深度={}): {}。至少需要 3 级路径深度。",
            depth,
            target.display()
        ));
    }

    // 第 3 层：目录名与应用名匹配检测
    // 目录名必须包含应用名的关键部分，或应用名包含目录名
    if !app_id.is_empty() {
        let dir_name = target
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let app_lower = app_id.to_lowercase();

        // 直接包含关系
        if dir_name.contains(&app_lower) || app_lower.contains(&dir_name) {
            return Ok(());
        }

        // 分词匹配：应用名中的关键词至少有一个出现在目录名中
        // 例如 "Postman x64" → ["postman", "x64"]，目录 "Postman" 匹配 "postman"
        let app_keywords: Vec<&str> = app_lower.split_whitespace().collect();
        if app_keywords.iter().any(|kw| dir_name.contains(kw)) {
            return Ok(());
        }

        // 都不匹配 → 拒绝
        return Err(format!(
            "安全校验未通过：目录名 '{}' 与应用名 '{}' 无匹配关系，拒绝删除以防误删无关应用。",
            dir_name, app_id
        ));
    }

    Ok(())
}

/// 强力卸载入口
/// 1) 解析并执行卸载命令（等待卸载进程退出）
/// 2) 返回成功后由前端手动确认是否触发残留扫描
pub async fn uninstall_application(input: UninstallInput) -> Result<UninstallResult, String> {
    #[cfg(windows)]
    {
        let app_name = input
            .app_id
            .as_deref()
            .unwrap_or("未知应用")
            .to_string();
        let uninstall_cmds = resolve_uninstall_commands(&input)?;
        eprintln!("[viap][uninstall] 候选卸载命令数量: {}", uninstall_cmds.len());
        let mut executed_cmd: Option<String> = None;
        let mut command_errors: Vec<String> = Vec::new();

        for uninstall_cmd in uninstall_cmds {
            eprintln!("[viap][uninstall] 尝试执行命令: {}", uninstall_cmd);
            match start_uninstall_process(&uninstall_cmd) {
                Ok(_) => {
                    executed_cmd = Some(uninstall_cmd.clone());
                    if !wait_until_uninstalled(&input) {
                        eprintln!("[viap][uninstall] 命令执行后仍检测到已安装，继续尝试下一条命令");
                        continue;
                    }

                    crate::storage::operation_log::add_operation_log(
                        &app_name,
                        "uninstall",
                        "success",
                        &format!("卸载命令已执行: {}", uninstall_cmd),
                        None,
                    );

                    // 清理该应用的迁移记录和兜底元数据（避免已卸载的应用仍显示为"已迁移"）
                    if let Some(ref loc) = input.install_location {
                        crate::storage::history::delete_migration_record_by_path(loc);
                    }

                    return Ok(UninstallResult {
                        success: true,
                        message: "卸载流程已完成。请在前端手动确认后再触发残留扫描。".to_string(),
                        command: Some(uninstall_cmd),
                        leftovers: Vec::new(),
                    });
                }
                Err(err) => {
                    command_errors.push(format!("{} => {}", uninstall_cmd, err));
                }
            }
        }

        if let Some(cmd) = executed_cmd {
            return Err(format!(
                "卸载命令已执行但仍检测到应用存在（可能未在卸载向导中确认完成）：{}",
                cmd
            ));
        }

        if !command_errors.is_empty() {
            return Err(format!("卸载命令执行失败：{}", command_errors.join(" | ")));
        }

        Err("未找到可执行的卸载命令".to_string())
    }

    #[cfg(not(windows))]
    {
        let _ = input;
        Ok(UninstallResult {
            success: false,
            message: "卸载功能仅支持 Windows 系统".to_string(),
            command: None,
            leftovers: Vec::new(),
        })
    }
}

/// 独立残留扫描命令
/// 供前端在需要时单独触发扫描
pub fn scan_app_residue(
    app_name: String,
    publisher: Option<String>,
    install_location: Option<String>,
) -> Result<Vec<LeftoverItem>, String> {
    #[cfg(windows)]
    {
        let mut leftovers = scan_app_residue_internal(app_name, publisher, install_location)?;
        leftovers.sort_by(|a, b| b.size_mb.partial_cmp(&a.size_mb).unwrap_or(std::cmp::Ordering::Equal));
        Ok(leftovers)
    }

    #[cfg(not(windows))]
    {
        let _ = (app_name, publisher, install_location);
        Ok(Vec::new())
    }
}

/// 执行清理命令
/// items 支持两类输入：
/// - 文件/目录路径: C:\\xxx\\yyy
/// - 注册表路径: HKCU\\Software\\xxx 或 HKLM\\Software\\xxx
pub fn execute_cleanup(
    items: Vec<String>,
    app_name: Option<String>,
    publisher: Option<String>,
) -> Result<CleanupResult, String> {
    #[cfg(windows)]
    {
        let cleanup_keywords = build_keywords(
            app_name.as_deref().unwrap_or_default(),
            publisher.as_deref(),
            None,
        );

        if items.is_empty() {
            return Ok(CleanupResult {
                success: true,
                message: "没有需要清理的项目".to_string(),
                cleaned_count: 0,
                failed_items: Vec::new(),
            });
        }

        let mut cleaned_count = 0usize;
        let mut failed_items: Vec<String> = Vec::new();

        for item in items {
            if let Some((hkey, sub_path)) = parse_registry_path(&item) {
                if !is_safe_registry_cleanup_target(hkey, sub_path, &cleanup_keywords) {
                    failed_items.push(item);
                    continue;
                }

                let deleted = RegKey::predef(hkey)
                    .delete_subkey_all(sub_path)
                    .is_ok();
                if deleted {
                    cleaned_count += 1;
                } else {
                    failed_items.push(item);
                }
                continue;
            }

            let path = PathBuf::from(item.trim());
            if !path.exists() {
                continue;
            }

            if is_blacklisted_path(&path) {
                failed_items.push(path.to_string_lossy().to_string());
                continue;
            }

            match force_delete_path(&path) {
                Ok(()) => cleaned_count += 1,
                Err(err) => {
                    eprintln!(
                        "[viap][cleanup] 强制删除失败 {} => {}",
                        path.display(),
                        err
                    );
                    failed_items.push(path.to_string_lossy().to_string());
                }
            }
        }

        let success = failed_items.is_empty();
        let message = if success {
            format!("清理完成，共删除 {} 项", cleaned_count)
        } else if cleaned_count == 0 {
            format!(
                "所有 {} 项清理均失败（可能需管理员权限），请以管理员身份运行后重试",
                failed_items.len()
            )
        } else {
            format!(
                "清理部分完成：成功 {} 项，失败 {} 项（失败项可能需管理员权限）",
                cleaned_count,
                failed_items.len()
            )
        };

        crate::storage::operation_log::add_operation_log(
            app_name.as_deref().unwrap_or("未知应用"),
            "cleanup",
            if success { "success" } else { "partial_failure" },
            &message,
            None,
        );

        Ok(CleanupResult {
            success,
            message,
            cleaned_count,
            failed_items,
        })
    }

    #[cfg(not(windows))]
    {
        let _ = (items, app_name, publisher);
        Ok(CleanupResult {
            success: false,
            message: "清理功能仅支持 Windows 系统".to_string(),
            cleaned_count: 0,
            failed_items: Vec::new(),
        })
    }
}

// ============================================================================
// 卸载命令解析与执行
// ============================================================================

#[cfg(windows)]
fn resolve_uninstall_commands(input: &UninstallInput) -> Result<Vec<String>, String> {
    let mut tried_registry_path = false;

    // 1. 按 registry_path 读取卸载命令
    if let Some(registry_path) = input.registry_path.as_ref().filter(|p| !p.trim().is_empty()) {
        tried_registry_path = true;
        let cmds = read_uninstall_commands_from_registry_path(registry_path);
        if !cmds.is_empty() {
            return Ok(cmds);
        }
        // registry_path 无有效命令时回退到 app_id 搜索，而非立即报错
        eprintln!(
            "[viap][uninstall] registry_path 无卸载命令，回退按 DisplayName 搜索: {}",
            registry_path
        );
    }

    // 2. 按 app_id（DisplayName 精确匹配）搜索注册表
    if let Some(app_id) = input.app_id.as_ref() {
        let cmds = find_uninstall_commands_by_display_name(app_id);
        if !cmds.is_empty() {
            return Ok(cmds);
        }
        // DisplayName 精确匹配失败时，回退按 InstallLocation 搜索
        // 文件系统扫描到的应用，display_name 取自目录名，可能与注册表 DisplayName 不完全一致
        if let Some(location) = input.install_location.as_ref().filter(|l| !l.trim().is_empty()) {
            let cmds = find_uninstall_commands_by_install_location(location);
            if !cmds.is_empty() {
                return Ok(cmds);
            }
            // 最后兜底：在安装目录中扫描卸载器可执行文件
            // 适用于注册表中完全没有有效卸载信息的便携/绿色应用
            if let Some(exe_path) = scan_uninstaller_in_directory(location) {
                return Ok(vec![exe_path]);
            }
        }
        let msg = format!("未找到应用 '{}' 的卸载命令", app_id);
        return Err(msg);
    }

    if tried_registry_path {
        Err("未在注册表中找到可用的卸载命令，且未提供应用名称用于搜索。".to_string())
    } else {
        Err("参数无效：请提供 app_id 或 registry_path".to_string())
    }
}

#[cfg(windows)]
fn start_uninstall_process(uninstall_cmd: &str) -> Result<(), String> {
    let cmd = uninstall_cmd.trim();
    if cmd.is_empty() {
        return Err("卸载命令为空".to_string());
    }

    // 解析命令为程序路径 + 参数
    let (program, args) = parse_program_and_args(cmd)
        .ok_or_else(|| format!("无法解析卸载命令: {}", cmd))?;

    if is_definitely_invalid_program(&program) {
        return Err(format!("卸载命令无效，程序路径非法: {}", program));
    }

    // 对本地路径检查可执行文件是否存在（msiexec 等系统命令除外）
    let is_path_like = program.contains('\\') || program.contains(':');
    if is_path_like
        && !program.eq_ignore_ascii_case("msiexec")
        && !program.eq_ignore_ascii_case("msiexec.exe")
    {
        if !Path::new(&program).exists() {
            return Err(format!("卸载程序不存在: {}", program));
        }
    }

    let working_dir = derive_working_dir(&program);
    eprintln!(
        "[viap][uninstall] 直接启动: {} {} | cwd: {}",
        program,
        args.join(" "),
        working_dir
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "<default>".to_string())
    );

    // 方案 A：直接执行卸载程序（等待进程退出）
    match spawn_and_wait(&program, &args, working_dir.as_deref()) {
        Ok(_) => return Ok(()),
        Err(err) => {
            // 方案 B：权限不足时提权重试（唯一合理的回退路径）
            if is_elevation_required_error(&err) {
                eprintln!(
                    "[viap][uninstall] 检测到权限提升需求，尝试提权执行: {} {}",
                    program,
                    args.join(" ")
                );
                return spawn_elevated_and_wait(&program, &args, working_dir.as_deref());
            }
            return Err(err);
        }
    }
}

#[cfg(windows)]
fn is_elevation_required_error(message: &str) -> bool {
    let normalized = message.to_lowercase();
    normalized.contains("os error 740")
        || normalized.contains("elevation")
        || normalized.contains("需要提升")
}

#[cfg(windows)]
fn spawn_elevated_and_wait(program: &str, args: &[String], working_dir: Option<&Path>) -> Result<(), String> {
    let escaped_program = escape_ps_single_quoted(program);
    let escaped_working_dir = working_dir
        .map(|dir| escape_ps_single_quoted(&dir.to_string_lossy()))
        .unwrap_or_default();

    let arg_clause = if args.is_empty() {
        String::new()
    } else {
        let quoted_args = args
            .iter()
            .map(|arg| format!("'{}'", escape_ps_single_quoted(arg)))
            .collect::<Vec<_>>()
            .join(", ");
        format!(" -ArgumentList @({})", quoted_args)
    };

    let script = if escaped_working_dir.is_empty() {
        format!(
            "$ErrorActionPreference='Stop'; \
             $p=Start-Process -FilePath '{}'{} -Verb RunAs -Wait -PassThru; \
             exit $p.ExitCode",
            escaped_program, arg_clause
        )
    } else {
        format!(
            "$ErrorActionPreference='Stop'; \
             $p=Start-Process -FilePath '{}'{} -WorkingDirectory '{}' -Verb RunAs -Wait -PassThru; \
             exit $p.ExitCode",
            escaped_program, arg_clause, escaped_working_dir
        )
    };

    let mut command = Command::new("powershell");
    command
        .arg("-NoProfile")
        .arg("-NonInteractive")
        .arg("-ExecutionPolicy")
        .arg("Bypass")
        .arg("-Command")
        .arg(script);

    let mut child = command
        .spawn()
        .map_err(|e| format!("启动提权卸载失败: {}", e))?;

    let status = child
        .wait()
        .map_err(|e| format!("等待提权卸载结束失败: {}", e))?;

    if !status.success() {
        let code = status.code().unwrap_or(-1);
        if code == 3010 {
            eprintln!("[viap][uninstall] 提权卸载需要重启，退出码: 3010");
        } else {
            return Err(format!("提权执行卸载失败，退出码: {}", code));
        }
    }

    Ok(())
}

#[cfg(windows)]
fn escape_ps_single_quoted(value: &str) -> String {
    value.replace("'", "''")
}

#[cfg(windows)]
fn spawn_and_wait(program: &str, args: &[String], working_dir: Option<&Path>) -> Result<(), String> {
    let mut command = Command::new(program);
    command.args(args);
    if let Some(dir) = working_dir {
        command.current_dir(dir);
    }

    let mut child = command
        .spawn()
        .map_err(|e| format!("启动卸载程序失败: {}", e))?;

    // 关键变更：等待子进程结束，确保后续残留扫描在卸载完成后执行
    let status = child
        .wait()
        .map_err(|e| format!("等待卸载程序结束失败: {}", e))?;

    if !status.success() {
        let exit_code = status.code().unwrap_or(-1);
        eprintln!(
            "[viap][uninstall] 进程退出: program={} code={} args={}",
            program,
            exit_code,
            args.join(" ")
        );

        // 退出码 3010 = 需要重启完成卸载，视为成功但记录日志
        if exit_code == 3010 {
            eprintln!("[viap][uninstall] 卸载需要重启，退出码: 3010");
            return Ok(());
        }
        return Err(format!("卸载程序执行失败，退出码: {}", exit_code));
    }

    Ok(())
}

#[cfg(windows)]
fn derive_working_dir(program: &str) -> Option<PathBuf> {
    let path = Path::new(program);
    if path.exists() {
        return path.parent().map(|p| p.to_path_buf());
    }
    None
}

#[cfg(windows)]
fn parse_program_and_args(command: &str) -> Option<(String, Vec<String>)> {
    let cmd = command.trim();
    if cmd.is_empty() {
        return None;
    }

    // 引号包裹的路径直接提取引号内容作为程序路径
    if let Some(rest) = cmd.strip_prefix('"') {
        let end = rest.find('"')?;
        let program = rest[..end].trim().to_string();
        let args_raw = rest[end + 1..].trim();
        return Some((program, split_command_args(args_raw)));
    }

    // 无引号命令：不能简单按第一个空格拆分，因为路径可能含空格
    // 例如 C:\Program Files (x86)\App\uninst.exe /S
    // 从最长可能前缀开始递减试探，找到第一个实际存在的文件/目录作为程序路径
    // 若都不存在则回退到原始简单拆分（兼容 msiexec /X{GUID} 等 PATH 命令）
    let tokens: Vec<&str> = cmd.split_whitespace().collect();
    for i in (1..=tokens.len()).rev() {
        let candidate = tokens[..i].join(" ");
        if Path::new(&candidate).exists() {
            let args = if i < tokens.len() {
                tokens[i..].iter().map(|s| s.to_string()).collect()
            } else {
                Vec::new()
            };
            return Some((candidate, args));
        }
    }

    // 回退：没有任何拼接路径存在时，取第一个 token 作为程序名
    let program = tokens[0].to_string();
    let args = if tokens.len() > 1 {
        tokens[1..].iter().map(|s| s.to_string()).collect()
    } else {
        Vec::new()
    };
    Some((program, args))
}

#[cfg(windows)]
fn split_command_args(input: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;

    for ch in input.chars() {
        match ch {
            '"' => in_quotes = !in_quotes,
            ' ' | '\t' if !in_quotes => {
                if !current.is_empty() {
                    args.push(current.clone());
                    current.clear();
                }
            }
            _ => current.push(ch),
        }
    }

    if !current.is_empty() {
        args.push(current);
    }

    args
}

#[cfg(windows)]
fn is_definitely_invalid_program(program: &str) -> bool {
    let p = program.trim().trim_matches('"').trim();
    p.is_empty() || p == "\\" || p == "\\\\" || p == "/"
}

// ============================================================================
// 残留扫描逻辑（与卸载逻辑解耦）
// ============================================================================

#[cfg(windows)]
fn scan_app_residue_internal(
    app_name: String,
    publisher: Option<String>,
    install_location: Option<String>,
) -> Result<Vec<LeftoverItem>, String> {
    let Some(context) = build_strict_scan_context(&app_name, publisher.as_deref(), install_location.as_deref()) else {
        return Ok(Vec::new());
    };

    let mut roots = build_scan_roots(&app_name, install_location);
    roots.sort();
    roots.dedup();

    let mut items = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    scan_filesystem_residue(&roots, &context, &mut items, &mut seen);
    scan_registry_residue(&context, &mut items, &mut seen);

    Ok(items)
}

#[cfg(windows)]
fn build_keywords(app_name: &str, publisher: Option<&str>, install_location: Option<&str>) -> Vec<String> {
    let mut values = vec![sanitize_search_text(app_name)];
    if let Some(pub_name) = publisher {
        values.push(sanitize_search_text(pub_name));
    }

    if let Some(location) = install_location {
        values.push(sanitize_search_text(location));
    }

    let mut keywords = Vec::new();
    for raw in values {
        if raw.is_empty() {
            continue;
        }
        for part in raw.split(|c: char| !c.is_alphanumeric()) {
            let token = part.trim().to_lowercase();
            if is_meaningful_keyword(&token) && !keywords.contains(&token) {
                keywords.push(token);
            }
        }
    }
    keywords
}

#[cfg(windows)]
fn is_meaningful_keyword(token: &str) -> bool {
    if token.is_empty() {
        return false;
    }

    let generic_ascii_words = [
        "app", "apps", "group", "company", "co", "ltd", "inc", "corp", "corporation", "limited", "tech",
        "technology", "software", "network", "internet", "china", "beijing", "shanghai", "windows", "microsoft",
    ];
    let generic_cn_words = ["公司", "集团", "技术", "网络", "软件", "中国", "有限", "科技"];

    if generic_ascii_words.iter().any(|w| *w == token) {
        return false;
    }

    if generic_cn_words.iter().any(|w| *w == token) {
        return false;
    }

    if token.chars().all(|ch| ch.is_ascii_alphanumeric()) {
        return token.len() >= 3;
    }

    token.chars().count() >= 2
}

#[cfg(windows)]
fn build_scan_roots(app_name: &str, install_location: Option<String>) -> Vec<String> {
    let mut roots = vec![
        std::env::var("APPDATA").unwrap_or_default(),
        std::env::var("LOCALAPPDATA").unwrap_or_default(),
        r"C:\ProgramData".to_string(),
    ];

    if let Some(path) = install_location {
        if !path.trim().is_empty() {
            roots.push(path);
        }
    }

    for path in find_install_locations_by_app_name(app_name) {
        roots.push(path);
    }

    roots.into_iter().filter(|p| !p.trim().is_empty()).collect()
}

#[cfg(windows)]
fn scan_filesystem_residue(
    roots: &[String],
    context: &StrictScanContext,
    output: &mut Vec<LeftoverItem>,
    seen: &mut HashSet<String>,
) {
    for root in roots {
        let root_path = Path::new(root);
        if !root_path.exists() || !root_path.is_dir() || is_blacklisted_path(root_path) {
            continue;
        }

        for entry in WalkDir::new(root_path)
            .max_depth(5)
            .into_iter()
            .filter_map(|entry| entry.ok())
        {
            if entry.depth() == 0 {
                continue;
            }

            let path = entry.path();
            if is_blacklisted_path(path) {
                continue;
            }

            if !matches_strict_leftover_path(path, context) {
                continue;
            }

            let canonical = normalize_path(path);
            if seen.contains(&canonical) {
                continue;
            }

            seen.insert(canonical);

            let item_type = if path.is_dir() { "Folder" } else { "File" };
            let size_mb = if path.is_dir() {
                bytes_to_mb(compute_dir_size(path))
            } else {
                bytes_to_mb(path.metadata().map(|m| m.len()).unwrap_or(0))
            };

            output.push(LeftoverItem {
                path: path.to_string_lossy().to_string(),
                item_type: item_type.to_string(),
                size_mb,
                selected: true,
            });
        }
    }
}

#[cfg(windows)]
fn scan_registry_residue(
    context: &StrictScanContext,
    output: &mut Vec<LeftoverItem>,
    seen: &mut HashSet<String>,
) {
    let registry_roots: [(HKEY, &str, &str); 3] = [
        (HKEY_LOCAL_MACHINE, "HKLM", r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall"),
        (HKEY_LOCAL_MACHINE, "HKLM", r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall"),
        (HKEY_CURRENT_USER, "HKCU", r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall"),
    ];

    for (hkey, hive_name, root_path) in registry_roots {
        let uninstall_root = match RegKey::predef(hkey).open_subkey_with_flags(root_path, KEY_READ) {
            Ok(v) => v,
            Err(_) => continue,
        };

        for subkey_name in uninstall_root.enum_keys().filter_map(|x| x.ok()) {
            let full_sub_path = format!(r"{}\{}", root_path, subkey_name);
            let subkey = match RegKey::predef(hkey).open_subkey_with_flags(&full_sub_path, KEY_READ) {
                Ok(v) => v,
                Err(_) => continue,
            };

            if !matches_registry_key_strict(&subkey, context) {
                continue;
            }

            let registry_path = format!(r"{}\{}", hive_name, full_sub_path);
            let canonical = registry_path.to_lowercase();
            if seen.contains(&canonical) {
                continue;
            }

            seen.insert(canonical);
            output.push(LeftoverItem {
                path: registry_path,
                item_type: "Registry".to_string(),
                size_mb: 0.0,
                selected: true,
            });
        }
    }

    // 扩展扫描：发布商路径（Software\<Publisher>）
    scan_publisher_registry_residue(context, output, seen);

    // 扩展扫描：文件关联（Software\Classes\Applications\<app_name>）
    scan_classes_registry_residue(context, output, seen);
}

/// 扫描发布商注册表路径残留
/// Geek 等专业卸载器会扫描这些路径，大量应用残留存在于 Software\<Publisher> 下
#[cfg(windows)]
fn scan_publisher_registry_residue(
    context: &StrictScanContext,
    output: &mut Vec<LeftoverItem>,
    seen: &mut HashSet<String>,
) {
    let publisher = match context.publisher_name.as_ref() {
        Some(p) => p.clone(),
        None => return,
    };
    if publisher.is_empty() {
        return;
    }

    let publisher_roots: [(HKEY, &str, &str); 4] = [
        (HKEY_LOCAL_MACHINE, "HKLM", r"SOFTWARE"),
        (HKEY_LOCAL_MACHINE, "HKLM", r"SOFTWARE\WOW6432Node"),
        (HKEY_CURRENT_USER, "HKCU", r"SOFTWARE"),
        (HKEY_CURRENT_USER, "HKCU", r"SOFTWARE\WOW6432Node"),
    ];

    for (hkey, hive_name, root_path) in publisher_roots {
        let _root = match RegKey::predef(hkey).open_subkey_with_flags(root_path, KEY_READ) {
            Ok(v) => v,
            Err(_) => continue,
        };

        // 尝试打开 Software\<Publisher>
        let publisher_path = format!(r"{}\{}", root_path, publisher);
        let publisher_key = match RegKey::predef(hkey).open_subkey_with_flags(&publisher_path, KEY_READ) {
            Ok(v) => v,
            Err(_) => continue,
        };

        // 扫描发布商下的子键（如 <AppName>、<Version>）
        for subkey_name in publisher_key.enum_keys().filter_map(|x| x.ok()) {
            let full_path = format!(r"{}\{}", publisher_path, subkey_name);
            let registry_path = format!(r"{}\{}", hive_name, full_path);
            let canonical = registry_path.to_lowercase();
            if seen.contains(&canonical) {
                continue;
            }

            // 匹配检查：子键名是否与应用名或安装路径相关
            let subkey_lower = subkey_name.to_lowercase();
            let matches_app = subkey_lower.contains(&context.app_folder_name)
                || context.app_folder_name.contains(&subkey_lower);
            let matches_install = context
                .install_location
                .as_ref()
                .map(|loc| {
                    loc.split('\\')
                        .last()
                        .map(|last| subkey_lower.contains(last) || last.to_lowercase().contains(&subkey_lower))
                        .unwrap_or(false)
                })
                .unwrap_or(false);

            if !matches_app && !matches_install {
                continue;
            }

            // 验证子键可打开（成功则继续，失败则跳过）
            if RegKey::predef(hkey).open_subkey_with_flags(&full_path, KEY_READ).is_err() {
                continue;
            }

            // 安全校验
            if !is_safe_registry_cleanup_target(hkey, &full_path, &[publisher.clone()]) {
                continue;
            }

            seen.insert(canonical);
            output.push(LeftoverItem {
                path: registry_path,
                item_type: "Registry".to_string(),
                size_mb: 0.0,
                selected: true,
            });
        }
    }
}

/// 扫描 Classes 文件关联残留
/// Windows 应用常在 Software\Classes\Applications\<appname.exe> 注册文件关联
#[cfg(windows)]
fn scan_classes_registry_residue(
    context: &StrictScanContext,
    output: &mut Vec<LeftoverItem>,
    seen: &mut HashSet<String>,
) {
    let classes_roots: [(HKEY, &str, &str); 2] = [
        (HKEY_LOCAL_MACHINE, "HKLM", r"SOFTWARE\Classes\Applications"),
        (HKEY_CURRENT_USER, "HKCU", r"SOFTWARE\Classes\Applications"),
    ];

    for (hkey, hive_name, root_path) in classes_roots {
        let root = match RegKey::predef(hkey).open_subkey_with_flags(root_path, KEY_READ) {
            Ok(v) => v,
            Err(_) => continue,
        };

        for subkey_name in root.enum_keys().filter_map(|x| x.ok()) {
            let subkey_lower = subkey_name.to_lowercase();

            // 匹配：子键包含应用目录名（如 appname.exe）
            if !subkey_lower.contains(&context.app_folder_name) {
                continue;
            }

            let full_path = format!(r"{}\{}", root_path, subkey_name);
            let registry_path = format!(r"{}\{}", hive_name, full_path);
            let canonical = registry_path.to_lowercase();
            if seen.contains(&canonical) {
                continue;
            }

            seen.insert(canonical);
            output.push(LeftoverItem {
                path: registry_path,
                item_type: "Registry".to_string(),
                size_mb: 0.0,
                selected: true,
            });
        }
    }
}

#[cfg(windows)]
fn matches_registry_key_strict(key: &RegKey, context: &StrictScanContext) -> bool {
    let display_name: String = key.get_value("DisplayName").unwrap_or_default();
    let display_name = normalize_match_text(&display_name);
    if !display_name.is_empty() && display_name == context.app_name_exact {
        return true;
    }

    let uninstall_candidates = [
        key.get_value::<String, _>("UninstallString").unwrap_or_default(),
        key.get_value::<String, _>("QuietUninstallString").unwrap_or_default(),
    ];

    uninstall_candidates
        .into_iter()
        .map(|v| normalize_windows_path(&v))
        .filter(|v| !v.is_empty())
        .any(|value| {
            context
                .uninstall_path_hints
                .iter()
                .any(|hint| !hint.is_empty() && value.contains(hint))
        })
}

#[cfg(windows)]
fn matches_strict_leftover_path(path: &Path, context: &StrictScanContext) -> bool {
    let normalized_path = normalize_path(path);
    if BLACKLIST.iter().any(|token| normalized_path.contains(token)) {
        return false;
    }

    if let Some(install_location) = context.install_location.as_ref() {
        if normalized_path == *install_location {
            return true;
        }
    }

    let components: Vec<String> = path
        .components()
        .map(|c| normalize_match_text(&c.as_os_str().to_string_lossy()))
        .filter(|v| !v.is_empty())
        .collect();

    if components.is_empty() {
        return false;
    }

    let app_indexes: Vec<usize> = components
        .iter()
        .enumerate()
        .filter_map(|(idx, value)| if *value == context.app_folder_name { Some(idx) } else { None })
        .collect();

    if app_indexes.is_empty() {
        return false;
    }

    if let Some(publisher) = context.publisher_name.as_ref() {
        if normalized_path.contains(publisher) {
            let publisher_index = components
                .iter()
                .position(|value| value.contains(publisher) || publisher.contains(value));

            // Rule B: 命中发布商目录时，必须在更深层出现精确应用目录名
            match publisher_index {
                Some(pub_idx) if app_indexes.iter().any(|app_idx| *app_idx > pub_idx) => return true,
                Some(_) => return false,
                None => return false,
            }
        }
    }

    // Rule A: 任意层出现精确应用目录名，才视为残留
    true
}

#[cfg(windows)]
fn collect_uninstall_path_hints(app_name_exact: &str, install_location: Option<&str>) -> Vec<String> {
    let mut hints: Vec<String> = Vec::new();

    if let Some(location) = install_location {
        let normalized = normalize_windows_path(location);
        if !normalized.is_empty() {
            hints.push(normalized);
        }
    }

    let registry_roots: [(HKEY, &str); 3] = [
        (HKEY_LOCAL_MACHINE, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall"),
        (HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall"),
        (HKEY_CURRENT_USER, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall"),
    ];

    for (hkey, path) in registry_roots {
        let uninstall_key = match RegKey::predef(hkey).open_subkey_with_flags(path, KEY_READ) {
            Ok(v) => v,
            Err(_) => continue,
        };

        for subkey_name in uninstall_key.enum_keys().filter_map(|x| x.ok()) {
            let subkey = match uninstall_key.open_subkey_with_flags(&subkey_name, KEY_READ) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let display_name: String = subkey.get_value("DisplayName").unwrap_or_default();
            if normalize_match_text(&display_name) != app_name_exact {
                continue;
            }

            for command in [
                subkey.get_value::<String, _>("UninstallString").unwrap_or_default(),
                subkey.get_value::<String, _>("QuietUninstallString").unwrap_or_default(),
            ] {
                if let Some(path_hint) = extract_uninstall_path_hint(&command) {
                    hints.push(path_hint);
                }
            }
        }
    }

    hints.sort();
    hints.dedup();
    hints
}

#[cfg(windows)]
fn extract_uninstall_path_hint(command: &str) -> Option<String> {
    let (program, _) = parse_program_and_args(command)?;
    let normalized = normalize_windows_path(&program);
    if normalized.is_empty() {
        return None;
    }

    // MSI GUID 这类非路径命令不作为路径证据
    if normalized.contains("msiexec") && !normalized.contains('\\') {
        return None;
    }

    Some(normalized)
}

#[cfg(windows)]
fn find_install_locations_by_app_name(app_name: &str) -> Vec<String> {
    let query = normalize_match_text(app_name);
    if query.is_empty() {
        return Vec::new();
    }

    let mut paths = Vec::new();
    let registry_roots: [(HKEY, &str); 3] = [
        (HKEY_LOCAL_MACHINE, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall"),
        (HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall"),
        (HKEY_CURRENT_USER, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall"),
    ];

    for (hkey, path) in registry_roots {
        let uninstall_key = match RegKey::predef(hkey).open_subkey(path) {
            Ok(k) => k,
            Err(_) => continue,
        };

        for subkey_name in uninstall_key.enum_keys().filter_map(|x| x.ok()) {
            let subkey = match uninstall_key.open_subkey(&subkey_name) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let display_name: String = subkey.get_value("DisplayName").unwrap_or_default();
            if normalize_match_text(&display_name) != query {
                continue;
            }

            let location: String = subkey.get_value("InstallLocation").unwrap_or_default();
            let normalized = sanitize_search_text(&location);
            if !normalized.is_empty() {
                paths.push(normalized);
            }
        }
    }

    paths
}

#[cfg(windows)]
fn compute_dir_size(path: &Path) -> u64 {
    WalkDir::new(path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter_map(|entry| entry.metadata().ok())
        .filter(|metadata| metadata.is_file())
        .map(|metadata| metadata.len())
        .sum()
}

#[cfg(windows)]
fn bytes_to_mb(bytes: u64) -> f64 {
    ((bytes as f64) / 1024.0 / 1024.0 * 100.0).round() / 100.0
}

#[cfg(windows)]
fn normalize_path(path: &Path) -> String {
    path.to_string_lossy().to_lowercase()
}

#[cfg(windows)]
fn is_blacklisted_path(path: &Path) -> bool {
    let normalized = normalize_path(path);

    if normalized == r"c:\windows"
        || normalized.starts_with(r"c:\windows\")
        || normalized == r"c:\windows\system32"
        || normalized.starts_with(r"c:\windows\system32\")
        || normalized == r"c:\program files"
        || normalized == r"c:\program files (x86)"
    {
        return true;
    }

    if BLACKLIST.iter().any(|token| normalized.contains(token)) {
        return true;
    }

    // 防止删除盘符根目录
    if path.parent().is_none() {
        return true;
    }

    false
}

/// 强制删除文件或目录
///
/// 三级回退策略：
/// 1. 直接调用 std::fs 删除（大多数场景）
/// 2. 递归清除只读属性后重试（覆盖 Read-only / 部分安装器设置的保护属性）
/// 3. 调用 Windows 的 takeown / icacls 夺回所有权与完全控制权限后再次重试
///    —— 覆盖 "Access Denied / 拒绝访问" 场景
#[cfg(windows)]
fn force_delete_path(path: &Path) -> Result<(), String> {
    // 第 1 步：直接尝试
    if try_remove(path).is_ok() {
        return Ok(());
    }

    // 第 2 步：清除只读属性后重试
    let _ = clear_readonly_recursively(path);
    if let Err(err) = try_remove(path) {
        // 第 3 步：夺权 + 授权 + 重试
        let path_str = path.to_string_lossy().to_string();
        if path.is_dir() {
            let _ = run_silent("takeown", &["/F", &path_str, "/R", "/D", "Y"]);
            // S-1-5-32-544 = BUILTIN\Administrators（避免本地化差异）
            let _ = run_silent(
                "icacls",
                &[&path_str, "/grant", "*S-1-5-32-544:F", "/T", "/C", "/Q"],
            );
        } else {
            let _ = run_silent("takeown", &["/F", &path_str]);
            let _ = run_silent(
                "icacls",
                &[&path_str, "/grant", "*S-1-5-32-544:F", "/C", "/Q"],
            );
        }
        let _ = clear_readonly_recursively(path);

        if let Err(final_err) = try_remove(path) {
            return Err(format!(
                "删除失败：{}；权限回退后仍失败：{}",
                err, final_err
            ));
        }
    }

    Ok(())
}

#[cfg(windows)]
fn try_remove(path: &Path) -> Result<(), String> {
    let result = if path.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    result.map_err(|e| e.to_string())
}

/// 递归清除只读属性；单文件也适用
#[cfg(windows)]
fn clear_readonly_recursively(path: &Path) -> Result<(), String> {
    if path.is_file() {
        return clear_readonly_single(path);
    }
    if path.is_dir() {
        for entry in WalkDir::new(path).into_iter().flatten() {
            let _ = clear_readonly_single(entry.path());
        }
        // 目录本身也清理一次（deny 属性常挂在目录上）
        let _ = clear_readonly_single(path);
    }
    Ok(())
}

#[cfg(windows)]
fn clear_readonly_single(path: &Path) -> Result<(), String> {
    let meta = fs::metadata(path).map_err(|e| e.to_string())?;
    let mut perm = meta.permissions();
    if perm.readonly() {
        perm.set_readonly(false);
        fs::set_permissions(path, perm).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 在无窗口的情况下执行一个命令行工具，忽略返回码，仅用于权限回退
#[cfg(windows)]
fn run_silent(program: &str, args: &[&str]) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    Command::new(program)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(windows)]
fn is_blacklisted_registry_path(_hkey: HKEY, sub_path: &str) -> bool {
    let normalized = sub_path.trim().to_lowercase();

    // 防止删除根级 Software 节点
    if normalized == "software" {
        return true;
    }

    // 防止误删系统核心注册表路径
    if normalized == r"software\microsoft" || normalized.starts_with(r"software\microsoft\windows") {
        return true;
    }

    false
}

// ============================================================================
// 注册表/命令辅助函数
// ============================================================================

#[cfg(windows)]
fn read_uninstall_commands_from_registry_path(path: &str) -> Vec<String> {
    let (hkey, sub_path) = match parse_registry_path(path) {
        Some(v) => v,
        None => {
            eprintln!("[viap][uninstall] 无法解析注册表路径: {}", path);
            return Vec::new();
        }
    };
    let key = match RegKey::predef(hkey).open_subkey_with_flags(sub_path, KEY_READ) {
        Ok(v) => v,
        Err(e) => {
            eprintln!(
                "[viap][uninstall] 打开注册表键失败: hive={:?} path={} error={}",
                hkey, sub_path, e
            );
            return Vec::new();
        }
    };

    let mut cmds = Vec::new();

    let quiet: String = key.get_value("QuietUninstallString").unwrap_or_default();
    if is_valid_uninstall_command(&quiet) {
        cmds.push(quiet.trim().to_string());
    }

    let normal: String = key.get_value("UninstallString").unwrap_or_default();
    if is_valid_uninstall_command(&normal) {
        let normalized = normal.trim().to_string();
        if !cmds.iter().any(|v| v.eq_ignore_ascii_case(&normalized)) {
            cmds.push(normalized);
        }
    }

    if cmds.is_empty() {
        eprintln!(
            "[viap][uninstall] 注册表键存在但无有效卸载命令: path={}",
            sub_path
        );
    }

    cmds
}

#[cfg(windows)]
fn find_uninstall_by_display_name(app_id: &str) -> Option<String> {
    find_uninstall_commands_by_display_name(app_id).into_iter().next()
}

#[cfg(windows)]
fn find_uninstall_commands_by_display_name(app_id: &str) -> Vec<String> {
    let query = app_id.trim().to_lowercase();
    if query.is_empty() {
        return Vec::new();
    }

    // 遍历四棵 Uninstall 注册表根（含 HKCU 32 位视图）
    let registry_roots: [(HKEY, &str); 4] = [
        (HKEY_LOCAL_MACHINE, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall"),
        (HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall"),
        (HKEY_CURRENT_USER, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall"),
        (HKEY_CURRENT_USER, r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall"),
    ];

    for (hkey, path) in registry_roots {
        let uninstall_key = match RegKey::predef(hkey).open_subkey_with_flags(path, KEY_READ) {
            Ok(k) => k,
            Err(_) => continue,
        };

        for subkey_name in uninstall_key.enum_keys().filter_map(|x| x.ok()) {
            let subkey = match uninstall_key.open_subkey_with_flags(&subkey_name, KEY_READ) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let display_name: String = subkey.get_value("DisplayName").unwrap_or_default();
            if display_name.trim().to_lowercase() != query {
                continue;
            }

            let mut cmds = Vec::new();

            let quiet: String = subkey.get_value("QuietUninstallString").unwrap_or_default();
            if is_valid_uninstall_command(&quiet) {
                cmds.push(quiet.trim().to_string());
            }

            let normal: String = subkey.get_value("UninstallString").unwrap_or_default();
            if is_valid_uninstall_command(&normal) {
                let normalized = normal.trim().to_string();
                if !cmds.iter().any(|v| v.eq_ignore_ascii_case(&normalized)) {
                    cmds.push(normalized);
                }
            }

            if !cmds.is_empty() {
                return cmds;
            }
        }
    }

    Vec::new()
}

/// 按 InstallLocation 回退搜索卸载命令
/// 当 DisplayName 精确匹配失败时调用，用于处理文件系统扫描到的应用
/// （此时 display_name 来自目录名，与注册表 DisplayName 可能不完全一致）
#[cfg(windows)]
fn find_uninstall_commands_by_install_location(install_location: &str) -> Vec<String> {
    let target = install_location.trim().to_lowercase();
    if target.is_empty() {
        return Vec::new();
    }
    // 归一化：统一去除尾部反斜杠，确保路径比较不受格式差异影响
    let normalized = target.trim_end_matches('\\').to_string();

    let registry_roots: [(HKEY, &str); 4] = [
        (HKEY_LOCAL_MACHINE, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall"),
        (HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall"),
        (HKEY_CURRENT_USER, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall"),
        (HKEY_CURRENT_USER, r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall"),
    ];

    for (hkey, path) in registry_roots {
        let uninstall_key = match RegKey::predef(hkey).open_subkey_with_flags(path, KEY_READ) {
            Ok(k) => k,
            Err(_) => continue,
        };

        for subkey_name in uninstall_key.enum_keys().filter_map(|x| x.ok()) {
            let subkey = match uninstall_key.open_subkey_with_flags(&subkey_name, KEY_READ) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let loc: String = subkey.get_value("InstallLocation").unwrap_or_default();
            if loc.trim().to_lowercase().trim_end_matches('\\') != normalized {
                continue;
            }

            let mut cmds = Vec::new();

            let quiet: String = subkey.get_value("QuietUninstallString").unwrap_or_default();
            if is_valid_uninstall_command(&quiet) {
                cmds.push(quiet.trim().to_string());
            }

            let normal: String = subkey.get_value("UninstallString").unwrap_or_default();
            if is_valid_uninstall_command(&normal) {
                let normalized_cmd = normal.trim().to_string();
                if !cmds.iter().any(|v| v.eq_ignore_ascii_case(&normalized_cmd)) {
                    cmds.push(normalized_cmd);
                }
            }

            if !cmds.is_empty() {
                return cmds;
            }
        }
    }

    Vec::new()
}

/// 在安装目录中扫描卸载器可执行文件（最终兜底策略）
/// 匹配文件名含 "unin"、"uninstall"、"卸载" 的 .exe 文件
/// 扫描范围：安装目录顶层 + 一层子目录
#[cfg(windows)]
fn scan_uninstaller_in_directory(dir: &str) -> Option<String> {
    let install_dir = Path::new(dir);
    if !install_dir.is_dir() {
        return None;
    }
    // 判断文件名是否匹配卸载器特征
    fn looks_like_uninstaller(name: &str) -> bool {
        let lower = name.to_lowercase();
        lower.contains("unin") || lower.contains("uninstall") || lower.contains("卸载")
    }

    // 扫描指定目录，返回第一个匹配的 .exe 的完整路径
    fn scan_single_dir(dir_path: &Path) -> Option<String> {
        let entries = match std::fs::read_dir(dir_path) {
            Ok(e) => e,
            Err(_) => return None,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "exe").unwrap_or(false) {
                if let Some(name) = path.file_stem().and_then(|n| n.to_str()) {
                    if looks_like_uninstaller(name) {
                        return Some(path.to_string_lossy().to_string());
                    }
                }
            }
        }
        None
    }

    // 1) 安装目录顶层
    if let Some(found) = scan_single_dir(install_dir) {
        return Some(found);
    }

    // 2) 安装目录下一层子目录
    let sub_entries = match std::fs::read_dir(install_dir) {
        Ok(e) => e,
        Err(_) => return None,
    };
    for sub in sub_entries.flatten() {
        let Ok(ft) = sub.file_type() else { continue };
        if !ft.is_dir() {
            continue;
        }
        if let Some(found) = scan_single_dir(&sub.path()) {
            return Some(found);
        }
    }

    None
}

#[cfg(windows)]
fn parse_registry_path(path: &str) -> Option<(HKEY, &str)> {
    if let Some(rest) = path.strip_prefix("HKLM\\") {
        return Some((HKEY_LOCAL_MACHINE, rest));
    }
    if let Some(rest) = path.strip_prefix("HKEY_LOCAL_MACHINE\\") {
        return Some((HKEY_LOCAL_MACHINE, rest));
    }
    if let Some(rest) = path.strip_prefix("HKCU\\") {
        return Some((HKEY_CURRENT_USER, rest));
    }
    if let Some(rest) = path.strip_prefix("HKEY_CURRENT_USER\\") {
        return Some((HKEY_CURRENT_USER, rest));
    }
    None
}

#[cfg(windows)]
fn is_valid_uninstall_command(command: &str) -> bool {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return false;
    }

    let normalized = trimmed.trim_matches('"').trim();
    if normalized.is_empty() {
        return false;
    }

    normalized != "\\" && normalized != "\\\\" && normalized != "/"
}

/// 从注册表键读取安装路径，含 DisplayIcon / UninstallString 回退推导
/// 与 scanner::derive_install_location_from_icon 逻辑一致，确保
/// 即使注册表 InstallLocation 为空也能找到实际安装目录
#[cfg(windows)]
fn read_install_location_with_fallback(key: &RegKey) -> Option<String> {
    // 1) 直接读取 InstallLocation
    let loc: String = key.get_value("InstallLocation").unwrap_or_default();
    let sanitized = sanitize_search_text(&loc);
    if !sanitized.is_empty() {
        return Some(sanitized);
    }

    // 2) 尝试从 DisplayIcon 推导
    let display_icon: String = key.get_value("DisplayIcon").unwrap_or_default();
    if !display_icon.is_empty() {
        if let Some(dir) = derive_install_location_from_icon(&display_icon) {
            return Some(dir);
        }
    }

    // 3) 尝试从 UninstallString 推导
    let uninstall_string: String = key.get_value("UninstallString").unwrap_or_default();
    if !uninstall_string.is_empty() {
        if let Some(dir) = derive_install_location_from_icon(&uninstall_string) {
            return Some(dir);
        }
    }

    None
}

/// 从 DisplayIcon / UninstallString 字段尝试推导安装目录
/// 形式如 "C:\path\app.exe,0" 或 "\"C:\path\uninst.exe\" /S"
#[cfg(windows)]
fn derive_install_location_from_icon(icon_or_uninstall: &str) -> Option<String> {
    let raw = icon_or_uninstall.trim();
    if raw.is_empty() {
        return None;
    }

    // 1) 先按逗号分割去掉 ",索引" 后缀（如 "C:\app.exe,0"）
    let (before_comma, _) = raw.split_once(',').unwrap_or((raw, ""));
    let before_comma = before_comma.trim();

    // 2) 提取实际存在的路径
    let path_str = find_existing_path_fragment(before_comma)?;
    let p = Path::new(&path_str);

    // 若候选路径是文件，返回其父目录；若是目录，直接使用
    let dir = if p.is_file() {
        p.parent()?.to_path_buf()
    } else {
        p.to_path_buf()
    };

    // 过滤掉系统/无意义目录
    let lower = dir.to_string_lossy().to_lowercase();
    if lower.contains("\\windows\\system32")
        || lower.contains("\\windows\\syswow64")
        || lower.contains("\\common files\\")
    {
        return None;
    }

    Some(dir.to_string_lossy().to_string())
}

/// 从可能含空格且无引号的字符串中提取第一个实际存在的文件/目录路径
/// 例如 "C:\Program Files\App\app.exe" → 逐词拼接试探直到找到存在路径
/// 如果已用引号包裹，直接提取引号内容
#[cfg(windows)]
fn find_existing_path_fragment(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    // 引号包裹：直接提取引号内容
    if trimmed.starts_with('"') {
        let rest = &trimmed[1..];
        let end = rest.find('"')?;
        return Some(rest[..end].to_string());
    }

    // 无引号：从最长前缀递减试探，找到第一个存在的文件/目录
    // 处理 C:\Program Files\App\app.exe 这类含空格的路径
    let tokens: Vec<&str> = trimmed.split_whitespace().collect();
    for i in (1..=tokens.len()).rev() {
        let candidate = tokens[..i].join(" ");
        if Path::new(&candidate).exists() {
            return Some(candidate);
        }
    }

    None
}
