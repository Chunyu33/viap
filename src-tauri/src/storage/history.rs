// 迁移历史记录持久化模块
//
// 持久化方案：JSON 文件存储，位于 %APPDATA%/viap/migration_history.json
// 选择 JSON 而非 SQLite 的原因：
// 1. 轻量级 — 无需额外依赖
// 2. 可读性 — 用户可直接查看/编辑
// 3. 简单可靠 — 迁移历史是低频写入场景，JSON 足够

use std::path::{Path, PathBuf};
use std::fs;
use std::io::{Read, Write};
use std::time::{SystemTime, UNIX_EPOCH};
use std::collections::HashSet;

use crate::models::*;
use crate::utils;
use super::data_dir::ensure_data_dir;

/// 获取历史记录文件路径
pub fn get_history_file_path() -> PathBuf {
    utils::history_file_path(&ensure_data_dir())
}

/// 从 JSON 文件加载历史记录
pub fn load_history() -> HistoryStorage {
    let path = get_history_file_path();

    if !path.exists() {
        return HistoryStorage { version: 1, records: Vec::new() };
    }

    let mut file = match fs::File::open(&path) {
        Ok(f) => f,
        Err(_) => return HistoryStorage { version: 1, records: Vec::new() },
    };

    let mut contents = String::new();
    if file.read_to_string(&mut contents).is_err() {
        return HistoryStorage { version: 1, records: Vec::new() };
    }

    serde_json::from_str(&contents).unwrap_or(HistoryStorage { version: 1, records: Vec::new() })
}

/// 原子写入历史记录
///
/// 策略：先写临时文件 → sync 刷盘 → 备份旧文件 → rename 覆盖
/// 确保写入过程中崩溃不会损坏原有数据
pub fn save_history(storage: &HistoryStorage) -> Result<(), String> {
    let path = get_history_file_path();
    let temp_path = path.with_extension("json.tmp");
    let backup_path = path.with_extension("json.bak");

    let json = serde_json::to_string_pretty(storage)
        .map_err(|e| format!("序列化历史记录失败: {}", e))?;

    // 1. 写入临时文件并刷盘
    let mut file = fs::File::create(&temp_path)
        .map_err(|e| format!("创建临时文件失败: {}", e))?;
    file.write_all(json.as_bytes())
        .map_err(|e| format!("写入临时文件失败: {}", e))?;
    file.sync_all()
        .map_err(|e| format!("同步临时文件失败: {}", e))?;

    // 2. 备份旧文件（失败不阻塞）
    if path.exists() {
        let _ = fs::copy(&path, &backup_path);
    }

    // 3. 原子替换
    fs::rename(&temp_path, &path)
        .map_err(|e| format!("重命名历史文件失败: {}", e))?;

    Ok(())
}

/// 添加一条迁移记录，返回记录 ID
pub fn add_migration_record(
    app_name: &str,
    original_path: &str,
    target_path: &str,
    size: u64,
    record_type: MigrationRecordType,
) -> Result<String, String> {
    let mut storage = load_history();

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let id = format!("mig_{}", timestamp);

    storage.records.push(MigrationRecord {
        id: id.clone(),
        app_name: app_name.to_string(),
        original_path: original_path.to_string(),
        target_path: target_path.to_string(),
        size,
        migrated_at: timestamp,
        status: "active".to_string(),
        record_type,
    });

    save_history(&storage)?;
    Ok(id)
}

/// 更新迁移记录状态（按 original_path 大小写不敏感匹配）
pub fn update_migration_record_status(original_path: &str, new_status: &str) -> Result<(), String> {
    let mut storage = load_history();

    let found = storage.records.iter_mut().any(|record| {
        if record.original_path.eq_ignore_ascii_case(original_path) && record.status == "active" {
            record.status = new_status.to_string();
            true
        } else {
            false
        }
    });

    if !found {
        return Err(format!("未找到路径 {} 的迁移记录", original_path));
    }

    save_history(&storage)
}

/// 卸载联动：将指定路径的迁移记录标记为已卸载，同时清理兜底元数据
/// 在 force_remove_application / uninstall_application 成功后调用
pub fn delete_migration_record_by_path(original_path: &str) {
    // 调取当前记录
    let mut storage = load_history();

    // 标记匹配的活跃记录为 uninstalled
    let mut updated = false;
    for record in storage.records.iter_mut() {
        if record.original_path.eq_ignore_ascii_case(original_path) && record.status == "active" {
            record.status = "uninstalled".to_string();
            updated = true;
        }
    }

    if updated {
        if let Err(e) = save_history(&storage) {
            eprintln!("[viap][history] 卸载后更新迁移记录失败: {}", e);
        }
    }

    // 同步清理兜底元数据
    crate::storage::migrated_app_metadata::remove_migrated_app(original_path);
}

