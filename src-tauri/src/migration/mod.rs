// 应用迁移模块（顶层）
// 负责应用目录迁移、空间校验、进度上报、回滚与历史写入。
// 该能力同时被 folder_manager（大文件夹迁移/恢复）与 storage/history（应用还原）复用，
// 因此作为独立顶层模块，不归属 app_manager。

mod cleanup;
mod copy_engine;
mod danger_rules;
mod links;
mod occupancy;
mod restore;

pub(crate) use restore::restore_directory_with_progress;

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use sysinfo::Disks;
use tauri::Emitter;
use walkdir::WalkDir;

use crate::models::{MigrationRecordType, MigrationResult};
use crate::utils;

// 子模块能力导入（父模块经 use 引入，保持 migrate_app 调用处简洁）
use cleanup::{remove_directory_robust, schedule_remove_on_reboot};
use copy_engine::{build_copy_plan_with_progress, copy_dir_with_progress};
use danger_rules::{check_dangerous_path, DangerLevel};
use links::{
    create_directory_link, create_migration_backup_path, preflight_directory_link,
    restore_source_from_backup, verify_directory_link,
};
use occupancy::check_directory_file_locks;

/// 迁移进度事件（发送到前端）
#[derive(Clone, Serialize)]
pub struct MigrationProgressEvent {
    /// 任务标识（源路径），前端用于区分批量迁移中各任务的进度
    pub task_id: String,
    /// 当前进度百分比 0.0 ~ 100.0
    pub percent: f64,
    /// 当前步骤: counting | copying | verifying | linking | done
    pub step: String,
    /// 描述消息
    pub message: String,
    /// 已复制字节数
    pub copied_size: u64,
    /// 总字节数
    pub total_size: u64,
}

/// 获取指定磁盘的可用空间
/// 使用最长前缀匹配，避免多挂载点场景（如 WSL/Subst 虚拟盘）
/// 回退到错误磁盘
fn get_available_space(path: &Path) -> u64 {
    let disks = Disks::new_with_refreshed_list();
    let path_str = path.to_string_lossy().to_uppercase();

    disks.list()
        .iter()
        .filter_map(|disk| {
            let mount = disk.mount_point().to_string_lossy().to_uppercase();
            let mount_clean = mount.trim_end_matches('\\');
            // 必须匹配完整路径分隔边界，避免 C: 误匹配 CD: 或 C:\Mount\Disk2
            let is_match = path_str == mount_clean
                || (path_str.starts_with(mount_clean)
                    && path_str.as_bytes().get(mount_clean.len()) == Some(&b'\\'));
            if is_match {
                Some((mount_clean.len(), disk.available_space()))
            } else {
                None
            }
        })
        .max_by_key(|(len, _)| *len) // 选最长（最具体）的挂载点匹配
        .map(|(_, space)| space)
        .unwrap_or(0)
}

/// 发送进度事件到前端
pub(crate) fn emit_progress(
    app_handle: &tauri::AppHandle,
    task_id: &str,
    percent: f64,
    step: &str,
    message: &str,
    copied_size: u64,
    total_size: u64,
) {
    let _ = app_handle.emit("migration-progress", MigrationProgressEvent {
        task_id: task_id.to_string(),
        percent,
        step: step.to_string(),
        message: message.to_string(),
        copied_size,
        total_size,
    });
}

/// 格式化字节数用于进度文案，避免前端重复实现同一套展示逻辑。
pub(crate) fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * KB;
    const GB: f64 = 1024.0 * MB;
    let size = bytes as f64;

    if size >= GB {
        format!("{:.2} GB", size / GB)
    } else if size >= MB {
        format!("{:.1} MB", size / MB)
    } else if size >= KB {
        format!("{:.1} KB", size / KB)
    } else {
        format!("{} B", bytes)
    }
}

/// 构建复制计划并持续上报扫描进度，避免大目录扫描期间前端长时间停在 0%。

