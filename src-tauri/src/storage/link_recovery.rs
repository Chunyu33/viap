// 迁移记录重建模块（原路径链接识别）
//
// 背景：数据目录被误删会同时丢失 migration_history.json，但原路径上的目录联接仍然
// 存在。此时界面无法识别「已迁移」，也无法提供恢复入口。
//
// 思路：由用户手动选择一个**原路径侧**目录，扫描其中的目录联接，反推并重建迁移记录。
// 联接本身不携带「由谁创建」的元数据，因此本模块只产出**候选**（含置信度与原因），
// 必须由用户确认后才写入历史，绝不静默导入。
//
// 关键不变量：migrate_app 的 target_path = target_parent\源目录名，
// 因此正常迁移联接的「目标末级名」必然等于「原目录末级名」，这是主要识别依据。

use std::collections::{HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Instant, UNIX_EPOCH};

use serde::Serialize;
use tauri::Emitter;

use crate::models::*;
use crate::storage::data_dir::{ensure_data_dir, load_custom_folders, save_custom_folders};
use crate::storage::history;
use crate::storage::migrated_app_metadata;
use crate::utils;

/// 单次扫描的目录数上限：误选盘符根目录时避免长时间占用磁盘 IO
const MAX_SCANNED_DIRS: u32 = 20_000;
/// 默认扫描深度：覆盖 C:\Users\<用户>\AppData\Local\Programs\<应用> 这一常见层级
const DEFAULT_SCAN_DEPTH: u32 = 4;
const MAX_SCAN_DEPTH: u32 = 8;
/// 单次导入条目上限，防止异常请求写入超大历史
const MAX_IMPORT_ENTRIES: usize = 2_000;
/// 进度事件推送间隔（按已扫描目录数计），避免事件风暴
const PROGRESS_INTERVAL: u32 = 64;

/// 链接识别进度事件（与 scan-progress / migration-progress 隔离，避免互相干扰）
#[derive(Clone, Serialize)]
pub struct LinkRecoveryProgressEvent {
    pub scanned_dirs: u32,
    pub found_links: u32,
    pub current_path: String,
}

/// 不进入内部的目录名：系统目录与包管理器产物目录
///
/// node_modules 下存在大量 pnpm/yarn 创建的联接，递归进去只会产生噪声候选。
const SKIP_DIR_NAMES: &[&str] = &[
    "$recycle.bin",
    "system volume information",
    "windows",
    "windows.old",
    "$windows.~bt",
    "$windows.~ws",
    "recovery",
    "config.msi",
    "node_modules",
    ".git",
];

/// Windows 为兼容旧程序在用户配置目录内创建的系统联接名称
const SYSTEM_COMPAT_LINK_NAMES: &[&str] = &[
    "application data",
    "local settings",
    "my documents",
    "my music",
    "my pictures",
    "my videos",
    "cookies",
    "history",
    "temporary internet files",
    "nethood",
    "printhood",
    "recent",
    "sendto",
    "templates",
    "「开始」菜单",
    "start menu",
    "documents and settings",
    "all users",
    "default user",
];

// ============================================================================
// Tauri 命令
// ============================================================================

/// 扫描进度回调（与 Tauri 解耦，便于单测直接驱动扫描逻辑）
type ProgressCallback<'a> = &'a dyn Fn(u32, u32, &Path);

/// 扫描指定目录下的目录联接并产出迁移记录候选（阻塞 IO 移入线程池）
#[tauri::command]
pub async fn scan_migration_links(
    root_path: String,
    max_depth: Option<u32>,
    record_size: Option<bool>,
    state: tauri::State<'_, LinkRecoveryState>,
    app_handle: tauri::AppHandle,
) -> Result<LinkRecoveryScanResult, String> {
    state.cancel_flag.store(false, Ordering::SeqCst);
    let cancel_flag = state.cancel_flag.clone();
    let record_size = record_size.unwrap_or(false);

    tauri::async_runtime::spawn_blocking(move || {
        let progress = move |scanned_dirs: u32, found_links: u32, current_path: &Path| {
            emit_scan_progress(&app_handle, scanned_dirs, found_links, current_path);
        };
        scan_links_blocking(&root_path, max_depth, record_size, &progress, &cancel_flag)
    })
    .await
    .map_err(|error| format!("链接识别线程异常: {}", error))?
}