/// 清理单条损坏的迁移记录（broken_lost）
/// 适用于用户通过外部途径（非 Viap）卸载了应用后，历史记录仍残留的场景
/// 执行内容：删除残留 Junction → 删除空目标目录 → 标记记录为 ghost_cleaned
#[tauri::command]
pub fn cleanup_broken_record(history_id: String) -> Result<MigrationResult, String> {
    let mut storage = load_history();

    let record_index = storage
        .records
        .iter()
        .position(|r| r.id == history_id && r.status == "active")
        .ok_or("未找到该迁移记录")?;

    let record = storage.records[record_index].clone();
    let original = std::path::Path::new(&record.original_path);
    let target = std::path::Path::new(&record.target_path);

    // 删除 Junction（若仍存在）—— remove_dir 只删 Junction 本身，不跟踪目标
    if crate::utils::is_junction(original) {
        let _ = std::fs::remove_dir(original);
    }

    // 删除目标目录残留（若存在且为空/已卸载）
    if target.exists() {
        let _ = std::fs::remove_dir_all(target);
    }

    // 标记记录为 ghost_cleaned
    storage.records[record_index].status = "ghost_cleaned".to_string();
    save_history(&storage)?;

    // 同步清理兜底元数据
    crate::storage::migrated_app_metadata::remove_migrated_app(&record.original_path);

    Ok(MigrationResult {
        success: true,
        message: format!("已清理「{}」的迁移记录", record.app_name),
        new_path: None,
    })
}

// ============================================================================
// 查询命令
// ============================================================================

/// 按记录 ID 更新状态
pub fn update_record_status_by_id(id: &str, new_status: &str) -> Result<(), String> {
    let mut storage = load_history();
    let found = storage.records.iter_mut().any(|r| {
        if r.id == id {
            r.status = new_status.to_string();
            true
        } else {
            false
        }
    });
    if !found {
        return Err(format!("未找到 ID 为 {} 的记录", id));
    }
    save_history(&storage)
}

/// 获取活跃的迁移记录
#[tauri::command]
pub fn get_migration_history() -> Result<Vec<MigrationRecord>, String> {
    let storage = load_history();
    Ok(storage.records.into_iter().filter(|r| r.status == "active").collect())
}

/// 获取所有已迁移应用的原始路径列表
#[tauri::command]
pub fn get_migrated_paths() -> Result<Vec<String>, String> {
    let storage = load_history();
    Ok(storage.records.iter()
        .filter(|r| r.status == "active")
        .map(|r| r.original_path.clone())
        .collect())
}

/// 检查迁移记录的链接健康状态
#[tauri::command]
pub fn check_link_status(record_id: String) -> Result<LinkStatusResult, String> {
    let storage = load_history();

    let record = match storage.records.iter().find(|r| r.id == record_id && r.status == "active") {
        Some(r) => r,
        None => return Ok(LinkStatusResult {
            healthy: false, target_exists: false, is_junction: false,
            error: Some("未找到该迁移记录".to_string()),
        }),
    };

    let original_path = Path::new(&record.original_path);
    let target_path = Path::new(&record.target_path);
    let is_junc = utils::is_junction(original_path);
    let target_exists = target_path.exists();

    Ok(LinkStatusResult {
        healthy: is_junc && target_exists,
        target_exists, is_junction: is_junc, error: None,
    })
}

// ============================================================================
// 幽灵链接管理
// ============================================================================

/// 预览无效记录（只读扫描，不执行删除）
///
/// 统一检测三类损坏：
/// - target_missing：新盘目标路径已不存在，数据丢失
/// - junction_broken：原路径存在但不是 Junction，链接已断裂
/// - original_missing：原路径 Junction 已被手动删除，记录孤立
#[tauri::command]
pub fn preview_ghost_links() -> Result<GhostLinkPreview, String> {
    let storage = load_history();
    let mut entries = Vec::new();
    let mut total_size: u64 = 0;

    for record in &storage.records {
        if record.status != "active" { continue; }

        let original_path = Path::new(&record.original_path);
        let target_path = Path::new(&record.target_path);

        let target_missing = !target_path.exists();
        let junction_broken = original_path.exists() && !utils::is_junction(original_path);
        let original_missing = !original_path.exists() && !utils::is_junction(original_path);

        if target_missing || junction_broken || original_missing {
            let damage_type = if target_missing {
                "target_missing"
            } else if junction_broken {
                "junction_broken"
            } else {
                "original_missing"
            };

            entries.push(GhostLinkEntry {
                record_id: record.id.clone(),
                app_name: record.app_name.clone(),
                original_path: record.original_path.clone(),
                target_path: record.target_path.clone(),
                size: record.size,
                damage_type: damage_type.to_string(),
            });
            total_size += record.size;
        }
    }

    Ok(GhostLinkPreview { entries, total_size })
}

