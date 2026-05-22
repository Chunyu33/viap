// 大文件夹发现与管理模块
//
// 负责系统文件夹、应用数据文件夹和自定义文件夹的扫描、
// 迁移和恢复，以及应用数据模板的管理

use std::path::PathBuf;
use std::fs;
#[cfg(windows)]
use std::os::windows::fs::symlink_dir;
use std::sync::atomic::Ordering;
use fs_extra::dir::{move_dir, CopyOptions};

use tauri::{AppHandle, Emitter};

use crate::models::*;
use crate::utils;
use crate::storage::data_dir;
use crate::storage::data_dir::ensure_data_dir;

// ============================================================================
// 应用数据模板管理
// ============================================================================

/// 默认内置模板列表（与旧版硬编码一致，确保向后兼容）
pub fn default_app_data_templates() -> Vec<AppDataTemplate> {
    vec![
        AppDataTemplate {
            id: "wechat".to_string(), display_name: "微信".to_string(),
            icon_id: "wechat".to_string(),
            process_names: vec!["WeChat.exe".to_string()], path: None,
        },
        AppDataTemplate {
            id: "wxwork".to_string(), display_name: "企业微信".to_string(),
            icon_id: "wxwork".to_string(),
            process_names: vec!["WXWork.exe".to_string()], path: None,
        },
        AppDataTemplate {
            id: "qq".to_string(), display_name: "QQ".to_string(),
            icon_id: "qq".to_string(),
            process_names: vec!["QQ.exe".to_string()], path: None,
        },
        AppDataTemplate {
            id: "dingtalk".to_string(), display_name: "钉钉".to_string(),
            icon_id: "dingtalk".to_string(),
            process_names: vec!["DingTalk.exe".to_string()], path: None,
        },
        AppDataTemplate {
            id: "feishu".to_string(), display_name: "飞书".to_string(),
            icon_id: "feishu".to_string(),
            process_names: vec!["Lark.exe".to_string(), "Feishu.exe".to_string()], path: None,
        },
        AppDataTemplate {
            id: "chrome_cache".to_string(), display_name: "Chrome 缓存".to_string(),
            icon_id: "chrome_cache".to_string(),
            process_names: vec!["chrome.exe".to_string()], path: None,
        },
        AppDataTemplate {
            id: "edge_cache".to_string(), display_name: "Edge 缓存".to_string(),
            icon_id: "edge_cache".to_string(),
            process_names: vec!["msedge.exe".to_string()], path: None,
        },
        AppDataTemplate {
            id: "vscode_extensions".to_string(), display_name: "VS Code 扩展".to_string(),
            icon_id: "vscode_extensions".to_string(),
            process_names: vec!["code.exe".to_string()], path: None,
        },
        AppDataTemplate {
            id: "npm_global".to_string(), display_name: "npm 全局包".to_string(),
            icon_id: "npm_global".to_string(),
            process_names: vec![], path: None,
        },
    ]
}

/// 获取应用数据模板（Tauri 命令，供设置页展示和编辑）
#[tauri::command]
pub fn get_app_data_templates() -> Result<Vec<AppDataTemplate>, String> {
    Ok(load_app_data_templates())
}

/// 加载应用数据模板（文件不存在时自动创建默认模板）
pub fn load_app_data_templates() -> Vec<AppDataTemplate> {
    let path = utils::app_data_templates_path(&ensure_data_dir());
    if !path.exists() {
        let defaults = default_app_data_templates();
        let json = serde_json::to_string_pretty(&defaults).unwrap_or_default();
        let _ = std::fs::write(&path, &json);
        return defaults;
    }
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<AppDataTemplate>>(&s).ok())
        .unwrap_or_else(default_app_data_templates)
}

/// 保存应用数据模板
#[tauri::command]
pub fn save_app_data_templates(templates: Vec<AppDataTemplate>) -> Result<(), String> {
    let path = utils::app_data_templates_path(&ensure_data_dir());
    let json = serde_json::to_string_pretty(&templates)
        .map_err(|e| format!("序列化模板失败: {}", e))?;
    std::fs::write(&path, &json)
        .map_err(|e| format!("写入模板文件失败: {}", e))?;
    Ok(())
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
/// 注意：返回时 size 均为 0，前端需随后调用 start_folder_size_scan 触发异步大小计算。
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
) -> Result<(), String> {
    compute_folder_sizes_async(app_handle, folders);
    Ok(())
}

