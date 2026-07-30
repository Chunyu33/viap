// 大文件夹发现与管理模块
//
// 负责系统文件夹、应用数据文件夹和自定义文件夹的扫描、
// 迁移和恢复，以及应用数据模板的管理

use std::path::PathBuf;
use std::sync::atomic::Ordering;

use tauri::AppHandle;

use crate::models::*;
use crate::utils;
use crate::storage::data_dir;
use crate::storage::data_dir::ensure_data_dir;

mod size_scan;
mod templates;

pub use templates::{default_app_data_templates, load_app_data_templates};

// ============================================================================
// 应用数据模板管理
// ============================================================================

/// 获取应用数据模板（Tauri 命令，供设置页展示和编辑）
#[tauri::command]
pub fn get_app_data_templates() -> Result<Vec<AppDataTemplate>, String> {
    Ok(templates::load_app_data_templates())
}

/// 保存应用数据模板
#[tauri::command]
pub fn save_app_data_templates(templates: Vec<AppDataTemplate>) -> Result<(), String> {
    templates::save_app_data_templates(templates)
}

// ============================================================================
// 大文件夹列表
// ============================================================================

/// 获取大文件夹列表
///
/// # 路径定位说明
///
/// ## 系统文件夹
/// 使用 `dirs` crate 获取 Windows 已知文件夹路径（Desktop/Documents/Downloads/Pictures/Videos）
///
/// ## 应用数据文件夹
/// 从 `app_data_templates.json` 加载模板，内置类型通过 detector 模块动态检测路径
///
/// 注意：返回时 size 均为 0；系统/自定义目录由 start_folder_size_scan 异步计算，
/// 应用数据目录由用户主动调用 start_app_data_size_scan 后计算。
/// 将扫描与计算分离是为了消除竞态：前端注册 large-folder-size 监听器后才启动后台线程，
/// 避免线程在监听器就绪前 emit 事件导致事件丢失。
#[tauri::command]
pub fn get_large_folders() -> Result<Vec<LargeFolder>, String> {
    let mut folders: Vec<LargeFolder> = Vec::new();

    // ========== 系统文件夹 ==========
    let system_folders: Vec<(&str, &str, fn() -> Option<PathBuf>, Vec<&str>)> = vec![
        ("desktop", "桌面", dirs::desktop_dir as fn() -> Option<PathBuf>, vec!["explorer.exe"]),
        ("documents", "文档", dirs::document_dir as fn() -> Option<PathBuf>, vec![]),
        ("downloads", "下载", dirs::download_dir as fn() -> Option<PathBuf>, vec![]),
        ("pictures", "图片", dirs::picture_dir as fn() -> Option<PathBuf>, vec![]),
        ("videos", "视频", dirs::video_dir as fn() -> Option<PathBuf>, vec![]),
    ];

    for (id, name, getter, processes) in system_folders {
        if let Some(dir) = getter() {
            let path_str = dir.to_string_lossy().to_string();
            let is_junc = utils::is_junction(&dir);
            folders.push(LargeFolder {
                id: id.to_string(),
                display_name: name.to_string(),
                path: path_str.clone(),
                size: 0,
                folder_type: LargeFolderType::System,
                is_junction: is_junc,
                junction_target: if is_junc { utils::get_junction_target(&dir) } else { None },
                app_process_names: processes.iter().map(|s| s.to_string()).collect(),
                icon_id: id.to_string(),
                exists: dir.exists(),
            });
        }
    }

    // ========== 应用数据文件夹 ==========
    let app_data_templates = load_app_data_templates();

    let all_statuses = crate::app_manager::detector::get_special_folders_status()?;

    for template in &app_data_templates {
        if let Some(custom_path) = &template.path {
            let expanded = utils::expand_env_vars(custom_path);
            let path = PathBuf::from(&expanded);
            let exists = path.exists() && path.is_dir();
            // 仅隐藏“内置默认路径且未检测到”的开发目录，用户手动改过的路径仍展示为未检测到，便于排查配置。
            if !exists && is_missing_default_builtin_path(template) {
                continue;
            }
            let is_junc = if exists { utils::is_junction(&path) } else { false };
            folders.push(LargeFolder {
                id: template.id.clone(),
                display_name: template.display_name.clone(),
                path: expanded, size: 0,
                folder_type: LargeFolderType::AppData,
                is_junction: is_junc,
                junction_target: if is_junc { utils::get_junction_target(&path) } else { None },
                app_process_names: template.process_names.clone(),
                icon_id: template.icon_id.clone(),
                exists,
            });
        } else {
            // 内置模板：无自定义路径，从 detector 状态读取
            let status = match all_statuses.iter().find(|s| s.name == template.id) {
                Some(s) => s, None => continue,
            };
            let path = PathBuf::from(&status.current_path);
            let exists = status.is_detected;
            let is_junc = if exists { utils::is_junction(&path) } else { false };
            folders.push(LargeFolder {
                id: status.name.clone(),
                display_name: template.display_name.clone(),
                path: status.current_path.clone(), size: 0,
                folder_type: LargeFolderType::AppData,
                is_junction: is_junc,
                junction_target: if is_junc { utils::get_junction_target(&path) } else { None },
                app_process_names: template.process_names.clone(),
                icon_id: template.icon_id.clone(),
                exists,
            });
        }
    }

    // ========== 自定义文件夹 ==========
    let custom = data_dir::load_custom_folders(&utils::custom_folders_path(&ensure_data_dir()));
    for cf in &custom {
        let path = PathBuf::from(&cf.path);
        let exists = path.exists();
        let is_junc = if exists { utils::is_junction(&path) } else { false };
        folders.push(LargeFolder {
            id: cf.id.clone(), display_name: cf.display_name.clone(),
            path: cf.path.clone(), size: 0,
            folder_type: LargeFolderType::Custom,
            is_junction: is_junc,
            junction_target: if is_junc { utils::get_junction_target(&path) } else { None },
            app_process_names: vec![], icon_id: "folder".to_string(), exists,
        });
    }

    // 排序：按类型分组（系统 > 应用数据 > 自定义），已迁移的排后
    folders.sort_by(|a, b| {
        if a.is_junction && !b.is_junction { return std::cmp::Ordering::Greater; }
        if !a.is_junction && b.is_junction { return std::cmp::Ordering::Less; }
        let type_order = |t: &LargeFolderType| match t {
            LargeFolderType::System => 0, LargeFolderType::AppData => 1, LargeFolderType::Custom => 2,
        };
        type_order(&a.folder_type).cmp(&type_order(&b.folder_type))
    });

    Ok(folders)
}

