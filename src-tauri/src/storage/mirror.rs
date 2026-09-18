// 迁移数据镜像备份模块
//
// 背景：数据目录可在设置中改到任意盘，一旦被误删，migration_history.json /
// custom_folders.json / migrated_apps.json 会一起丢失。此时原路径上的目录联接
// 仍然存在，但用户无法从界面恢复「已迁移」状态与恢复入口。
//
// 做法：每次成功落盘后，把同样的三份数据镜像到与数据目录解耦的固定位置，
// 供「从镜像备份导入」一键恢复。
//
// 镜像写入是尽力而为：失败只记录日志，绝不阻塞或回滚主流程——镜像只是兜底，
// 主数据的写入成功与否不取决于镜像结果。

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::de::DeserializeOwned;

use crate::models::*;
use crate::storage::data_dir::{ensure_data_dir, load_custom_folders, save_custom_folders};
use crate::storage::history;
use crate::storage::migrated_app_metadata::{self, MigratedAppEntry, MigratedAppStorage};
use crate::utils;

const MIRROR_HISTORY_FILE: &str = "migration_history.json";
const MIRROR_CUSTOM_FOLDER_FILE: &str = "custom_folders.json";
const MIRROR_MIGRATED_APP_FILE: &str = "migrated_apps.json";

/// 镜像目录：安装版 %LOCALAPPDATA%\viap-recovery，便携版 <程序目录>\recovery。
///
/// 便携版不写用户 AppData，保持「复制整个文件夹即可搬走」的行为一致。
#[cfg(feature = "portable")]
fn mirror_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join("recovery")))
        .unwrap_or_else(|| PathBuf::from("recovery"))
}

#[cfg(not(feature = "portable"))]
fn mirror_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("viap-recovery")
}

fn mirror_file_path(file_name: &str) -> PathBuf {
    mirror_dir().join(file_name)
}

/// 原子写入镜像文件（temp → rename）；失败只记录日志
fn write_mirror_file(file_name: &str, json: &str) {
    write_mirror_file_at(&mirror_file_path(file_name), json)
}

fn write_mirror_file_at(path: &Path, json: &str) {
    if let Some(parent) = path.parent() {
        if let Err(error) = fs::create_dir_all(parent) {
            log_warn!("mirror", "创建镜像目录失败 {}: {}", parent.display(), error);
            return;
        }
    }

    let temp_path = path.with_extension("json.tmp");
    if let Err(error) = fs::write(&temp_path, json).and_then(|_| fs::rename(&temp_path, &path)) {
        log_warn!("mirror", "写入镜像文件失败 {}: {}", path.display(), error);
    }
}

/// 读取镜像文件；文件缺失或损坏时返回 None 并记录原因
fn read_mirror_json<T: DeserializeOwned>(file_name: &str) -> Option<T> {
    read_mirror_json_at(&mirror_file_path(file_name))
}

fn read_mirror_json_at<T: DeserializeOwned>(path: &Path) -> Option<T> {
    if !path.exists() {
        return None;
    }

    match fs::read_to_string(path) {
        Ok(contents) => match serde_json::from_str::<T>(&contents) {
            Ok(value) => Some(value),
            Err(error) => {
                log_warn!("mirror", "镜像文件格式无效 {}: {}", path.display(), error);
                None
            }
        },
        Err(error) => {
            log_warn!("mirror", "读取镜像文件失败 {}: {}", path.display(), error);
            None
        }
    }
}

/// 自动备份开关（用户可在设置页关闭，关闭后只保留手动备份）
fn auto_backup_enabled() -> bool {
    crate::storage::user_settings::load_current_settings().auto_backup_enabled
}

/// 镜像迁移历史（save_history 成功后调用）
pub fn mirror_history(storage: &HistoryStorage) {
    if !auto_backup_enabled() {
        return;
    }
    write_history_mirror(storage);
}

/// 镜像自定义文件夹列表（save_custom_folders 成功后调用）
pub fn mirror_custom_folders(folders: &[CustomFolderEntry]) {
    if !auto_backup_enabled() {
        return;
    }
    write_custom_folders_mirror(folders);
}

/// 镜像迁移应用兜底元数据（save_all 成功后调用）
pub fn mirror_migrated_apps(apps: &[MigratedAppEntry]) {
    if !auto_backup_enabled() {
        return;
    }
    write_migrated_apps_mirror(apps);
}

/// 在资源管理器中打开备份目录（不存在时先创建，避免首次点击报错）
#[tauri::command]
pub fn open_mirror_dir() -> Result<(), String> {
    let dir = mirror_dir();
    if let Err(error) = fs::create_dir_all(&dir) {
        return Err(format!("无法创建备份目录 {}: {}", dir.display(), error));
    }

    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = std::process::Command::new("explorer");
        command.arg(dir.to_string_lossy().as_ref());
        command
    };
    #[cfg(not(target_os = "windows"))]
    let mut command = {
        let mut command = std::process::Command::new("open");
        command.arg(dir.to_string_lossy().as_ref());
        command
    };

    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("打开备份目录失败: {}", error))
}

