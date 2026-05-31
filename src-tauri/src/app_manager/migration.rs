// 应用迁移模块
// 负责应用目录迁移、空间校验、进度上报、回滚与历史写入

use std::fs;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use sysinfo::Disks;
use tauri::Emitter;
use walkdir::WalkDir;

use crate::models::{MigrationRecordType, MigrationResult};
use crate::utils;

#[cfg(windows)]
use std::os::windows::fs::symlink_dir;

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
fn emit_progress(
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


/// 分块复制单个文件，在每 64KB 块之间检查取消标志
/// 避免大文件（数 GB）的 fs::copy 阻塞期间无法取消和上报进度
/// 权限拒绝时中断迁移（步骤 0.5 已做预检，此处不应再出现被锁文件）
fn copy_file_with_cancel(
    src: &Path,
    dest: &Path,
    cancel_flag: &Arc<AtomicBool>,
) -> Result<u64, String> {
    // 被锁文件：步骤 0.5 已做预检，此处出现说明文件在复制过程中被新进程锁定，直接中断
    let file = match fs::File::open(src) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            // 步骤 0.5 已做预检，到这里说明文件在复制过程中被新进程锁定
            // 不能静默跳过（会导致数据不完整），直接中断迁移
            return Err(format!(
                "复制过程中文件被程序占用: {}\n请关闭相关程序后重试。",
                src.display()
            ));
        }
        Err(e) => return Err(format!("打开源文件失败 {}: {}", src.display(), e)),
    };
    let file_size = file.metadata()
        .map_err(|e| format!("读取文件元数据失败 {}: {}", src.display(), e))?
        .len();

    // 小文件（< 1MB）直接使用 fs::copy，免去分块开销
    if file_size < 1024 * 1024 {
        if let Err(e) = fs::copy(src, dest) {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                return Err(format!(
                    "复制过程中文件被程序占用: {}\n请关闭相关程序后重试。",
                    src.display()
                ));
            }
            return Err(format!("复制文件失败 {}: {}", src.display(), e));
        }
        return Ok(file_size);
    }

    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let dest_file = fs::File::create(dest)
        .map_err(|e| format!("创建目标文件失败 {}: {}", dest.display(), e))?;
    let mut writer = BufWriter::with_capacity(64 * 1024, dest_file);
    let mut buffer = [0u8; 64 * 1024];
    let mut copied: u64 = 0;

    loop {
        if cancel_flag.load(Ordering::Relaxed) {
            // 删除未完成的目标文件，避免残留
            let _ = fs::remove_file(dest);
            return Err("用户取消了迁移".to_string());
        }
        let bytes_read = reader.read(&mut buffer)
            .map_err(|e| format!("读取文件失败 {} (已复制 {}/{}): {}", src.display(), copied, file_size, e))?;
        if bytes_read == 0 {
            break;
        }
        writer.write_all(&buffer[..bytes_read])
            .map_err(|e| format!("写入文件失败 {}: {}", dest.display(), e))?;
        copied += bytes_read as u64;
    }
    writer.flush()
        .map_err(|e| format!("刷新文件缓冲区失败 {}: {}", dest.display(), e))?;

    Ok(copied)
}