fn is_missing_default_builtin_path(template: &AppDataTemplate) -> bool {
    default_app_data_templates()
        .iter()
        .find(|default_template| default_template.id.eq_ignore_ascii_case(&template.id))
        .and_then(|default_template| default_template.path.as_ref())
        .zip(template.path.as_ref())
        .map(|(default_path, current_path)| default_path.eq_ignore_ascii_case(current_path))
        .unwrap_or(false)
}

/// 启动文件夹大小异步扫描（Tauri 命令）
///
/// 前端在注册好 `large-folder-size` 事件监听器后调用此命令，
/// 避免后台线程在监听器就绪前 emit 事件导致事件丢失。
/// 接收前端回传的文件夹列表（来自 get_large_folders 的返回值），
/// 仅读取路径和 Junction 信息用于大小计算。
#[tauri::command]
pub fn start_folder_size_scan(
    folders: Vec<LargeFolder>,
    app_handle: AppHandle,
    scan_id: Option<String>,
) -> Result<(), String> {
    size_scan::start_folder_size_scan(folders, app_handle, scan_id)
}

/// 用户主动触发应用数据大小扫描。
///
/// 应用数据目录通常位于 HDD，默认不随页面进入自动递归；单独命令让前端明确表达
/// 用户意图，并通过同一个大小事件逐项推送结果。
#[tauri::command]
pub fn start_app_data_size_scan(
    folders: Vec<LargeFolder>,
    app_handle: AppHandle,
    scan_id: Option<String>,
) -> Result<(), String> {
    size_scan::start_app_data_size_scan(folders, app_handle, scan_id)
}

// ============================================================================
// 大文件夹迁移与恢复
// ============================================================================