/// 取消当前链接识别扫描
#[tauri::command]
pub fn cancel_link_recovery(state: tauri::State<'_, LinkRecoveryState>) -> Result<(), String> {
    state.cancel_flag.store(true, Ordering::SeqCst);
    Ok(())
}

/// 导入用户确认后的识别结果，重建迁移记录
#[tauri::command]
pub fn import_recovered_links(
    entries: Vec<RecoveredLinkImport>,
) -> Result<LinkRecoveryImportResult, String> {
    if entries.is_empty() {
        return Err("没有选择任何条目".to_string());
    }
    if entries.len() > MAX_IMPORT_ENTRIES {
        return Err(format!("单次最多导入 {} 条记录", MAX_IMPORT_ENTRIES));
    }

    let mut result = LinkRecoveryImportResult::default();
    let mut accepted: Vec<RecoveredLinkImport> = Vec::new();

    // 前端结果不可信：写入前必须逐条按当前文件系统状态重新校验
    for entry in entries {
        match validate_import_entry(&entry) {
            Ok(()) => accepted.push(entry),
            Err(reason) => {
                result.rejected += 1;
                result.failed.push(format!("{}：{}", entry.original_path, reason));
            }
        }
    }

    if accepted.is_empty() {
        return Ok(result);
    }

    // 已存在同原路径活跃记录的条目会被跳过，保证重复恢复不会写入重复记录
    let (added, duplicated) = history::add_recovered_records(&accepted)?;
    result.imported = added;
    result.duplicated = duplicated;

    // App 类型补写兜底元数据：扫描器遗漏时应用列表仍能显示为已迁移
    for entry in accepted.iter().filter(|entry| entry.record_type == MigrationRecordType::App) {
        migrated_app_metadata::add_migrated_app(
            &entry.app_name,
            &entry.original_path,
            &entry.target_path,
        );
    }

    // 自定义文件夹登记失败不应否定已写入的迁移记录，只作为提示返回
    match register_custom_folders(&accepted) {
        Ok(count) => result.custom_folders_added = count,
        Err(reason) => {
            log_warn!("link_recovery", "登记自定义文件夹失败: {}", reason);
            result.failed.push(format!("迁移记录已写入，但自定义文件夹登记失败：{}", reason));
        }
    }

    // 「已迁移」角标来自历史记录，失效应用列表缓存让界面下次加载重新计算
    crate::app_manager::cache::invalidate();
    Ok(result)
}

// ============================================================================
// 扫描实现
// ============================================================================