pub fn migrate_app(
    app_name: String,
    source: String,
    target_parent: String,
    cancel_flag: &Arc<AtomicBool>,
    app_handle: &tauri::AppHandle,
    record_type: MigrationRecordType,
    force_overwrite: bool,
    user_confirmed_warning: bool,
) -> Result<MigrationResult, String> {
    #[cfg(windows)]
    {
        let source_path = Path::new(&source);
        let target_parent_path = Path::new(&target_parent);

        // 步骤 0: 基础验证
        if !source_path.exists() {
            return Ok(MigrationResult {
                success: false,
                message: format!("源路径不存在: {}", source),
                new_path: None,
            });
        }

        if !source_path.is_dir() {
            return Ok(MigrationResult {
                success: false,
                message: "源路径必须是一个目录".to_string(),
                new_path: None,
            });
        }

        // 步骤 0.1: 危险路径分级检测
        // 前端也会做同规则检测，后端作为防绕过兜底防线
        if let Some((level, danger_msg)) = check_dangerous_path(&source) {
            match level {
                DangerLevel::Blocked => {
                    // 绝对拦截，不允许任何绕过
                    return Ok(MigrationResult {
                        success: false,
                        message: danger_msg,
                        new_path: None,
                    });
                }
                DangerLevel::Warning => {
                    // 要求前端显式传 user_confirmed_warning 才放行
                    if !user_confirmed_warning {
                        return Ok(MigrationResult {
                            success: false,
                            message: format!("REQUIRES_WARNING_CONFIRM:{}", danger_msg),
                            new_path: None,
                        });
                    }
                    // 用户已确认，记录日志后继续执行
                    log_warn!("migration", "高风险迁移（用户已确认）: {} — {}", source, danger_msg);
                }
            }
        }

        if !target_parent_path.exists() {
            return Ok(MigrationResult {
                success: false,
                message: format!("目标路径不存在: {}", target_parent),
                new_path: None,
            });
        }

        let folder_name = source_path
            .file_name()
            .ok_or("无法获取源文件夹名称")?
            .to_string_lossy()
            .to_string();

        let target_path = target_parent_path.join(&folder_name);
        let target_path_str = target_path.to_string_lossy().to_string();

        // 防止 target 是 source 的子目录（会导致 WalkDir 无限递归写满磁盘）
        if target_path.starts_with(source_path) {
            return Ok(MigrationResult {
                success: false,
                message: format!(
                    "目标路径不能是源路径的子目录。\n源路径：{}\n目标路径：{}",
                    source, target_path_str
                ),
                new_path: None,
            });
        }

        if target_path.exists() {
            if !force_overwrite {
                // 这里返回内部控制协议，不直接作为用户提示展示：
                // source 是普通目录且 target 也存在时，既可能是失败残留，也可能是用户手动创建的同名目录。
                // 前端会用 TARGET_EXISTS_RETRY 弹出中文覆盖确认，确认后才允许 force_overwrite 清理目标。
                let can_retry_with_user_confirmation = source_path.is_dir()
                    && !crate::utils::is_junction(source_path)
                    && target_path.is_dir();

                let msg = if can_retry_with_user_confirmation {
                    format!("TARGET_EXISTS_RETRY:{}", target_path_str)
                } else {
                    format!("TARGET_EXISTS:{}", target_path_str)
                };

                return Ok(MigrationResult {
                    success: false,
                    message: msg,
                    new_path: None,
                });
            }

            // force_overwrite 安全检查：源路径是否为指向目标路径的 Junction
            // 场景：迁移成功但恢复失败，源路径仍是 Junction 指向目标盘数据
            // 此时删除目标 = 删除唯一数据副本，源 Junction 变成悬空链接
            let source_is_junction_to_target = crate::utils::is_junction(source_path) && {
                crate::utils::get_junction_target(source_path)
                    .map(|t| {
                        t.to_lowercase().trim_end_matches('\\').to_string()
                        == target_path_str.to_lowercase().trim_end_matches('\\').to_string()
                    })
                    .unwrap_or(false)
            };

            if source_is_junction_to_target {
                return Ok(MigrationResult {
                    success: false,
                    message: format!("JUNCTION_LOOP:{}", target_path_str),
                    new_path: None,
                });
            }

            // 安全：源路径非 Junction 或指向不同目标，可安全删除目标残留
            log_warn!("migration", "force_overwrite: 删除残留目标目录 {}", target_path_str);
            fs::remove_dir_all(&target_path)
                .map_err(|e| format!(
                    "无法删除残留目录: {}。请手动删除后重试。原因: {}",
                    target_path_str, e
                ))?;
        }

        // 步骤 0.5: 智能文件占用检测
        // 两种检测互补，都执行：
        //   1. 含 exe（应用目录）→ 进程 exe 路径前缀匹配
        //      拦截正在运行的应用本体（exe 被 Windows 内存映射时独占锁探测检测不到）
        //   2. 所有目录 → 文件独占锁探测
        //      拦截被 explorer/系统服务加载的 DLL（shell extension 等）与被写锁
        //      持有的数据文件——这类文件虽不阻塞复制（读共享），但会导致迁移后
        //      备份目录清理失败，必须在迁移前发现
        emit_progress(app_handle, &source, 0.0, "checking", "正在检测文件占用...", 0, 0);

        // 及时响应取消（大目录 WalkDir 可能耗时较长）
        if cancel_flag.load(Ordering::Relaxed) {
            return Err("用户取消了迁移".to_string());
        }

        let has_exe_in_source = WalkDir::new(source_path)
            .max_depth(5) // 深度5覆盖 Electron/部分游戏的 bin/ 等深层 exe 目录
            .into_iter()
            .filter_map(|e| e.ok())
            .any(|e| {
                e.file_type().is_file()
                    && e.path()
                        .extension()
                        .map(|ext| ext.eq_ignore_ascii_case("exe"))
                        .unwrap_or(false)
            });

        if has_exe_in_source {
            // 应用目录：用进程 exe 路径前缀匹配
            // 拦截正在运行的应用本体（exe 被内存映射时独占锁探测检测不到）
            let mut sys = sysinfo::System::new_all();
            sys.refresh_all();
            let source_lower = source.to_lowercase();
            let running: Vec<String> = sys
                .processes()
                .values()
                .filter_map(|p| {
                    p.exe().and_then(|exe| {
                        if exe
                            .to_string_lossy()
                            .to_lowercase()
                            .starts_with(&source_lower)
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
                        "检测到以下程序正在运行，请关闭后重试：\n{}",
                        running.join("、")
                    ),
                    new_path: None,
                });
            }
        }

        // 无论是否含 exe，都做文件独占锁探测：
        // 应用目录里可能含被 explorer/系统服务加载的 DLL（如 OneDrive 的
        // FileSyncShell64.dll 是 shell extension，explorer 启动即加载，进程
        // exe 前缀匹配检测不到）。这类文件不阻塞复制（读共享），但会导致
        // 迁移后的备份目录清理失败，必须在迁移前拦截。
        let locked_files = check_directory_file_locks(source_path, cancel_flag);
        // 锁探测被取消时其返回值为占位"检测已取消"，此处优先按取消处理，
        // 避免把取消误报成"文件被占用"
        if cancel_flag.load(Ordering::Relaxed) {
            return Err("用户取消了迁移".to_string());
        }
        if !locked_files.is_empty() {
            // 含 exe 时占用源多为被加载的 DLL，提示注销/重启；否则提示关闭程序
            let hint = if has_exe_in_source {
                "被占用的文件通常是 .dll 等被资源管理器或系统服务加载的组件，\n\
                 请注销当前用户或重启系统后重试。"
            } else {
                "请关闭正在使用这些文件的程序（如浏览器、游戏、编辑器等）后重试。"
            };
            return Ok(MigrationResult {
                success: false,
                message: format!(
                    "目录中有文件正被其他程序占用，无法迁移。\n\n\
                     被占用的文件：\n{}\n\n{}",
                    locked_files
                        .iter()
                        .map(|f| format!("  • {}", f))
                        .collect::<Vec<_>>()
                        .join("\n"),
                    hint
                ),
                new_path: None,
            });
        }

        // 步骤 0.5.1：及时响应取消（sysinfo 刷新可能较慢）
        if cancel_flag.load(Ordering::Relaxed) {
            return Err("用户取消了迁移".to_string());
        }

        // 步骤 1: 构建复制计划 + 空间检查
        // 复制前必须知道总大小；这里直接产出复制计划，避免后续复制阶段再次遍历整棵目录。
        let copy_plan = match build_copy_plan_with_progress(
            source_path,
            &target_path,
            &source,
            cancel_flag,
            app_handle,
        ) {
            Ok(plan) => plan,
            Err(e) => {
                let _ = remove_directory_robust(&target_path);
                return Ok(MigrationResult {
                    success: false,
                    message: e,
                    new_path: None,
                });
            }
        };
        let source_size = copy_plan.total_size;

        let available_space = get_available_space(target_parent_path);
        // 1.2× 源大小 + 100MB 最小预留，避免目标盘被填满
        let required_space = (source_size as f64 * 1.2) as u64 + 100 * 1024 * 1024;

        if available_space < required_space {
            return Ok(MigrationResult {
                success: false,
                message: format!(
                    "目标磁盘空间不足。需要: {:.2} GB，可用: {:.2} GB",
                    required_space as f64 / 1024.0 / 1024.0 / 1024.0,
                    available_space as f64 / 1024.0 / 1024.0 / 1024.0
                ),
                new_path: None,
            });
        }

        // 步骤 1.1：及时响应取消（get_dir_size_safe 对大目录可能耗时较长）
        if cancel_flag.load(Ordering::Relaxed) {
            return Err("用户取消了迁移".to_string());
        }

        // 步骤 1.5：同盘迁移走 rename 快路径（原子操作，毫秒级，零数据风险）
        let source_drive = source.chars().next().map(|c| c.to_ascii_uppercase());
        let target_drive = target_path_str.chars().next().map(|c| c.to_ascii_uppercase());
        if source_drive == target_drive && source_drive.is_some() {
            emit_progress(app_handle, &source, 50.0, "copying",
                "同盘迁移，正在移动目录...", source_size, source_size);

            let rename_succeeded = if let Err(e) = fs::rename(source_path, &target_path) {
                // os error 17 = ERROR_NOT_SAME_DEVICE，跨盘 rename 的预期失败，回退到复制模式
                let is_cross_device = e.raw_os_error() == Some(17);
                if is_cross_device {
                    log_warn!("migration", "同盘 rename 跨设备失败，回退到复制模式: {}", e);
                    false
                } else {
                    // 其他错误（目标已存在、权限不足等）不能静默回退，直接报错
                    return Ok(MigrationResult {
                        success: false,
                        message: format!(
                            "移动目录失败（错误码 {}）：{}\n\
                             如目标路径已存在残留目录，请手动删除后重试。",
                            e.raw_os_error().unwrap_or(0), e
                        ),
                        new_path: None,
                    });
                }
            } else {
                true
            };

            if rename_succeeded {
                emit_progress(app_handle, &source, 93.0, "linking",
                    "正在创建目录链接...", source_size, source_size);

                match create_directory_link(&target_path, source_path) {
                    Ok(_) => {
                        let is_app = matches!(record_type, MigrationRecordType::App);
                        if let Err(e) = crate::storage::history::add_migration_record(
                            &app_name, &source, &target_path_str, source_size, record_type,
                        ) {
                            log_warn!("migration", "保存迁移记录失败: {}", e);
                        }
                        if is_app {
                            crate::storage::migrated_app_metadata::add_migrated_app(
                                &app_name, &source, &target_path_str,
                            );
                        }
                        emit_progress(app_handle, &source, 100.0, "done",
                            "迁移完成", source_size, source_size);
                        return Ok(MigrationResult {
                            success: true,
                            message: format!("迁移成功！应用已从 {} 迁移到 {}", source, target_path_str),
                            new_path: Some(target_path_str),
                        });
                    }
                    Err(symlink_err) => {
                        let rollback = fs::rename(&target_path, source_path);
                        match rollback {
                            Ok(_) => {
                                return Ok(MigrationResult {
                                    success: false,
                                    message: format!(
                                        "{}\n\n您的数据已完整恢复到：{}",
                                        symlink_err, source
                                    ),
                                    new_path: None,
                                });
                            }
                            Err(rename_back_err) => {
                                orbit_log!("ERROR", "migration",
                                    "同盘快路径：链接创建失败且 rename 回滚失败。link_err={}, rename_err={}, data_at={}",
                                    symlink_err, rename_back_err, target_path_str
                                );
                                return Ok(MigrationResult {
                                    success: false,
                                    message: format!(
                                        "SYMLINK_FAILED_DATA_AT_TARGET:{target}\n\
                                         创建目录链接失败，自动回滚也未成功。\n\n\
                                         ⚠️ 您的数据完整保存在新位置：{target}\n\n\
                                         请手动执行：mklink /J \"{source}\" \"{target}\"\n\n\
                                         链接失败原因：{symlink_err}",
                                        target = target_path_str,
                                        source = source,
                                        symlink_err = symlink_err,
                                    ),
                                    new_path: Some(target_path_str),
                                });
                            }
                        }
                    }
                }
            }
            // 仅跨盘错误走到这里，继续执行下方的全量复制流程
        }

        // 步骤 2: 复制文件（带进度上报和取消支持）
        // 先创建目标目录的父目录结构
        fs::create_dir_all(&target_path)
            .map_err(|e| format!("创建目标目录失败: {}", e))?;

        let (total_size, skipped_size) = match copy_dir_with_progress(
            copy_plan, &source, cancel_flag, app_handle,
        ) {
            Ok((total, skipped)) => (total, skipped),
            Err(e) => {
                // 取消或复制错误：清理已创建的目标目录，避免残留半成品
                let _ = remove_directory_robust(&target_path);
                // 细化错误消息：拒绝访问通常意味着文件被内核映射或系统进程独占
                let user_message = if e.contains("拒绝访问") || e.contains("Access is denied")
                    || e.contains("os error 5") || e.contains("permission denied")
                {
                    format!(
                        "复制失败：部分文件被 Windows 系统内核映射，无法在运行时复制。\n\n\
                         常见原因：开发工具（Visual Studio、JetBrains 等）的编译器、\
                         语言服务进程（MSBuild、VBCSCompiler、ServiceHub 等）仍在后台运行。\n\n\
                         解决方案：\n\
                         1. 完全退出所有 IDE 实例（包括系统托盘图标）\n\
                         2. 打开任务管理器，结束所有 MSBuild、VBCSCompiler、\
                         ServiceHub、dotnet 相关进程\n\
                         3. 等待 10 秒后重试\n\n\
                         原始错误：{}", e
                    )
                } else {
                    e
                };
                return Ok(MigrationResult {
                    success: false,
                    message: user_message,
                    new_path: None,
                });
            }
        };

        // 步骤 2.1: 空目录安全检查
        // source_size > 0 说明预扫描时目录有内容，但 WalkDir 遍历结果为空，
        // 意味着遍历过程遇到了大面积的权限拒绝或重解析点异常（WalkDir 静默跳过）。
        // 若此时继续，校验通过 → 源目录被删除 → 空链接被创建 → 数据丢失。
        if source_size > 0 && total_size == 0 {
            let _ = remove_directory_robust(&target_path);
            return Ok(MigrationResult {
                success: false,
                message: format!(
                    "目录遍历失败：源目录 {} 预扫描有 {} 字节内容，但遍历时未发现任何可复制文件。\n\
                     这通常是因为目录权限限制或文件系统特殊属性导致的。\n\
                     建议：以管理员身份运行本程序后重试。",
                    source,
                    source_size
                ),
                new_path: None,
            });
        }

        // 步骤 3: 完整性校验
        // 使用实际复制量（去除跳过文件）作为预期基准，避免权限拒绝文件导致误报
        emit_progress(app_handle, &source, 90.0, "verifying", "正在校验文件完整性...", source_size, source_size);

        let target_size = utils::get_dir_size_safe(&target_path);
        let expected_target = total_size.saturating_sub(skipped_size);
        // 容差 = 跳过体积 + 1MB 元数据浮动，但不超过源大小的 5%
        // 避免大面积权限拒绝（如 DRM 保护文件）时容差过宽导致漏检
        let max_tolerance = (total_size as f64 * 0.05) as u64 + 1024 * 1024;
        let tolerance = skipped_size.min(max_tolerance) + 1024 * 1024;

        if (target_size as i64 - expected_target as i64).abs() > tolerance as i64 {
            let _ = remove_directory_robust(&target_path);
            return Ok(MigrationResult {
                success: false,
                message: format!(
                    "文件完整性校验失败。预期: {} 字节，实际: {} 字节，跳过: {} 字节",
                    expected_target, target_size, skipped_size
                ),
                new_path: None,
            });
        }

        // 步骤 4：原子切换源目录
        // 不能直接 remove_dir_all：目录删除不是事务操作，遇到 uTools 等被占用文件时，
        // Windows 可能已经删掉部分文件才返回失败。先改名为同父目录临时备份，失败时
        // 源目录仍保持完整；链接确认成功后才清理备份，整个切换过程不会暴露半目录。
        emit_progress(app_handle, &source, 93.0, "linking", "正在创建目录链接...", source_size, source_size);

        if let Err(e) = preflight_directory_link(&target_path, source_path) {
            // 预检失败时源目录仍在，只清理本次创建的目标副本。
            let _ = remove_directory_robust(&target_path);
            return Ok(MigrationResult {
                success: false,
                message: format!(
                    "创建目录链接预检失败，迁移中止。\n\
                     已自动清理目标副本，原数据完好无损。\n\n\
                     失败原因：{}\n\n\
                     建议：以管理员身份运行本程序后重试。",
                    e
                ),
                new_path: None,
            });
        }

        let backup_path = match create_migration_backup_path(source_path) {
            Ok(path) => path,
            Err(e) => {
                let _ = remove_directory_robust(&target_path);
                return Ok(MigrationResult {
                    success: false,
                    message: format!(
                        "迁移中止：无法准备安全切换。\n\n{}\n\n原数据仍完整保留。",
                        e
                    ),
                    new_path: None,
                });
            }
        };

        if let Err(e) = fs::rename(source_path, &backup_path) {
            // 改名失败通常是目录仍被程序占用；此时源目录从未被删除，目标副本可安全清理。
            let _ = remove_directory_robust(&target_path);
            return Ok(MigrationResult {
                success: false,
                message: format!(
                    "迁移中止：原目录仍被其他程序占用，未删除任何源文件。\n\
                     已自动清理目标副本，原数据完好无损。\n\n\
                     失败路径：{}\n\
                     失败原因：{}\n\n\
                     常见解决方案：\n\
                     • 文件被程序占用 → 关闭相关程序（浏览器、游戏、编辑器等）后重试\n\
                     • 权限不足 → 以管理员身份运行本程序后重试\n\
                     • 只读保护 → 以管理员身份运行后程序会自动解除只读属性",
                    source, e
                ),
                new_path: None,
            });
        }

        // 步骤 5：创建并确认目录链接（Junction 或软链接）
        match create_directory_link(&target_path, source_path) {
            Ok(_) => {
                if let Err(verify_error) = verify_directory_link(source_path, &target_path) {
                    let restore_result = restore_source_from_backup(source_path, &backup_path, &target_path);
                    let restore_message = match restore_result {
                        Ok(()) => {
                            let _ = remove_directory_robust(&target_path);
                            format!("原数据已完整恢复到：{}", source)
                        }
                        Err(restore_error) => format!(
                            "自动恢复未完成。原数据备份仍保留在：{}；恢复原因：{}",
                            backup_path.display(),
                            restore_error
                        ),
                    };
                    return Ok(MigrationResult {
                        success: false,
                        message: format!(
                            "目录链接确认失败，迁移已中止。\n\n{}\n\n{}",
                            verify_error, restore_message
                        ),
                        new_path: Some(target_path_str.clone()),
                    });
                }

                // 链接已经确认指向完整目标，之后清理临时备份不会影响实际运行数据。
                // 清理失败（多为 explorer/服务加载的 DLL 被占用）时迁移仍成功：
                // 先尝试标记重启后自动删除（需管理员权限），再给出分级提示。
                let cleanup_warning = remove_directory_robust(&backup_path)
                    .err()
                    .map(|e| {
                        if schedule_remove_on_reboot(&backup_path) {
                            format!(
                                "临时备份目录清理失败（文件被占用），已安排在下次重启时自动删除。\n\
                                 残留目录：{}。",
                                backup_path.display()
                            )
                        } else {
                            format!(
                                "临时备份目录清理失败，仍保留在：{}。\n\
                                 原因：{}。\n\
                                 该目录已被扫描过滤，不会被识别为新应用；\n\
                                 其中被占用的文件通常在注销或重启系统后释放，届时可手动删除此目录。",
                                backup_path.display(),
                                e
                            )
                        }
                    });

                // 步骤 6: 写入迁移历史
                let is_app = matches!(record_type, MigrationRecordType::App);
                if let Err(e) = crate::storage::history::add_migration_record(
                    &app_name,
                    &source,
                    &target_path_str,
                    source_size,
                    record_type,
                ) {
                    log_warn!("migration", "保存迁移记录失败: {}", e);
                }
                // 写入兜底元数据：仅对应用类型记录，确保扫描器遗漏时仍能识别
                if is_app {
                    crate::storage::migrated_app_metadata::add_migrated_app(
                        &app_name, &source, &target_path_str,
                    );
                }

                emit_progress(app_handle, &source, 100.0, "done", "迁移完成", source_size, source_size);

                let success_msg = match cleanup_warning {
                    Some(warning) => format!(
                        "迁移成功！应用已从 {} 迁移到 {}\n\n⚠️ {}",
                        source, target_path_str, warning
                    ),
                    None => format!("迁移成功！应用已从 {} 迁移到 {}", source, target_path_str),
                };

                Ok(MigrationResult {
                    success: true,
                    message: success_msg,
                    new_path: Some(target_path_str),
                })
            }
            Err(symlink_err) => {
                // 链接创建失败时，源目录仍在临时备份中；先恢复原目录，再清理目标副本。
                match restore_source_from_backup(source_path, &backup_path, &target_path) {
                    Ok(()) => {
                        let _ = remove_directory_robust(&target_path);
                        Ok(MigrationResult {
                            success: false,
                            message: format!(
                                "{}\n\n您的数据已完整恢复到：{}",
                                symlink_err, source
                            ),
                            new_path: None,
                        })
                    }
                    Err(restore_err) => {
                        // 恢复失败时保留目标副本和备份目录，两个副本都不再自动删除，避免数据损失。
                        orbit_log!(
                            "ERROR", "migration",
                            "链接失败且源目录恢复失败。link_err={}, restore_err={}, data_at={}, backup_at={}",
                            symlink_err, restore_err, target_path_str, backup_path.display()
                        );
                        Ok(MigrationResult {
                            success: false,
                            message: format!(
                                "SYMLINK_FAILED_DATA_AT_TARGET:{target}\n\
                                 创建目录链接失败，自动恢复也未完成。\n\n\
                                 ⚠️ 数据副本仍完整保存在：{target}\n\
                                 原目录备份仍保留在：{backup}\n\n\
                                 请勿删除上述任一目录，关闭占用程序后再手动恢复。\n\n\
                                 链接失败原因：{symlink_err}\n\
                                 恢复失败原因：{restore_err}",
                                target = target_path_str,
                                backup = backup_path.display(),
                                symlink_err = symlink_err,
                                restore_err = restore_err,
                            ),
                            new_path: Some(target_path_str.clone()),
                        })
                    }
                }
            }
        }
    }

    #[cfg(not(windows))]
    {
        Ok(MigrationResult {
            success: false,
            message: "迁移功能仅支持 Windows 系统".to_string(),
            new_path: None,
        })
    }
}