/// 后台异步计算各文件夹大小并通过事件推送
/// 始终推送事件（即使大小为 0），避免前端因缺少事件而永久显示 "--"
/// Junction 文件夹计算其目标目录的实际大小
fn compute_folder_sizes_async(app_handle: AppHandle, folders: Vec<LargeFolder>) {
    std::thread::spawn(move || {
        for folder in &folders {
            if !folder.exists { continue; }
            // Junction 文件夹计算目标目录大小，非 Junction 计算自身大小
            let path = if folder.is_junction {
                match &folder.junction_target {
                    Some(target) => PathBuf::from(target),
                    None => continue,
                }
            } else {
                PathBuf::from(&folder.path)
            };
            let size = utils::get_folder_size(&path);
            let _ = app_handle.emit("large-folder-size", LargeFolderSizeEvent {
                folder_id: folder.id.clone(), size,
            });
        }
    });
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

    state.cancel_flag.store(false, Ordering::SeqCst);
    let cancel_flag = state.cancel_flag.clone();
    let handle = app_handle.clone();

    let result = tauri::async_runtime::spawn_blocking(move || {
        crate::app_manager::migration::migrate_app(
            folder_name, source_path, target_dir, &cancel_flag, &handle,
            MigrationRecordType::LargeFolder, false,
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

        // 获取全局恢复锁，防止与 restore_app 或其他恢复任务并发
        let _guard = match utils::try_acquire_restore_lock() {
            Ok(guard) => guard,
            Err(msg) => return Ok(MigrationResult {
                success: false, message: msg, new_path: None,
            }),
        };

        let target_path_str = target_path.to_string_lossy().to_string();
        tauri::async_runtime::spawn_blocking(move || {
            // 将 _guard 移入 blocking 线程，hold 锁直到恢复完成
            let _lock = _guard;
            restore_large_folder_inner(&junction, &target_path_str)
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
) -> Result<MigrationResult, String> {
    let target_path = PathBuf::from(target_str);

    // 步骤 1: 还原前检查目标盘空间
    let file_size = utils::get_dir_size_safe(&target_path);
    let original_parent = junction_path.parent()
        .ok_or("无法获取原路径的父目录")?;
    utils::check_disk_space_for_restore(original_parent, file_size)?;

    // 步骤 2: 删除 Junction
    fs::remove_dir(junction_path).map_err(|e| {
        format!("无法删除符号链接: {}", e)
    })?;

    // 步骤 3: 移动文件夹回原位置
    let mut options = CopyOptions::new();
    options.overwrite = false;
    options.copy_inside = false;

    move_dir(&target_path, original_parent, &options).map_err(|move_err| {
        // 尝试在原位置重建 Junction，恢复用户对数据的访问路径
        let rollback_label = {
            #[cfg(windows)]
            {
                match symlink_dir(&target_path, junction_path) {
                    Ok(_) => format!(
                        "已自动恢复符号链接，数据仍在: {}",
                        target_path.display()
                    ),
                    Err(rb_err) => format!(
                        "且无法恢复符号链接 ({})。数据仍安全保存在: {}，请手动移回: {}",
                        rb_err, target_path.display(), junction_path.display()
                    ),
                }
            }
            #[cfg(not(windows))]
            { String::new() }
        };
        format!("移动文件夹失败: {}。{}", move_err, rollback_label)
    })?;

    // 步骤 4: 更新迁移记录状态
    let junction_str = junction_path.to_string_lossy().to_string();
    if let Err(e) = crate::storage::history::update_migration_record_status(&junction_str, "restored") {
        eprintln!("警告: 更新迁移记录状态失败: {}", e);
    }

    Ok(MigrationResult {
        success: true,
        message: format!(
            "恢复成功！文件夹已从 {} 移回 {}",
            target_str, junction_str
        ),
        new_path: Some(junction_str),
    })
}

/// 通过 history 记录 ID 恢复大文件夹（供 restore_app 统一入口分发）
/// 与 restore_large_folder 核心逻辑相同，但末尾按 ID 更新 history 记录状态，
/// 确保 MigrationHistory 页面的记录能被正确标记为 restored
pub fn restore_large_folder_by_history(
    history_id: String,
    record: crate::models::MigrationRecord,
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

        // 还原前空间检查
        let file_size = utils::get_dir_size_safe(&target_path);
        let original_parent = junction_path.parent()
            .ok_or("无法获取原路径的父目录")?;
        utils::check_disk_space_for_restore(original_parent, file_size)?;

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

        // 删除 Junction
        fs::remove_dir(&junction_path).map_err(|e| {
            format!("无法删除目录联接: {}", e)
        })?;

        // 移动文件回原位置
        let mut options = CopyOptions::new();
        options.overwrite = false;
        options.copy_inside = false;

        move_dir(&target_path, original_parent, &options).map_err(|e| {
            let target_intact = target_path.exists()
                && std::fs::read_dir(&target_path)
                    .map(|mut d| d.next().is_some())
                    .unwrap_or(false);

            if target_intact {
                let _ = symlink_dir(&target_path, &junction_path);
                format!(
                    "移动文件失败: {}。\n已恢复链接，数据完好保存在：{}\n请关闭相关程序后重试。",
                    e, target_path.display()
                )
            } else {
                format!(
                    "严重警告：移动文件过程中发生错误（{}），文件处于中间状态。\n\n\
                     部分文件已移至：{}\n\
                     剩余文件在：{}\n\n\
                     请手动将 {} 目录下所有内容合并到 {}，\n\
                     然后删除空的 {} 目录。",
                    e,
                    record.original_path,
                    record.target_path,
                    record.target_path,
                    record.original_path,
                    record.target_path,
                )
            }
        })?;

        // 防御性清理 target 残留
        if target_path.exists() {
            let _ = fs::remove_dir_all(&target_path);
        }

        // 按 ID 更新 history 记录状态
        if let Err(e) = crate::storage::history::update_record_status_by_id(&history_id, "restored") {
            eprintln!("警告: 更新大文件夹恢复记录状态失败: {}", e);
        }

        Ok(MigrationResult {
            success: true,
            message: format!(
                "恢复成功！文件夹已从 {} 移回 {}",
                record.target_path, record.original_path
            ),
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