fn scan_links_blocking(
    root_path: &str,
    max_depth: Option<u32>,
    record_size: bool,
    progress: ProgressCallback<'_>,
    cancel_flag: &Arc<AtomicBool>,
) -> Result<LinkRecoveryScanResult, String> {
    let started = Instant::now();
    let root = PathBuf::from(root_path);

    if !root.exists() {
        return Err(format!("路径不存在：{}", root_path));
    }
    if !root.is_dir() {
        return Err("请选择文件夹：目录联接建在原路径一侧".to_string());
    }

    let depth_limit = max_depth.unwrap_or(DEFAULT_SCAN_DEPTH).clamp(1, MAX_SCAN_DEPTH);
    let recorded_paths = active_recorded_paths();
    let app_locations = cached_app_locations();
    let profile_roots = profile_roots();

    let mut entries: Vec<RecoveredLinkEntry> = Vec::new();
    let mut scanned_dirs = 0u32;
    let mut skipped_dirs = 0u32;
    let mut truncated = false;
    // 广度优先：按层级推进，触达目录上限时被丢掉的只会是最深、最不相关的目录
    let mut queue: VecDeque<(PathBuf, u32)> = VecDeque::new();
    queue.push_back((root.clone(), 0));

    while let Some((dir, depth)) = queue.pop_front() {
        if cancel_flag.load(Ordering::Relaxed) {
            return Err("用户取消了链接识别".to_string());
        }
        if scanned_dirs >= MAX_SCANNED_DIRS {
            truncated = true;
            break;
        }
        scanned_dirs += 1;
        progress(scanned_dirs, entries.len() as u32, &dir);

        let read_dir = match fs::read_dir(&dir) {
            Ok(read_dir) => read_dir,
            // 权限不足的目录直接跳过：单个目录失败不能中断整次扫描
            Err(_) => {
                skipped_dirs += 1;
                continue;
            }
        };

        for entry in read_dir.flatten() {
            let path = entry.path();
            let Ok(metadata) = fs::symlink_metadata(&path) else { continue };

            // 联接先判定：它既是候选本身，也绝不能递归进入（否则会深入目标盘数据）
            if utils::is_junction(&path) {
                if let Some(candidate) = build_candidate(
                    &path,
                    &recorded_paths,
                    &app_locations,
                    &profile_roots,
                    record_size,
                ) {
                    entries.push(candidate);
                }
                continue;
            }

            if !metadata.file_type().is_dir() {
                continue;
            }
            if should_skip_dir(&path) {
                skipped_dirs += 1;
                continue;
            }
            if depth < depth_limit {
                queue.push_back((path, depth + 1));
            }
        }
    }

    sort_entries(&mut entries);

    Ok(LinkRecoveryScanResult {
        entries,
        scanned_dirs,
        skipped_dirs,
        truncated,
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
}

/// 候选排序：置信度优先，其次原路径，保证「可直接导入」的条目排在前面
fn sort_entries(entries: &mut [RecoveredLinkEntry]) {
    fn confidence_rank(confidence: &str) -> u8 {
        match confidence {
            "high" => 0,
            "medium" => 1,
            _ => 2,
        }
    }
    entries.sort_by(|left, right| {
        confidence_rank(&left.confidence)
            .cmp(&confidence_rank(&right.confidence))
            .then_with(|| left.original_path.to_lowercase().cmp(&right.original_path.to_lowercase()))
    });
}

/// 由目录联接构造候选记录；返回 None 表示该联接已被判定为与 Viap 无关
fn build_candidate(
    original: &Path,
    recorded_paths: &HashSet<String>,
    app_locations: &HashSet<String>,
    profile_roots: &[PathBuf],
    record_size: bool,
) -> Option<RecoveredLinkEntry> {
    let target_string = utils::get_junction_target(original)?;
    let target = PathBuf::from(&target_string);

    // 系统兼容联接数量多且绝不属于 Viap 产物，整条丢弃
    if is_system_compat_link(original, &target, profile_roots) {
        return None;
    }

    let app_name = original
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
        .to_string();
    if app_name.is_empty() {
        return None;
    }

    let target_exists = target.is_dir();
    let target_empty = target_exists && history::is_empty_directory(&target).unwrap_or(false);
    let name_matched = target
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.eq_ignore_ascii_case(&app_name))
        .unwrap_or(false);

    let mut warnings: Vec<String> = Vec::new();
    let mut confidence = "high";

    if !name_matched {
        confidence = "low";
        warnings.push("目标目录名与原目录名不一致，可能由其他工具创建，请谨慎确认".to_string());
    }
    if !target_exists {
        confidence = "low";
        warnings.push("目标目录不存在（悬空联接），没有可恢复的数据".to_string());
    } else if target_empty {
        if confidence == "high" {
            confidence = "medium";
        }
        warnings.push("目标目录为空，可能已无数据，请确认后再导入".to_string());
    }

    let original_string = original.to_string_lossy().to_string();
    let original_key = original_string.to_lowercase();
    let already_recorded = recorded_paths.contains(&original_key);
    if already_recorded {
        warnings.push("迁移记录中已存在该原路径，导入时会被跳过".to_string());
    }

    let size = if record_size && target_exists {
        // 必须用联接目标路径统计：get_dir_size_safe 不跟随链接，直接传联接会得到 0
        utils::get_dir_size_safe(&target)
    } else {
        0
    };

    Some(RecoveredLinkEntry {
        app_name,
        original_path: original_string,
        target_path: target_string.trim_start_matches("\\\\?\\").to_string(),
        record_type: classify_record_type(&target, &original_key, app_locations),
        migrated_at: junction_created_millis(original),
        size,
        confidence: confidence.to_string(),
        target_exists,
        target_empty,
        cross_drive: drive_letter(original) != drive_letter(&target),
        already_recorded,
        warnings,
    })
}

/// 判断是否为 Windows 兼容性联接
///
/// 两类判据：名称命中系统保留名；或目标落在用户配置目录内部——兼容联接的目标恒为
/// 配置目录内的真实位置，而 Viap 的迁移目标必然在配置目录之外（迁移目的就是搬出去）。
fn is_system_compat_link(original: &Path, target: &Path, profile_roots: &[PathBuf]) -> bool {
    if original
        .file_name()
        .and_then(|name| name.to_str())
        .map(is_system_compat_name)
        .unwrap_or(true)
    {
        return true;
    }

    profile_roots.iter().any(|root| path_within_root(target, root))
}

fn is_system_compat_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    SYSTEM_COMPAT_LINK_NAMES.iter().any(|reserved| lower == *reserved)
}