/// 带进度上报和取消支持的文件复制
///
/// 替代 fs_extra::copy_items，逐个文件复制以便：
/// 1. 在每个文件 / 每 64KB 之间检查取消标志
/// 2. 按实际复制量上报进度百分比
///
/// 返回 (总文件大小, 因权限拒绝跳过的字节数)
fn copy_dir_with_progress(
    source: &Path,
    target: &Path,
    task_id: &str,
    cancel_flag: &Arc<AtomicBool>,
    app_handle: &tauri::AppHandle,
) -> Result<(u64, u64), String> {
    // 阶段 1：遍历统计文件列表和总大小
    emit_progress(app_handle, task_id, 0.0, "counting", "正在扫描文件...", 0, 0);

    let mut file_list: Vec<(PathBuf, PathBuf, u64)> = Vec::new();
    let mut total_size: u64 = 0;

    for entry in WalkDir::new(source).into_iter().filter_map(|e| e.ok()) {
        if cancel_flag.load(Ordering::Relaxed) {
            return Err("用户取消了迁移".to_string());
        }
        if entry.file_type().is_file() {
            let rel_path = entry.path().strip_prefix(source)
                .map_err(|e| format!("路径解析失败: {}", e))?;
            let dest = target.join(rel_path);
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            total_size += size;
            file_list.push((entry.path().to_path_buf(), dest, size));
        }
    }

    if total_size == 0 {
        emit_progress(app_handle, task_id, 100.0, "copying", "源目录为空，跳过复制", 0, 0);
        return Ok((0, 0));
    }

    // 阶段 2：逐个复制文件，上报进度
    let total_files = file_list.len() as u64;
    let mut copied_size: u64 = 0;
    let mut skipped_size: u64 = 0;
    let mut last_report_pct: u64 = 0;

    for (idx, (src, dest, size)) in file_list.iter().enumerate() {
        if cancel_flag.load(Ordering::Relaxed) {
            return Err("用户取消了迁移".to_string());
        }

        // 创建目标父目录
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("创建目录失败 {}: {}", parent.display(), e))?;
        }

        // 使用分块复制替代 fs::copy，确保大文件复制期间仍可取消
        // 返回值：实际复制的字节数，权限拒绝时返回 0
        let actually_copied = copy_file_with_cancel(src, dest, cancel_flag)?;
        if actually_copied == 0 && *size > 0 {
            // 权限拒绝跳过的文件，记录其大小用于完整性校验容差
            skipped_size += size;
        }

        copied_size += size;

        // 每 1% 或每 50 个文件上报一次进度（避免过于频繁的事件）
        let current_pct = if total_size > 0 {
            ((copied_size as f64 / total_size as f64) * 100.0) as u64
        } else {
            100
        };

        if current_pct > last_report_pct || idx as u64 % 50 == 0 || idx == file_list.len() - 1 {
            last_report_pct = current_pct;
            emit_progress(
                app_handle,
                task_id,
                current_pct as f64,
                "copying",
                &format!("正在复制文件 ({}/{})", idx + 1, total_files),
                copied_size,
                total_size,
            );
        }
    }

    Ok((total_size, skipped_size))
}

/// 检测目录内文件是否被其他进程独占持有（适用于数据目录，如浏览器缓存）
///
/// 原理：以独占模式（FILE_SHARE_NONE）尝试打开每个文件。
/// 若其他进程持有写锁（如 Chrome 正在写 Cache），此调用返回
/// ERROR_SHARING_VIOLATION(32) 或 ERROR_ACCESS_DENIED(5)。
///
/// 注意：此方法不适用于应用目录——exe/dll 被内存映射时不阻塞独占打开，
/// 应用目录需用进程 exe 路径匹配检测。
///
/// 返回：被占用文件的相对路径列表，最多 10 条；空列表表示无占用。
#[cfg(windows)]
fn check_directory_file_locks(dir: &Path) -> Vec<String> {
    use std::os::windows::fs::OpenOptionsExt;

    let mut locked_files: Vec<String> = Vec::new();

    for entry in WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        let path = entry.path();

        // FILE_SHARE_NONE = 0，独占打开。若其他进程持有该文件句柄则失败。
        let result = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(path);

        if let Err(e) = result {
            let os_err = e.raw_os_error().unwrap_or(0);
            // 32 = ERROR_SHARING_VIOLATION（文件被其他进程打开且不允许共享）
            // 5  = ERROR_ACCESS_DENIED（无访问权限，通常也意味着被占用）
            if os_err == 32 || os_err == 5 {
                let rel = path
                    .strip_prefix(dir)
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| path.to_string_lossy().to_string());
                locked_files.push(rel);
                if locked_files.len() >= 10 {
                    locked_files.push("...（更多文件被占用）".to_string());
                    return locked_files;
                }
            }
            // 其他错误（文件消失等竞态）忽略，不视为占用
        }
    }

    locked_files
}