/// 迁移大文件夹（async，复用 migrate_app 引擎，支持进度上报和取消）
///
/// 改为 async + spawn_blocking，与 migrate_app 命令行为一致：
/// - 通过 migration-progress 事件实时上报进度
/// - 支持 cancel_migration 取消
/// - 直接返回 MigrationResult，前端无需监听完成事件
#[tauri::command]
pub async fn migrate_large_folder(
    source_path: String,
    target_dir: String,
    force_overwrite: Option<bool>,
    user_confirmed_warning: Option<bool>,
    state: tauri::State<'_, MigrationState>,
    app_handle: AppHandle,
) -> Result<MigrationResult, String> {
    let source = PathBuf::from(&source_path);
    if !source.exists() { return Err(format!("源路径不存在: {}", source_path)); }
    if !source.is_dir() { return Err("源路径必须是一个目录".to_string()); }

    let folder_name = source
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let force = force_overwrite.unwrap_or(false);
    let confirmed = user_confirmed_warning.unwrap_or(false);

    state.cancel_flag.store(false, Ordering::SeqCst);
    let cancel_flag = state.cancel_flag.clone();
    let handle = app_handle.clone();

    let result = tauri::async_runtime::spawn_blocking(move || {
        crate::app_manager::migration::migrate_app(
            folder_name, source_path, target_dir, &cancel_flag, &handle,
            MigrationRecordType::LargeFolder, force, confirmed,
        )
    }).await.map_err(|e| format!("迁移线程异常: {}", e))?;

    result
}

/// 添加自定义文件夹
#[tauri::command]
pub fn add_custom_folder(path: String) -> Result<(), String> {
    let folder_path = PathBuf::from(&path);
    if !folder_path.exists() || !folder_path.is_dir() {
        return Err(format!("路径不存在或不是文件夹: {}", path));
    }

    let display_name = folder_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.clone());

    // 基于路径 + 时间戳生成唯一 ID，使用标准库 DefaultHasher 避免碰撞
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hasher);
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .hash(&mut hasher);
    let id = format!("custom_{:x}", hasher.finish());

    let storage_path = utils::custom_folders_path(&ensure_data_dir());
    let mut custom = data_dir::load_custom_folders(&storage_path);
    if custom.iter().any(|c| c.path.to_lowercase() == path.to_lowercase()) {
        return Err("该文件夹已在列表中".to_string());
    }

    custom.push(CustomFolderEntry { id, path, display_name });
    data_dir::save_custom_folders(&storage_path, &custom)
}

/// 删除自定义文件夹
#[tauri::command]
pub fn remove_custom_folder(id: String) -> Result<(), String> {
    let storage_path = utils::custom_folders_path(&ensure_data_dir());
    let mut custom = data_dir::load_custom_folders(&storage_path);
    let before = custom.len();
    custom.retain(|c| c.id != id);
    if custom.len() == before {
        return Err("未找到该自定义文件夹".to_string());
    }
    data_dir::save_custom_folders(&storage_path, &custom)
}

/// 恢复大文件夹（从 Junction 恢复到原位置）
/// async + spawn_blocking：直接返回结果，弃用 fire-and-forget 事件模式
/// 避免线程 panic 导致前端 restoringFolderId 永不清除
#[tauri::command]
pub async fn restore_large_folder(
    junction_path: String,
    app_handle: AppHandle,
) -> Result<MigrationResult, String> {
    #[cfg(windows)]
    {
        let junction = PathBuf::from(&junction_path);

        if !utils::is_junction(&junction) {
            return Err("该路径不是一个符号链接，无法恢复".to_string());
        }

        let target_path = match utils::get_junction_target(&junction) {
            Some(target) => PathBuf::from(target),
            None => return Err("无法读取符号链接的目标路径".to_string()),
        };

        if !target_path.exists() {
            return Err(format!("目标路径不存在: {}", target_path.to_string_lossy()));
        }
        if crate::storage::history::is_empty_directory(&target_path)? {
            return Ok(MigrationResult {
                success: false,
                message: format!(
                    "目标目录为空，没有可恢复的数据：{}\n\n未创建原目录，请先确认该文件夹是否已被卸载或手动清理。",
                    target_path.display()
                ),
                new_path: None,
            });
        }

        // 获取全局恢复锁，防止与 restore_app 或其他恢复任务并发
        let _guard = match utils::try_acquire_restore_lock() {
            Ok(guard) => guard,
            Err(msg) => return Ok(MigrationResult {
                success: false, message: msg, new_path: None,
            }),
        };

        let target_path_str = target_path.to_string_lossy().to_string();
        let handle = app_handle.clone();
        tauri::async_runtime::spawn_blocking(move || {
            // 将 _guard 移入 blocking 线程，hold 锁直到恢复完成
            let _lock = _guard;
            restore_large_folder_inner(&junction, &target_path_str, &handle)
        })
        .await
        .map_err(|e| format!("恢复线程异常: {}", e))?
    }

    #[cfg(not(windows))]
    { Err("此功能仅支持 Windows 系统".to_string()) }
}