/// 用户配置目录根（USERPROFILE / APPDATA / LOCALAPPDATA）
fn profile_roots() -> Vec<PathBuf> {
    ["USERPROFILE", "APPDATA", "LOCALAPPDATA"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .map(PathBuf::from)
        .collect()
}

/// 大小写不敏感的「path 是否位于 root 之内」
fn path_within_root(path: &Path, root: &Path) -> bool {
    let root_lower = root.to_string_lossy().trim_end_matches(['\\', '/']).to_lowercase();
    if root_lower.is_empty() {
        return false;
    }
    let path_lower = path.to_string_lossy().trim_end_matches(['\\', '/']).to_lowercase();
    path_lower == root_lower || path_lower.starts_with(&format!("{}\\", root_lower))
}

/// 猜测记录类型：能确认是应用安装目录的记为 App，其余按大文件夹处理
fn classify_record_type(
    target: &Path,
    original_key: &str,
    app_locations: &HashSet<String>,
) -> MigrationRecordType {
    if original_key.contains("\\program files\\") || original_key.contains("\\program files (x86)\\") {
        return MigrationRecordType::App;
    }
    // 缓存中的应用列表命中安装位置（覆盖 Wand 这类装在 AppData 的应用），不触发新扫描
    if app_locations.contains(original_key) {
        return MigrationRecordType::App;
    }
    // 根层含 exe 的目录按应用处理；只读一层，避免为分类付出全量遍历成本
    if target_has_root_executable(target) {
        return MigrationRecordType::App;
    }
    MigrationRecordType::LargeFolder
}

fn target_has_root_executable(target: &Path) -> bool {
    let Ok(entries) = fs::read_dir(target) else { return false };
    entries.flatten().any(|entry| {
        entry
            .path()
            .extension()
            .map(|extension| extension.eq_ignore_ascii_case("exe"))
            .unwrap_or(false)
            && entry.file_type().map(|file_type| file_type.is_file()).unwrap_or(false)
    })
}

/// 联接创建时间（Unix 毫秒）
///
/// 必须用 symlink_metadata（lstat）：metadata 会跟随链接返回**目标目录**的创建时间，
/// 而 lstat 返回联接自身的时间戳，才是迁移发生的时间。
fn junction_created_millis(original: &Path) -> u64 {
    fs::symlink_metadata(original)
        .and_then(|metadata| metadata.created())
        .ok()
        .and_then(|created| created.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn drive_letter(path: &Path) -> Option<char> {
    let text = path.to_string_lossy();
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(letter), Some(':')) => Some(letter.to_ascii_uppercase()),
        _ => None,
    }
}

/// 系统目录与 Viap 自身的临时目录不进入递归
fn should_skip_dir(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return true;
    };
    let lower = name.to_lowercase();
    if lower.starts_with(".viap") {
        return true;
    }
    SKIP_DIR_NAMES.iter().any(|skip| lower == *skip)
}

/// 已存在活跃记录的原路径集合（大小写不敏感），用于标记重复候选
fn active_recorded_paths() -> HashSet<String> {
    history::load_history()
        .records
        .iter()
        .filter(|record| record.status == "active")
        .map(|record| record.original_path.to_lowercase())
        .collect()
}