/// 手动备份入口：不受自动备份开关影响
#[tauri::command]
pub fn backup_now() -> Result<MirrorBackupInfo, String> {
    write_history_mirror(&history::load_history());

    let custom_folders = load_custom_folders(&utils::custom_folders_path(&ensure_data_dir()));
    write_custom_folders_mirror(&custom_folders);

    write_migrated_apps_mirror(&migrated_app_metadata::load_all());

    let info = collect_mirror_info();
    if !info.exists {
        return Err(format!("备份写入失败，请检查 {} 是否可写", info.path));
    }
    Ok(info)
}

fn write_history_mirror(storage: &HistoryStorage) {
    match serde_json::to_string_pretty(storage) {
        Ok(json) => write_mirror_file(MIRROR_HISTORY_FILE, &json),
        Err(error) => log_warn!("mirror", "序列化历史镜像失败: {}", error),
    }
}

/// 从自动备份中删除某条迁移记录（用户显式勾选时调用）
///
/// 返回该记录在备份中是否存在；备份文件不存在时视为已删除，不报错。
pub fn remove_record_from_mirror(record_id: &str) -> Result<bool, String> {
    remove_record_from_mirror_at(&mirror_file_path(MIRROR_HISTORY_FILE), record_id)
}

/// 删除逻辑核心：接收备份文件路径，便于单测在临时文件上验证
fn remove_record_from_mirror_at(path: &Path, record_id: &str) -> Result<bool, String> {
    let Some(mut storage) = read_mirror_json_at::<HistoryStorage>(path) else {
        return Ok(false);
    };

    let before = storage.records.len();
    storage.records.retain(|record| record.id != record_id);
    if storage.records.len() == before {
        return Ok(false);
    }

    match serde_json::to_string_pretty(&storage) {
        Ok(json) => write_mirror_file_at(path, &json),
        Err(error) => log_warn!("mirror", "序列化历史镜像失败: {}", error),
    }
    Ok(true)
}

/// 从自动备份的兜底元数据中删除某个应用
pub fn remove_migrated_app_from_mirror(original_path: &str) -> Result<(), String> {
    let Some(mut storage) = read_mirror_json::<MigratedAppStorage>(MIRROR_MIGRATED_APP_FILE) else {
        return Ok(());
    };

    let before = storage.apps.len();
    storage.apps.retain(|entry| !entry.original_path.eq_ignore_ascii_case(original_path));
    if storage.apps.len() != before {
        write_migrated_apps_mirror(&storage.apps);
    }
    Ok(())
}

fn write_custom_folders_mirror(folders: &[CustomFolderEntry]) {
    match serde_json::to_string_pretty(folders) {
        Ok(json) => write_mirror_file(MIRROR_CUSTOM_FOLDER_FILE, &json),
        Err(error) => log_warn!("mirror", "序列化自定义文件夹镜像失败: {}", error),
    }
}

fn write_migrated_apps_mirror(apps: &[MigratedAppEntry]) {
    let storage = MigratedAppStorage { apps: apps.to_vec() };
    match serde_json::to_string_pretty(&storage) {
        Ok(json) => write_mirror_file(MIRROR_MIGRATED_APP_FILE, &json),
        Err(error) => log_warn!("mirror", "序列化兜底元数据镜像失败: {}", error),
    }
}

/// 取镜像文件中最近的修改时间（Unix 毫秒），作为「镜像备份时间」展示
fn mirror_saved_at() -> u64 {
    [MIRROR_HISTORY_FILE, MIRROR_CUSTOM_FOLDER_FILE, MIRROR_MIGRATED_APP_FILE]
        .iter()
        .filter_map(|file_name| {
            let metadata = fs::metadata(mirror_file_path(file_name)).ok()?;
            let modified = metadata.modified().ok()?;
            let millis = modified.duration_since(UNIX_EPOCH).ok()?.as_millis() as u64;
            Some(millis)
        })
        .max()
        .unwrap_or(0)
}

/// 读取镜像备份信息，供前端判断是否提供「从自动备份导入」
#[tauri::command]
pub fn get_mirror_backup_info() -> Result<MirrorBackupInfo, String> {
    Ok(collect_mirror_info())
}