/// 清理无效记录
///
/// 覆盖三类损坏的清理：
/// - Junction 正常删除，非 Junction 普通目录拒绝删除（保护用户数据）并报错
/// - 原路径已消失的直接更新状态即可
#[tauri::command]
pub fn clean_ghost_links() -> Result<CleanupResult, String> {
    let mut storage = load_history();
    let mut cleaned_count = 0u32;
    let mut cleaned_size: u64 = 0;
    let mut errors: Vec<String> = Vec::new();

    for record in storage.records.iter_mut() {
        if record.status != "active" { continue; }

        let original_path = Path::new(&record.original_path);
        let target_path = Path::new(&record.target_path);

        let target_missing = !target_path.exists();
        let junction_broken = original_path.exists() && !utils::is_junction(original_path);
        let original_missing = !original_path.exists() && !utils::is_junction(original_path);

        if !target_missing && !junction_broken && !original_missing {
            continue; // 健康记录，跳过
        }

        // 清理原路径（如果还存在）
        if original_path.exists() {
            if utils::is_junction(original_path) {
                // Junction → 正常删除
                if let Err(e) = fs::remove_dir(original_path) {
                    errors.push(format!("无法删除链接 {}: {}", record.original_path, e));
                    continue;
                }
            } else {
                // 非 Junction 的普通目录 → 拒绝删除以保护数据，但仍清理记录
                // 场景：restore_app 已把数据移回但 Junction 实际是普通目录、或用户手动移回数据
                errors.push(format!(
                    "{} 的原路径 {} 是普通目录（非 Junction），已保留该目录并清理无效记录，请手动确认。\n\
                     提示：若需重新迁移到 {}，应用已支持覆盖残留目录",
                    record.app_name, record.original_path, record.target_path
                ));
                // 继续执行下方 status 更新，不 continue 阻塞
            }
        }
        // original_path 不存在 → 无需清理，直接更新状态

        record.status = "ghost_cleaned".to_string();
        cleaned_count += 1;
        cleaned_size += record.size;

        if record.record_type == MigrationRecordType::App {
            crate::storage::migrated_app_metadata::remove_migrated_app(&record.original_path);
        }
    }

    if cleaned_count > 0 || !errors.is_empty() {
        save_history(&storage)?;
    }

    Ok(CleanupResult { cleaned_count, cleaned_size, errors })
}

// ============================================================================
// 统计信息
// ============================================================================

/// 获取迁移统计信息
#[tauri::command]
pub fn get_migration_stats() -> Result<MigrationStats, String> {
    let storage = load_history();

    let mut total_migrated: u64 = 0;
    let mut active_count: u32 = 0;
    let mut restored_count: u32 = 0;
    let mut app_count: u32 = 0;
    let mut folder_count: u32 = 0;

    for record in &storage.records {
        match record.status.as_str() {
            "active" => {
                active_count += 1;
                total_migrated += record.size;
                if record.record_type == MigrationRecordType::LargeFolder {
                    folder_count += 1;
                } else {
                    app_count += 1;
                }
            }
            "restored" => { restored_count += 1; }
            _ => {}
        }
    }

    Ok(MigrationStats {
        total_space_saved: total_migrated,
        active_migrations: active_count,
        restored_count,
        app_migrations: app_count,
        folder_migrations: folder_count,
    })
}

// ============================================================================
// 导入导出
// ============================================================================

/// 导出迁移历史记录到指定路径
#[tauri::command]
pub fn export_history(dest_path: String) -> Result<(), String> {
    let src = get_history_file_path();
    if !src.exists() {
        return Err("历史记录文件不存在，请先执行迁移操作".to_string());
    }
    fs::copy(&src, &dest_path).map_err(|e| format!("导出失败: {}", e))?;
    Ok(())
}