/// 已知应用安装位置（仅读内存缓存，未扫描时返回空集合，不触发新扫描）
fn cached_app_locations() -> HashSet<String> {
    crate::app_manager::cache::get_cached()
        .map(|apps| {
            apps.into_iter()
                .map(|app| app.install_location.to_lowercase())
                .collect()
        })
        .unwrap_or_default()
}

fn emit_scan_progress(
    app_handle: &tauri::AppHandle,
    scanned_dirs: u32,
    found_links: u32,
    current_path: &Path,
) {
    if scanned_dirs % PROGRESS_INTERVAL != 0 && scanned_dirs != 1 {
        return;
    }
    let _ = app_handle.emit(
        "link-recovery-progress",
        LinkRecoveryProgressEvent {
            scanned_dirs,
            found_links,
            current_path: current_path.to_string_lossy().to_string(),
        },
    );
}

// ============================================================================
// 导入校验与自定义文件夹登记
// ============================================================================

/// 逐条重新校验（不信任前端回传），返回拒绝原因
fn validate_import_entry(entry: &RecoveredLinkImport) -> Result<(), String> {
    if entry.original_path.trim().is_empty() || entry.target_path.trim().is_empty() {
        return Err("路径为空".to_string());
    }

    let original = PathBuf::from(&entry.original_path);
    if !utils::is_junction(&original) {
        return Err("原路径已不是目录联接".to_string());
    }

    // 联接目标可能已被改写，必须以当前真实目标为准
    let actual_target = utils::get_junction_target(&original)
        .ok_or_else(|| "无法读取联接目标".to_string())?;
    if !same_path(&actual_target, &entry.target_path) {
        return Err(format!("联接目标已变化（当前指向 {}）", actual_target));
    }

    let target = PathBuf::from(&actual_target);
    if !target.is_dir() {
        return Err("目标目录不存在，没有可恢复的数据".to_string());
    }
    if is_system_compat_link(&original, &target, &profile_roots()) {
        return Err("疑似系统兼容联接，拒绝写入迁移记录".to_string());
    }
    if history::is_empty_directory(&target).unwrap_or(false) {
        return Err("目标目录为空，没有可恢复的数据".to_string());
    }
    Ok(())
}

fn same_path(left: &str, right: &str) -> bool {
    left.trim_end_matches(['\\', '/']).eq_ignore_ascii_case(right.trim_end_matches(['\\', '/']))
}

/// 按用户勾选把识别到的大文件夹登记为自定义文件夹，使其重新出现在「数据迁移」页
fn register_custom_folders(entries: &[RecoveredLinkImport]) -> Result<u32, String> {
    let targets: Vec<&RecoveredLinkImport> = entries
        .iter()
        .filter(|entry| {
            entry.register_custom_folder && entry.record_type == MigrationRecordType::LargeFolder
        })
        .collect();
    if targets.is_empty() {
        return Ok(0);
    }

    let storage_path = utils::custom_folders_path(&ensure_data_dir());
    let mut custom = load_custom_folders(&storage_path);
    let covered_paths = template_covered_paths();
    let mut added = 0u32;

    for entry in targets {
        // 内置应用数据模板已经会展示这些目录，重复登记会让列表出现两行
        if covered_paths.contains(&normalize_for_compare(&entry.original_path)) {
            continue;
        }
        if custom
            .iter()
            .any(|existing| existing.path.eq_ignore_ascii_case(&entry.original_path))
        {
            continue;
        }
        custom.push(CustomFolderEntry {
            id: crate::folder_manager::build_custom_folder_id(&entry.original_path),
            path: entry.original_path.clone(),
            display_name: entry.app_name.clone(),
        });
        added += 1;
    }

    if added > 0 {
        save_custom_folders(&storage_path, &custom)?;
    }
    Ok(added)
}

/// 已被内置应用数据模板覆盖的路径（大小写不敏感）
///
/// 模板目录（浏览器缓存、VSCode 扩展等）本来就由「数据迁移」页动态展示，
/// 恢复时不需要再登记为自定义文件夹。
fn template_covered_paths() -> HashSet<String> {
    let mut covered: HashSet<String> = HashSet::new();

    for template in crate::folder_manager::load_app_data_templates() {
        if let Some(path) = template.path {
            covered.insert(normalize_for_compare(&utils::expand_env_vars(&path)));
        }
    }

    match crate::app_manager::detector::get_special_folders_status() {
        Ok(statuses) => {
            for status in statuses {
                if status.is_detected {
                    covered.insert(normalize_for_compare(&status.current_path));
                }
            }
        }
        // 模板探测失败只影响重复登记判断，不能中断已经完成的历史重建
        Err(error) => log_warn!("link_recovery", "读取应用数据模板状态失败: {}", error),
    }

    covered
}