/// 强制删除目录，处理只读文件导致 PermissionDenied 的情况
/// Windows 上部分目录（Shell 已知文件夹、Chrome 缓存等）包含只读文件，
/// fs::remove_dir_all 直接调用会因权限不足失败。
/// 此函数先遍历清除只读属性，再逐个删除文件和子目录。
fn remove_directory_robust(path: &Path) -> Result<(), std::io::Error> {
    match fs::remove_dir_all(path) {
        Ok(_) => return Ok(()),
        Err(e) if e.kind() != std::io::ErrorKind::PermissionDenied => return Err(e),
        Err(_) => {}
    }
    // 因只读文件导致失败：遍历清除只读属性后逐个删除
    for entry in WalkDir::new(path).contents_first(true).into_iter().filter_map(|e| e.ok()) {
        let entry_path = entry.path();
        // 清除只读属性，否则 remove_file / remove_dir 会失败
        if let Ok(mut perms) = fs::metadata(entry_path).map(|m| m.permissions()) {
            perms.set_readonly(false);
            let _ = fs::set_permissions(entry_path, perms);
        }
        if entry_path.is_file() || entry_path.is_symlink() {
            // 文件删除失败不能忽略：说明文件仍被进程持有，整个删除操作必须中止
            // 否则源目录删除不完整，后续 symlink_dir 因目标已存在而失败
            if let Err(e) = fs::remove_file(entry_path) {
                return Err(e);
            }
        } else if entry_path.is_dir() && entry_path != path {
            // 子目录删除失败可忽略（contents_first 保证子项先删，空目录才轮到自己）
            let _ = fs::remove_dir(entry_path);
        }
    }
    fs::remove_dir(path)
}