/// 从指定路径导入并合并迁移历史记录（按 id 去重）
#[tauri::command]
pub fn import_history(src_path: String) -> Result<u32, String> {
    let import_path = Path::new(&src_path);
    if !import_path.exists() { return Err("导入文件不存在".to_string()); }

    let contents = fs::read_to_string(import_path)
        .map_err(|e| format!("读取导入文件失败: {}", e))?;

    let imported: HistoryStorage = serde_json::from_str(&contents)
        .map_err(|e| format!("导入文件格式无效: {}", e))?;

    let mut current = load_history();
    let existing_ids: HashSet<String> = current.records.iter().map(|r| r.id.clone()).collect();

    let mut added: u32 = 0;
    for record in imported.records {
        if !existing_ids.contains(&record.id) {
            current.records.push(record);
            added += 1;
        }
    }

    if added > 0 { save_history(&current)?; }
    Ok(added)
}

// ============================================================================
// 应用还原
// ============================================================================

/// 恢复已迁移应用到原始位置
///
/// # 恢复流程
/// 0. 获取全局恢复锁（防止并发）
/// 1. 查找迁移记录
/// 2. 验证状态（目标存在、原路径为 Junction 或不存在）
/// 3. 空间检查（必须在删除 Junction 前执行）
/// 4. 删除 Junction（确认是 Reparse Point 才删除）
/// 5. 移动文件回原位置（失败时回滚重建 Junction）
/// 6. 更新记录状态
#[tauri::command]
pub fn restore_app(history_id: String, app_handle: tauri::AppHandle) -> Result<MigrationResult, String> {
    #[cfg(windows)]
    {
        // 步骤 0: 尝试获取恢复锁，防止并发恢复任务互相干扰
        let _guard = match utils::try_acquire_restore_lock() {
            Ok(guard) => guard,
            Err(msg) => return Ok(MigrationResult {
                success: false, message: msg, new_path: None,
            }),
        };

        // 步骤 1: 查找记录
        let mut storage = load_history();

        let record_index = match storage.records.iter().position(|r| r.id == history_id && r.status == "active") {
            Some(i) => i,
            None => return Ok(MigrationResult {
                success: false,
                message: "未找到该迁移记录或已被恢复".to_string(),
                new_path: None,
            }),
        };

        let record = storage.records[record_index].clone();

        // 若记录类型为 LargeFolder，分发给大文件夹恢复逻辑
        // 统一入口使所有恢复操作都能正确更新 history 记录状态
        if record.record_type == MigrationRecordType::LargeFolder {
            drop(storage);
            return crate::folder_manager::restore_large_folder_by_history(
                history_id,
                record,
                app_handle,
            );
        }

        let original_path = Path::new(&record.original_path);
        let target_path = Path::new(&record.target_path);

        // 步骤 2: 验证状态
        if !target_path.exists() {
            return Ok(MigrationResult {
                success: false,
                message: format!("目标路径不存在: {}，可能已被手动删除", record.target_path),
                new_path: None,
            });
        }

        // 用 utils::is_junction 替代 symlink_metadata().is_symlink()
        // Windows Junction（目录联接）在 Rust 标准库中 is_symlink() 返回 false，
        // 而 symlink_dir 在未开启开发者模式时实际创建的是 Junction，导致无法恢复
        let is_junction = utils::is_junction(original_path);

        // 原路径存在但不是 Reparse Point → 真正的普通目录，拒绝恢复保护用户数据
        if original_path.exists() && !is_junction {
            // 原路径是普通目录，说明之前的恢复操作中断了（复制回原位部分成功）
            // 根据 target 是否还存在给出不同提示
            let target_still_exists = target_path.exists()
                && std::fs::read_dir(&target_path)
                    .map(|mut d| d.next().is_some())
                    .unwrap_or(false);

            let message = if target_still_exists {
                format!(
                    "检测到上次恢复未完成（原路径 {} 是普通目录而非链接）。\n\n\
                     目标位置 {} 仍有数据。\n\n\
                     修复方法：\n\
                     1. 将 {} 目录下所有内容复制/合并到 {}\n\
                     2. 删除 {} 目录\n\
                     3. 删除 {} 目录\n\
                     完成后此记录将自动标记为已损坏，可在迁移记录中清理。",
                    record.original_path,
                    record.target_path,
                    record.target_path,
                    record.original_path,
                    record.target_path,
                    record.original_path,
                )
            } else {
                format!(
                    "原路径 {} 是普通目录（不是迁移创建的链接），且目标位置 {} 已无数据。\n\n\
                     数据可能已在原路径恢复完毕（上次恢复部分成功）。\n\
                     请检查 {} 目录内容是否完整，若完整则无需恢复。\n\
                     若内容不完整，请手动从备份恢复。",
                    record.original_path,
                    record.target_path,
                    record.original_path,
                )
            };

            return Ok(MigrationResult {
                success: false,
                message,
                new_path: None,
            });
        }

        crate::app_manager::migration::emit_progress(
            &app_handle,
            &record.original_path,
            0.0,
            "checking",
            "正在检查恢复条件...",
            0,
            record.size,
        );

        // 步骤 3.5: 进程占用检测（必须在删除 Junction 之前）
        // 应用运行中会导致复制/清理失败，提前拒绝可以避免进入恢复半程状态
        // 提前检测并拒绝，不动任何文件，是最安全的策略
        {
            let mut sys = sysinfo::System::new_all();
            sys.refresh_all();
            let original_lower = record.original_path.to_lowercase();
            // Junction 透明：进程 exe 路径实际指向 target，但 os 层面以 original_path 上报
            // 同时检测 original_path 和 target_path 前缀，覆盖两种上报方式
            let target_lower = record.target_path.to_lowercase();
            let running: Vec<String> = sys.processes().values()
                .filter_map(|p| {
                    p.exe().and_then(|exe| {
                        let exe_lower = exe.to_string_lossy().to_lowercase();
                        if exe_lower.starts_with(&original_lower)
                            || exe_lower.starts_with(&target_lower)
                        {
                            Some(p.name().to_string_lossy().to_string())
                        } else {
                            None
                        }
                    })
                })
                .collect();

            if !running.is_empty() {
                return Ok(MigrationResult {
                    success: false,
                    message: format!(
                        "检测到以下程序正在运行，请关闭后重试：\n{}\n\n\
                         恢复前必须关闭应用，否则文件移动可能失败并导致数据损坏。",
                        running.join("、")
                    ),
                    new_path: None,
                });
            }
        }

        let restore_result = crate::app_manager::migration::restore_directory_with_progress(
            original_path,
            target_path,
            &record.original_path,
            &app_handle,
        )?;

        // 步骤 6: 更新记录
        storage.records[record_index].status = "restored".to_string();
        save_history(&storage)?;

        // 同步移除兜底元数据，避免恢复后仍显示为"已迁移"
        crate::storage::migrated_app_metadata::remove_migrated_app(&record.original_path);

        let mut message = format!(
            "恢复成功！应用 {} 已从 {} 恢复到 {}（{}）",
            record.app_name,
            record.target_path,
            record.original_path,
            crate::app_manager::migration::format_bytes(restore_result.restored_size)
        );
        if let Some(warning) = restore_result.cleanup_warning {
            message.push_str(&format!("\n\n{}", warning));
        }

        Ok(MigrationResult {
            success: true,
            message,
            new_path: Some(record.original_path),
        })
    }

    #[cfg(not(windows))]
    {
        Ok(MigrationResult {
            success: false,
            message: "恢复功能仅支持 Windows 系统".to_string(),
            new_path: None,
        })
    }
}