fn normalize_for_compare(path: &str) -> String {
    path.trim_end_matches(['\\', '/']).to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_compat_names_are_filtered_but_app_names_are_kept() {
        // Windows 兼容联接名称一律排除
        assert!(is_system_compat_name("Application Data"));
        assert!(is_system_compat_name("My Documents"));
        assert!(is_system_compat_name("「开始」菜单"));
        // 普通应用/数据目录名不能被误伤
        assert!(!is_system_compat_name(".cargo"));
        assert!(!is_system_compat_name("Wand"));
    }

    #[test]
    fn path_within_root_matches_only_real_children() {
        let root = PathBuf::from(r"C:\Users\tester");
        assert!(path_within_root(Path::new(r"C:\Users\tester\Documents"), &root));
        assert!(path_within_root(Path::new(r"c:\users\tester"), &root));
        // 前缀相同但并非子目录的路径不能被误判
        assert!(!path_within_root(Path::new(r"C:\Users\tester2\Data"), &root));
        assert!(!path_within_root(Path::new(r"D:\C_Map\link\Wand"), &root));
    }

    #[test]
    fn same_path_ignores_case_and_trailing_separator() {
        assert!(same_path(r"D:\C_Map\link\Wand\", r"d:\c_map\link\wand"));
        assert!(!same_path(r"D:\C_Map\link\Wand", r"D:\C_Map\link\Other"));
    }

    #[test]
    fn import_rejects_plain_directory() {
        // 普通目录（非联接）必须被拒绝，避免把用户新建的普通目录写成迁移记录
        let suffix = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("系统时间应有效")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("viap-link-recovery-{suffix}"));
        std::fs::create_dir_all(&root).expect("创建测试目录失败");

        let entry = RecoveredLinkImport {
            app_name: "fixture".to_string(),
            original_path: root.to_string_lossy().to_string(),
            target_path: root.to_string_lossy().to_string(),
            record_type: MigrationRecordType::LargeFolder,
            migrated_at: 0,
            size: 0,
            register_custom_folder: false,
        };
        assert!(validate_import_entry(&entry).is_err());

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 端到端验证联接识别：同一联接在「目标位于用户配置目录内」时被排除，
    /// 在显式传入空配置目录列表时作为高置信候选返回。
    #[cfg(windows)]
    #[test]
    fn junction_is_excluded_inside_profile_and_reported_outside() {
        let suffix = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("系统时间应有效")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("viap-link-scan-{suffix}"));
        let target = root.join("target").join("SampleApp");
        let link = root.join("source").join("SampleApp");
        std::fs::create_dir_all(&target).expect("创建目标目录失败");
        std::fs::create_dir_all(link.parent().expect("联接父目录")).expect("创建源目录失败");
        std::fs::write(target.join("payload.bin"), b"viap").expect("写入测试文件失败");
        junction::create(&target, &link).expect("创建目录联接失败");

        let recorded = HashSet::new();
        let app_locations = HashSet::new();

        // 目标仍在 %TEMP%（属于 LOCALAPPDATA）内 → 按系统兼容联接排除
        let inside_profile = build_candidate(&link, &recorded, &app_locations, &profile_roots(), true);
        assert!(inside_profile.is_none(), "目标位于用户配置目录内时应被排除");

        // 假设目标已搬离配置目录 → 应作为高置信候选，且名称一致
        let candidate = build_candidate(&link, &recorded, &app_locations, &[], true)
            .expect("合法联接应产出候选");
        assert_eq!(candidate.app_name, "SampleApp");
        assert!(candidate.target_exists);
        assert!(!candidate.target_empty);
        assert_eq!(candidate.confidence, "high");
        assert_eq!(candidate.size, 4);

        let _ = std::fs::remove_dir(link);
        let _ = std::fs::remove_dir_all(&root);
    }
}