/// 危险路径检测
///
/// 对源路径做黑名单匹配，拦截以下三类不可迁移的目录：
///
/// 1. **系统核心目录**：Windows / Program Files / System32 等，迁移会导致系统崩溃
/// 2. **系统级浏览器**：Edge / Chrome 安装目录。
///    Edge 的 MicrosoftEdgeUpdate 服务会把 Junction 识别为"损坏安装"并覆盖；
///    Chromium 把安装路径写死进扩展签名，路径变更后所有插件报损坏。
/// 3. **GPU / 显卡驱动目录**：NVIDIA / AMD / Intel 驱动深度注册进系统服务，
///    迁移后驱动服务找不到 DLL，轻则降级到基本显示适配器，重则蓝屏。
///
/// 返回 Some(错误消息) 表示命中黑名单，None 表示安全。
fn check_dangerous_path(source: &str) -> Option<String> {
    let source_lower = source.to_lowercase();
    // 统一转为正斜杠，兼容用户粘贴的混合路径
    let source_normalized = source_lower.replace('/', "\\");

    // ── 规则表 ──────────────────────────────────────────────────────────────
    // 每条规则：(匹配片段, 分类标签, 用户可见原因)
    // 匹配逻辑：source_normalized 包含该片段即命中
    let rules: &[(&str, &str, &str)] = &[
        // 系统核心目录
        (r"c:\windows",             "系统目录", "Windows 系统目录"),
        (r"c:\program files\windowsapps", "系统目录", "Windows 应用商店目录"),
        (r"c:\programdata\microsoft\windows", "系统目录", "Windows 系统数据目录"),

        // 系统级浏览器（安装目录，非缓存）
        // Edge：MicrosoftEdgeUpdate 服务会把 Junction 识别为损坏安装并自动覆盖
        // Chrome：Chromium 把安装路径写死进扩展签名，迁移后所有插件报损坏
        (r"microsoft\edge\application",   "浏览器", "Microsoft Edge 安装目录"),
        (r"microsoft\msedge\application",  "浏览器", "Microsoft Edge 安装目录"),
        (r"google\chrome\application",     "浏览器", "Google Chrome 安装目录"),
        (r"google\chrome beta\application","浏览器", "Google Chrome Beta 安装目录"),
        (r"google\chrome dev\application", "浏览器", "Google Chrome Dev 安装目录"),
        (r"bromite\application",           "浏览器", "Bromite 安装目录"),

        // GPU / 显卡驱动（驱动 DLL 路径写死进服务注册表，迁移后驱动失效）
        (r"nvidia corporation\installer2",   "GPU驱动", "NVIDIA 驱动安装目录"),
        (r"nvidia\displaydriver",            "GPU驱动", "NVIDIA 显卡驱动目录"),
        (r"\nvidia\",                        "GPU驱动", "NVIDIA 驱动目录"),
        (r"amd\ccc2",                        "GPU驱动", "AMD 显卡控制中心目录"),
        (r"advanced micro devices",          "GPU驱动", "AMD 驱动目录"),
        (r"intel\graphics",                  "GPU驱动", "Intel 核显驱动目录"),
        (r"intel\intelgraphicscontrolpanel", "GPU驱动", "Intel 显卡控制面板目录"),

        // Microsoft Edge WebView2 运行时（系统级组件）
        (r"microsoft\edgewebview\application", "浏览器", "Microsoft WebView2 运行时目录"),
    ];

    for (pattern, category, label) in rules {
        if source_normalized.contains(pattern) {
            let tip = match *category {
                "系统目录" => "迁移系统核心目录会导致 Windows 组件崩溃，无法开机。",
                "浏览器"   => "浏览器安装目录含有系统级注册和自动修复机制，迁移后 Junction 会被自动覆盖，且所有扩展插件将损坏。\n如需释放空间，请迁移浏览器的缓存目录（在「数据迁移」页面的快捷项中）。",
                "GPU驱动"  => "GPU 驱动路径写死进系统服务注册表，迁移后驱动无法加载，轻则降级到基本显示模式，重则蓝屏。",
                _          => "该目录包含系统级组件，不支持迁移。",
            };
            return Some(format!(
                "🚫 无法迁移：{label} 属于「{category}」，不支持通过 Junction 迁移。\n\n{tip}",
                label = label,
                category = category,
                tip = tip,
            ));
        }
    }

    None
}

/// 核心迁移命令
/// 将应用从源路径迁移到目标路径，并创建 Windows 目录联接（Junction）
///
/// 新增参数：
/// - `cancel_flag`: 共享的取消标志，前端可通过 cancel_migration 命令设置
/// - `app_handle`: Tauri AppHandle，用于发送进度事件
pub fn migrate_app(
    app_name: String,
    source: String,
    target_parent: String,
    cancel_flag: &Arc<AtomicBool>,
    app_handle: &tauri::AppHandle,
    record_type: MigrationRecordType,
    force_overwrite: bool,
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

        // 步骤 0.1: 危险路径黑名单检测
        // 拦截系统目录、系统级浏览器安装目录、GPU 驱动目录等不可迁移路径
        // 此检测在前端也会触发，后端作为兜底防线（前端校验可被绕过）
        if let Some(danger_msg) = check_dangerous_path(&source) {
            return Ok(MigrationResult {
                success: false,
                message: danger_msg,
                new_path: None,
            });
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

        if target_path.exists() {
            if !force_overwrite {
                // 判断是否为失败迁移残留：source 是普通目录且 target 也存在
                // 区别于 JUNCTION_LOOP 场景（source 仍是 Junction 指向 target）
                let looks_like_failed_migration = source_path.is_dir()
                    && !crate::utils::is_junction(source_path)
                    && target_path.is_dir();

                let msg = if looks_like_failed_migration {
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
        // 根据源目录类型选择检测策略：
        //   - 含 exe（应用目录）→ 进程 exe 路径前缀匹配
        //     原因：exe/dll 被 Windows 内存映射，独占锁探测打开会成功，检测不到
        //   - 不含 exe（数据/缓存目录）→ 文件独占锁探测
        //     原因：数据文件有真实写锁，而进程 exe 不在该目录内，路径匹配无效
        emit_progress(app_handle, &source, 0.0, "checking", "正在检测文件占用...", 0, 0);

        let has_exe_in_source = WalkDir::new(source_path)
            .max_depth(2)
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
        } else {
            // 数据目录：用文件独占锁探测（独占打开每个文件，检测 SHARING_VIOLATION）
            let locked_files = check_directory_file_locks(source_path);
            if !locked_files.is_empty() {
                return Ok(MigrationResult {
                    success: false,
                    message: format!(
                        "目录中有文件正被其他程序占用，无法迁移。\n\n\
                         被占用的文件：\n{}\n\n\
                         请关闭正在使用这些文件的程序（如浏览器、游戏、编辑器等）后重试。",
                        locked_files
                            .iter()
                            .map(|f| format!("  • {}", f))
                            .collect::<Vec<_>>()
                            .join("\n")
                    ),
                    new_path: None,
                });
            }
        }

        // 步骤 0.5.1：及时响应取消（sysinfo 刷新可能较慢）
        if cancel_flag.load(Ordering::Relaxed) {
            return Err("用户取消了迁移".to_string());
        }

        // 步骤 1: 空间检查
        emit_progress(app_handle, &source, 0.0, "counting", "正在计算源文件夹大小...", 0, 0);

        let source_size = utils::get_dir_size_safe(source_path);

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

        // 步骤 2: 复制文件（带进度上报和取消支持）
        // 先创建目标目录的父目录结构
        fs::create_dir_all(&target_path)
            .map_err(|e| format!("创建目标目录失败: {}", e))?;

        let (total_size, skipped_size) = match copy_dir_with_progress(
            source_path, &target_path, &source, cancel_flag, app_handle,
        ) {
            Ok((total, skipped)) => (total, skipped),
            Err(e) => {
                // 取消或复制错误：清理已创建的目标目录，避免残留半成品
                let _ = fs::remove_dir_all(&target_path);
                return Ok(MigrationResult {
                    success: false,
                    message: e,
                    new_path: None,
                });
            }
        };

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
            let _ = fs::remove_dir_all(&target_path);
            return Ok(MigrationResult {
                success: false,
                message: format!(
                    "文件完整性校验失败。预期: {} 字节，实际: {} 字节，跳过: {} 字节",
                    expected_target, target_size, skipped_size
                ),
                new_path: None,
            });
        }

        // 步骤 4: 删除源目录（数据已完整复制到 target，直接原地删除）
        // 不再使用 rename 备份方案：Shell 已知文件夹（Desktop、Videos 等）和
        // Chrome 缓存等目录被 Windows Shell / 索引服务持有引用，fs::rename 会
        // 因 ACCESS_DENIED 失败。新方案直接删除源目录，消除了 rename 成功但
        // symlink 失败且 rename-back 也失败的双重失败极端情况。
        emit_progress(app_handle, &source, 93.0, "linking", "正在创建目录链接...", source_size, source_size);

        if let Err(e) = remove_directory_robust(source_path) {
            // 删除源目录失败，说明复制过程中有新进程锁定了文件
            // 必须清理 target，将状态恢复到迁移前（source 完整，target 不存在）
            let _ = fs::remove_dir_all(&target_path);
            return Ok(MigrationResult {
                success: false,
                message: format!(
                    "迁移中止：原目录中有文件在复制期间被程序重新锁定。\n\
                     路径: {}\n原因: {}\n\n\
                     已自动清理目标副本，原数据完好无损。\n\
                     请关闭相关程序（如浏览器、游戏等）后重试。",
                    source, e
                ),
                new_path: None,
            });
        }

        // 步骤 5: 创建目录联接
        match symlink_dir(&target_path, source_path) {
            Ok(_) => {
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

                let success_msg = format!("迁移成功！应用已从 {} 迁移到 {}", source, target_path_str);

                Ok(MigrationResult {
                    success: true,
                    message: success_msg,
                    new_path: Some(target_path_str),
                })
            }
            Err(e) => {
                // source 已删除，target 是唯一数据副本，无法自动回滚
                // 相比旧方案（rename 备份），新方案消除了 rename 成功但 symlink
                // 失败且 rename-back 也失败的双重失败极端情况
                Ok(MigrationResult {
                    success: false,
                    message: format!(
                        "创建目录链接失败：{}\n\n\
                         您的数据完整保存在：{}\n\
                         请手动处理：\n\
                         1. 将 {} 目录移回 {}\n\
                         2. 或手动创建从 {} 到 {} 的目录联接\n\
                         （以管理员身份运行 cmd：mklink /J \"{}\" \"{}\"）",
                        e, target_path_str,
                        target_path_str, source,
                        target_path_str, source,
                        source, target_path_str
                    ),
                    new_path: None,
                })
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