// ============================================================================
// 工具命令
// ============================================================================

/// 在文件资源管理器中打开数据目录
#[tauri::command]
pub fn open_data_dir() -> Result<(), String> {
    let data_dir = ensure_data_dir();
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(data_dir.to_string_lossy().as_ref())
            .spawn()
            .map_err(|e| format!("无法打开资源管理器: {}", e))?;
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::process::Command::new("open")
            .arg(data_dir.to_string_lossy().as_ref())
            .spawn()
            .map_err(|e| format!("无法打开文件管理器: {}", e))?;
    }
    Ok(())
}

/// 在资源管理器中打开指定文件夹
#[tauri::command]
pub fn open_folder(path: String) -> Result<(), String> {
    #[cfg(windows)]
    {
        let path_obj = Path::new(&path);
        if !path_obj.exists() {
            return Err(format!("路径不存在: {}", path));
        }

        let result = if path_obj.is_dir() {
            std::process::Command::new("explorer").arg(&path).spawn()
        } else {
            std::process::Command::new("explorer")
                .arg("/select,").arg(&path).spawn()
        };

        result.map(|_| ()).map_err(|e| format!("打开文件夹失败: {}", e))
    }

    #[cfg(not(windows))]
    { Err("此功能仅支持 Windows 系统".to_string()) }
}