fn collect_mirror_info() -> MirrorBackupInfo {
    let history_storage = read_mirror_json::<HistoryStorage>(MIRROR_HISTORY_FILE);
    let custom_folders =
        read_mirror_json::<Vec<CustomFolderEntry>>(MIRROR_CUSTOM_FOLDER_FILE).unwrap_or_default();
    let migrated_apps =
        read_mirror_json::<MigratedAppStorage>(MIRROR_MIGRATED_APP_FILE).unwrap_or_default();

    MirrorBackupInfo {
        exists: history_storage.is_some() || !custom_folders.is_empty() || !migrated_apps.apps.is_empty(),
        auto_backup_enabled: auto_backup_enabled(),
        path: mirror_dir().to_string_lossy().to_string(),
        history_count: history_storage.map(|storage| storage.records.len() as u32).unwrap_or(0),
        custom_folder_count: custom_folders.len() as u32,
        migrated_app_count: migrated_apps.apps.len() as u32,
        saved_at: mirror_saved_at(),
    }
}

/// 从镜像备份导入：迁移历史按记录 ID 去重，自定义文件夹与兜底元数据按路径去重
///
/// 只做「补充」，不覆盖或删除现有数据，因此可在任意时刻安全执行。
#[tauri::command]
pub fn import_mirror_backup() -> Result<MirrorImportResult, String> {
    let mut result = MirrorImportResult::default();
    let mut found_any = false;

    if let Some(mirror_history) = read_mirror_json::<HistoryStorage>(MIRROR_HISTORY_FILE) {
        found_any = true;
        let mut current = history::load_history();
        let existing_ids: HashSet<String> =
            current.records.iter().map(|record| record.id.clone()).collect();

        for record in mirror_history.records {
            if existing_ids.contains(&record.id) {
                result.history_skipped += 1;
                continue;
            }
            current.records.push(record);
            result.history_added += 1;
        }

        if result.history_added > 0 {
            history::save_history(&current)?;
        }
    }

    if let Some(mirror_folders) = read_mirror_json::<Vec<CustomFolderEntry>>(MIRROR_CUSTOM_FOLDER_FILE) {
        found_any = true;
        let storage_path = utils::custom_folders_path(&ensure_data_dir());
        let mut custom = load_custom_folders(&storage_path);
        for folder in mirror_folders {
            if custom.iter().any(|existing| existing.path.eq_ignore_ascii_case(&folder.path)) {
                continue;
            }
            custom.push(folder);
            result.custom_folders_added += 1;
        }
        if result.custom_folders_added > 0 {
            save_custom_folders(&storage_path, &custom)?;
        }
    }

    if let Some(mirror_apps) = read_mirror_json::<MigratedAppStorage>(MIRROR_MIGRATED_APP_FILE) {
        found_any = true;
        let mut apps = migrated_app_metadata::load_all();
        for entry in mirror_apps.apps {
            if apps.iter().any(|existing| existing.original_path.eq_ignore_ascii_case(&entry.original_path)) {
                continue;
            }
            apps.push(entry);
            result.migrated_apps_added += 1;
        }
        if result.migrated_apps_added > 0 {
            migrated_app_metadata::save_all(&apps)?;
        }
    }

    if !found_any {
        return Err(format!(
            "未找到镜像备份文件，请确认 {} 是否存在",
            mirror_dir().display()
        ));
    }

    // 兜底元数据可能补充了新的已迁移应用，失效应用列表缓存让角标刷新
    crate::app_manager::cache::invalidate();
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_history(ids: &[&str]) -> HistoryStorage {
        HistoryStorage {
            version: 1,
            records: ids
                .iter()
                .map(|id| MigrationRecord {
                    id: (*id).to_string(),
                    app_name: "sample".to_string(),
                    original_path: format!(r"C:\sample\{}", id),
                    target_path: format!(r"D:\sample\{}", id),
                    size: 1,
                    migrated_at: 1,
                    status: "active".to_string(),
                    record_type: MigrationRecordType::App,
                })
                .collect(),
        }
    }

    /// 勾选"同时删除备份记录"时，只有目标记录被移除，其余记录保持不动
    #[test]
    fn removes_only_the_requested_record_from_the_backup() {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("系统时间应有效")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("viap-mirror-{suffix}"));
        std::fs::create_dir_all(&dir).expect("创建测试目录失败");
        let path = dir.join("migration_history.json");

        let storage = sample_history(&["mig_1", "mig_2", "mig_3"]);
        std::fs::write(&path, serde_json::to_string_pretty(&storage).unwrap()).expect("写入失败");

        assert_eq!(remove_record_from_mirror_at(&path, "mig_2"), Ok(true));
        let remaining = read_mirror_json_at::<HistoryStorage>(&path).expect("读取失败");
        assert_eq!(remaining.records.len(), 2);
        assert!(remaining.records.iter().all(|record| record.id != "mig_2"));

        // 再次删除同一条记录：不存在，返回 false 且不报错
        assert_eq!(remove_record_from_mirror_at(&path, "mig_2"), Ok(false));
        // 备份文件不存在时同样安全返回
        assert_eq!(remove_record_from_mirror_at(&dir.join("missing.json"), "mig_1"), Ok(false));

        std::fs::remove_dir_all(&dir).ok();
    }
}