/// 恢复大文件夹的内部逻辑（在后台线程中执行）
fn restore_large_folder_inner(
    junction_path: &std::path::Path,
    target_str: &str,
    app_handle: &AppHandle,
) -> Result<MigrationResult, String> {
    let target_path = PathBuf::from(target_str);

    let restore_result = crate::app_manager::migration::restore_directory_with_progress(
        junction_path,
        &target_path,
        &junction_path.to_string_lossy(),
        app_handle,
    )?;

    // 步骤 4: 更新迁移记录状态
    let junction_str = junction_path.to_string_lossy().to_string();
    if let Err(e) = crate::storage::history::update_migration_record_status(&junction_str, "restored") {
        eprintln!("警告: 更新迁移记录状态失败: {}", e);
    }

    let mut message = format!(
        "恢复成功！文件夹已从 {} 移回 {}（{}）",
        target_str,
        junction_str,
        crate::app_manager::migration::format_bytes(restore_result.restored_size)
    );
    if let Some(warning) = restore_result.cleanup_warning {
        message.push_str(&format!("\n\n{}", warning));
    }

    Ok(MigrationResult {
        success: true,
        message,
        new_path: Some(junction_str),
    })
}

/// 通过 history 记录 ID 恢复大文件夹（供 restore_app 统一入口分发）
/// 与 restore_large_folder 核心逻辑相同，但末尾按 ID 更新 history 记录状态，
/// 确保 MigrationHistory 页面的记录能被正确标记为 restored
pub fn restore_large_folder_by_history(
    history_id: String,
    record: crate::models::MigrationRecord,
    app_handle: AppHandle,
) -> Result<MigrationResult, String> {
    #[cfg(windows)]
    {
        let junction_path = std::path::PathBuf::from(&record.original_path);
        let target_path = std::path::PathBuf::from(&record.target_path);

        if !utils::is_junction(&junction_path) {
            return Ok(MigrationResult {
                success: false,
                message: format!(
                    "原路径 {} 不是目录联接，拒绝恢复以保护数据安全。",
                    record.original_path
                ),
                new_path: None,
            });
        }

        if !target_path.exists() {
            return Ok(MigrationResult {
                success: false,
                message: format!("目标路径不存在: {}，可能已被手动删除", record.target_path),
                new_path: None,
            });
        }
        if crate::storage::history::is_empty_directory(&target_path)? {
            return Ok(MigrationResult {
                success: false,
                message: format!(
                    "目标目录为空，没有可恢复的数据：{}\n\n未创建原目录，请先确认该文件夹是否已被卸载或手动清理。",
                    target_path.display()
                ),
                new_path: None,
            });
        }

        // 进程占用检测
        {
            let mut sys = sysinfo::System::new_all();
            sys.refresh_all();
            let original_lower = record.original_path.to_lowercase();
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
                         恢复前必须关闭相关程序，否则文件移动可能失败并导致数据损坏。",
                        running.join("、")
                    ),
                    new_path: None,
                });
            }
        }

        let restore_result = crate::app_manager::migration::restore_directory_with_progress(
            &junction_path,
            &target_path,
            &record.original_path,
            &app_handle,
        )?;

        // 按 ID 更新 history 记录状态
        if let Err(e) = crate::storage::history::update_record_status_by_id(&history_id, "restored") {
            eprintln!("警告: 更新大文件夹恢复记录状态失败: {}", e);
        }

        let mut message = format!(
            "恢复成功！文件夹已从 {} 移回 {}（{}）",
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
