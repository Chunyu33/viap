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
/// 每 200 个文件检查一次取消标志，避免大目录扫描期间取消无响应。
#[cfg(windows)]
fn check_directory_file_locks(dir: &Path, cancel_flag: &Arc<AtomicBool>) -> Vec<String> {
    use std::os::windows::fs::OpenOptionsExt;

    let mut locked_files: Vec<String> = Vec::new();
    let mut checked_count: u64 = 0;

    for entry in WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        // 每 200 个文件检查一次取消标志，避免大目录扫描期间取消无响应
        checked_count += 1;
        if checked_count % 200 == 0 && cancel_flag.load(Ordering::Relaxed) {
            return vec!["检测已取消".to_string()];
        }

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
    let mut first_dir_error: Option<(PathBuf, std::io::Error)> = None;

    for entry in WalkDir::new(path).contents_first(true).into_iter().filter_map(|e| e.ok()) {
        let entry_path = entry.path();
        // 清除只读属性，否则 remove_file / remove_dir 会失败
        if let Ok(mut perms) = fs::metadata(entry_path).map(|m| m.permissions()) {
            perms.set_readonly(false);
            let _ = fs::set_permissions(entry_path, perms);
        }
        if entry_path.is_file() || entry_path.is_symlink() {
            // 文件删除失败直接返回，说明文件仍被进程持有
            fs::remove_file(entry_path)?;
        } else if entry_path.is_dir() && entry_path != path {
            if let Err(e) = fs::remove_dir(entry_path) {
                // 记录第一个失败的子目录，用于最终错误消息诊断
                if first_dir_error.is_none() {
                    first_dir_error = Some((entry_path.to_path_buf(), e));
                }
            }
        }
    }

    // 最终删除根目录
    fs::remove_dir(path).map_err(|e| {
        // 优先用子目录失败信息（更具体），没有则用根目录删除失败信息
        if let Some((failed_dir, dir_err)) = first_dir_error {
            std::io::Error::new(
                dir_err.kind(),
                format!(
                    "子目录删除失败（目录：{}，原因：{}）；根目录删除失败：{}",
                    failed_dir.display(), dir_err, e
                ),
            )
        } else {
            e
        }
    })
}

/// 危险路径分级
#[derive(Debug, Clone, PartialEq)]
enum DangerLevel {
    Blocked,   // 绝对拦截，迁移必然导致系统级不可逆损坏
    Warning,   // 高风险但可手动恢复，用户确认后放行
}

/// 危险路径匹配规则
struct DangerRule {
    pattern: &'static str,
    level: DangerLevel,
    category: &'static str,
    label: &'static str,
}

/// 危险路径检测（两级：BLOCKED / WARNING）
///
/// 对源路径做黑名单匹配，拦截以下类别的目录：
///
/// BLOCKED — 迁移必然导致系统级不可逆损坏：
/// 1. **系统核心目录**：Windows / Program Files / WindowsApps 等
/// 2. **系统级浏览器**：Edge / Chrome 安装目录（自动修复服务会覆盖 Junction）
/// 3. **GPU / 显卡驱动**：NVIDIA / AMD / Intel 驱动路径写死进服务注册表
///
/// WARNING — 迁移可能导致相关软件失效，但可手动恢复：
/// 1. **虚拟化软件**：VMware / VirtualBox / Hyper-V（含绝对路径引用）
/// 2. **数据库**：MySQL / PostgreSQL / MongoDB / Redis / SQL Server（含事务日志）
/// 3. **安全软件**：Defender / Kaspersky / ESET（含内核级驱动）
/// 4. **系统组件缓存**：VS Package Cache
/// 5. **开发工具**：Visual Studio / JetBrains（含内核映射 DLL）
/// 6. **ProgramData 根目录**（包含大量系统级配置）
///
/// 返回 Some((level, 用户可见消息)) 或 None（安全路径）
fn check_dangerous_path(source: &str) -> Option<(DangerLevel, String)> {
    let source_lower = source.to_lowercase();
    let source_normalized = source_lower.replace('/', "\\");

    // ── 规则表（BLOCKED 在前，WARNING 在后）──────────────────────────────
    // 顺序至关重要：c:\programdata\microsoft\windows (BLOCKED) 必须在
    // c:\programdata (WARNING) 之前，确保具体子路径不会被父路径规则降级
    let rules: &[DangerRule] = &[
        // ═══════════════════════════════════════
        // BLOCKED — 系统核心目录
        // ═══════════════════════════════════════
        DangerRule { pattern: r"c:\windows",                            level: DangerLevel::Blocked, category: "系统目录", label: "Windows 系统目录" },
        DangerRule { pattern: r"c:\program files\windowsapps",          level: DangerLevel::Blocked, category: "系统目录", label: "Windows 应用商店目录" },
        DangerRule { pattern: r"c:\programdata\microsoft\windows",      level: DangerLevel::Blocked, category: "系统目录", label: "Windows 系统数据目录" },
        DangerRule { pattern: r"c:\windows\system32",          level: DangerLevel::Blocked, category: "系统目录", label: "Windows System32 目录" },
        DangerRule { pattern: r"c:\windows\syswow64",          level: DangerLevel::Blocked, category: "系统目录", label: "Windows SysWOW64 目录" },
        DangerRule { pattern: r"c:\windows\winsxs",            level: DangerLevel::Blocked, category: "系统目录", label: "Windows WinSxS 组件库" },
        DangerRule { pattern: r"^c:\users$",                   level: DangerLevel::Blocked, category: "系统目录", label: "Users 用户配置根目录" },
        DangerRule { pattern: r"wpsystem",                     level: DangerLevel::Blocked, category: "系统目录", label: "Windows 商店加密数据目录" },

        // ═══════════════════════════════════════
        // BLOCKED — 系统级浏览器安装目录
        // ═══════════════════════════════════════
        DangerRule { pattern: r"microsoft\edge\application",            level: DangerLevel::Blocked, category: "浏览器", label: "Microsoft Edge 安装目录" },
        DangerRule { pattern: r"microsoft\msedge\application",          level: DangerLevel::Blocked, category: "浏览器", label: "Microsoft Edge 安装目录" },
        DangerRule { pattern: r"microsoft\edgewebview\application",     level: DangerLevel::Blocked, category: "浏览器", label: "Microsoft WebView2 运行时目录" },
        DangerRule { pattern: r"google\chrome\application",             level: DangerLevel::Blocked, category: "浏览器", label: "Google Chrome 安装目录" },
        DangerRule { pattern: r"google\chrome beta\application",        level: DangerLevel::Blocked, category: "浏览器", label: "Google Chrome Beta 安装目录" },
        DangerRule { pattern: r"google\chrome dev\application",         level: DangerLevel::Blocked, category: "浏览器", label: "Google Chrome Dev 安装目录" },
        DangerRule { pattern: r"bromite\application",                   level: DangerLevel::Blocked, category: "浏览器", label: "Bromite 安装目录" },

        // ═══════════════════════════════════════
        // BLOCKED — Microsoft Office ClickToRun
        // ═══════════════════════════════════════
        DangerRule { pattern: r"\microsoft office",                       level: DangerLevel::Blocked, category: "办公软件", label: "Microsoft Office 安装目录" },
        DangerRule { pattern: r"programdata\microsoft\clicktorun",         level: DangerLevel::Blocked, category: "办公软件", label: "Office ClickToRun 服务目录" },

        // ═══════════════════════════════════════
        // BLOCKED — GPU / 显卡驱动
        // ═══════════════════════════════════════
        DangerRule { pattern: r"nvidia corporation\installer2",         level: DangerLevel::Blocked, category: "GPU驱动", label: "NVIDIA 驱动安装目录" },
        DangerRule { pattern: r"nvidia\displaydriver",                  level: DangerLevel::Blocked, category: "GPU驱动", label: "NVIDIA 显卡驱动目录" },
        DangerRule { pattern: r"\nvidia corporation",                   level: DangerLevel::Blocked, category: "GPU驱动", label: "NVIDIA 驱动目录" },
        DangerRule { pattern: r"\nvidia\",                              level: DangerLevel::Blocked, category: "GPU驱动", label: "NVIDIA 驱动目录" },
        DangerRule { pattern: r"amd\ccc2",                             level: DangerLevel::Blocked, category: "GPU驱动", label: "AMD 显卡控制中心目录" },
        DangerRule { pattern: r"advanced micro devices",               level: DangerLevel::Blocked, category: "GPU驱动", label: "AMD 驱动目录" },
        DangerRule { pattern: r"intel\graphics",                       level: DangerLevel::Blocked, category: "GPU驱动", label: "Intel 核显驱动目录" },
        DangerRule { pattern: r"intel\intelgraphicscontrolpanel",      level: DangerLevel::Blocked, category: "GPU驱动", label: "Intel 显卡控制面板目录" },

        // ═══════════════════════════════════════
        // BLOCKED — .NET Runtime
        // ═══════════════════════════════════════
        DangerRule { pattern: r"c:\program files\dotnet",                 level: DangerLevel::Blocked, category: "运行时", label: ".NET Runtime 安装目录" },

        // ═══════════════════════════════════════
        // WARNING — 虚拟化软件
        // ═══════════════════════════════════════
        DangerRule { pattern: r"vmware",         level: DangerLevel::Warning, category: "虚拟化", label: "VMware 目录" },
        DangerRule { pattern: r"virtualbox",     level: DangerLevel::Warning, category: "虚拟化", label: "VirtualBox 目录" },
        DangerRule { pattern: r"hyper-v",        level: DangerLevel::Warning, category: "虚拟化", label: "Hyper-V 目录" },

        // ═══════════════════════════════════════
        // WARNING — 数据库
        // ═══════════════════════════════════════
        DangerRule { pattern: r"mysql",                level: DangerLevel::Warning, category: "数据库", label: "MySQL 数据目录" },
        DangerRule { pattern: r"postgresql",           level: DangerLevel::Warning, category: "数据库", label: "PostgreSQL 数据目录" },
        DangerRule { pattern: r"mongodb",              level: DangerLevel::Warning, category: "数据库", label: "MongoDB 数据目录" },
        DangerRule { pattern: r"redis",                level: DangerLevel::Warning, category: "缓存服务", label: "Redis 数据目录" },
        DangerRule { pattern: r"microsoft sql server", level: DangerLevel::Warning, category: "数据库", label: "SQL Server 数据目录" },
        DangerRule { pattern: r"elasticsearch",          level: DangerLevel::Warning, category: "数据库", label: "Elasticsearch 数据目录" },
        DangerRule { pattern: r"rabbitmq",               level: DangerLevel::Warning, category: "数据库", label: "RabbitMQ 数据目录" },
        DangerRule { pattern: r"kafka",                  level: DangerLevel::Warning, category: "数据库", label: "Kafka 数据目录" },

        // ═══════════════════════════════════════
        // WARNING — 安全软件
        // ═══════════════════════════════════════
        DangerRule { pattern: r"windows defender", level: DangerLevel::Warning, category: "安全软件", label: "Windows Defender 目录" },
        DangerRule { pattern: r"kaspersky",        level: DangerLevel::Warning, category: "安全软件", label: "Kaspersky 目录" },
        DangerRule { pattern: r"eset",             level: DangerLevel::Warning, category: "安全软件", label: "ESET 目录" },
        DangerRule { pattern: r"norton",          level: DangerLevel::Warning, category: "安全软件", label: "Norton 安全软件目录" },
        DangerRule { pattern: r"symantec",        level: DangerLevel::Warning, category: "安全软件", label: "Symantec 目录" },
        DangerRule { pattern: r"mcafee",          level: DangerLevel::Warning, category: "安全软件", label: "McAfee/Trellix 目录" },
        DangerRule { pattern: r"360安全",          level: DangerLevel::Warning, category: "安全软件", label: "360 安全卫士目录" },
        DangerRule { pattern: r"360total",        level: DangerLevel::Warning, category: "安全软件", label: "360 Total Security 目录" },
        DangerRule { pattern: r"huorong",         level: DangerLevel::Warning, category: "安全软件", label: "火绒安全目录" },
        DangerRule { pattern: r"bitdefender",     level: DangerLevel::Warning, category: "安全软件", label: "Bitdefender 目录" },
        DangerRule { pattern: r"malwarebytes",    level: DangerLevel::Warning, category: "安全软件", label: "Malwarebytes 目录" },

        // ═══════════════════════════════════════
        // WARNING — 系统组件缓存
        // ═══════════════════════════════════════
        DangerRule { pattern: r"package cache",  level: DangerLevel::Warning, category: "系统组件", label: "Visual Studio Package Cache" },

        // ═══════════════════════════════════════
        // WARNING — 开发工具
        // ═══════════════════════════════════════
        DangerRule { pattern: r"microsoft visual studio", level: DangerLevel::Warning, category: "开发工具", label: "Visual Studio 安装目录" },
        DangerRule { pattern: r"jetbrains",              level: DangerLevel::Warning, category: "开发工具", label: "JetBrains IDE 目录" },
        DangerRule { pattern: r"\microsoft vs code", level: DangerLevel::Warning, category: "开发工具", label: "VSCode 安装目录" },

        // ═══════════════════════════════════════
        // WARNING — 即时通讯应用数据
        // ═══════════════════════════════════════
        DangerRule { pattern: r"wechat files",  level: DangerLevel::Warning, category: "缓存服务", label: "微信数据目录" },
        DangerRule { pattern: r"tencent files", level: DangerLevel::Warning, category: "缓存服务", label: "腾讯系应用数据目录" },

        // ═══════════════════════════════════════
        // WARNING — 游戏平台库
        // ═══════════════════════════════════════
        DangerRule { pattern: r"steamapps", level: DangerLevel::Warning, category: "游戏平台", label: "Steam 游戏库目录" },

        // ═══════════════════════════════════════
        // WARNING — ProgramData 根目录
        // 必须排在 c:\programdata\microsoft\windows (BLOCKED) 之后
        // ═══════════════════════════════════════
        DangerRule { pattern: r"c:\programdata", level: DangerLevel::Warning, category: "系统目录", label: "ProgramData 根目录" },
    ];

    /// 路径匹配：支持 ^pattern$ 精确匹配（如 ^c:\users$ 只匹配根目录，不匹配子目录），
    /// 其余规则用 contains 前缀匹配。确保 Blocked First 原则：BLOCKED 规则先遍历，
    /// 命中即返回，不会降级为 WARNING。
    fn match_path(source: &str, pattern: &str) -> bool {
        let is_exact = pattern.starts_with('^') && pattern.ends_with('$');
        let match_pattern = if is_exact { &pattern[1..pattern.len()-1] } else { pattern };
        if is_exact { source == match_pattern } else { source.contains(match_pattern) }
    }

    for rule in rules {
        if match_path(&source_normalized, rule.pattern) {
            match rule.level {
                DangerLevel::Blocked => {
                    let tip = match rule.category {
                        "系统目录" => "迁移系统核心目录会导致 Windows 组件崩溃，无法开机。",
                        "浏览器"   => "浏览器安装目录含有系统级注册和自动修复机制，迁移后 Junction 会被自动覆盖，且所有扩展插件将损坏。\n如需释放空间，请迁移浏览器的缓存目录（在「数据迁移」页面的快捷项中）。",
                        "GPU驱动"  => "GPU 驱动路径写死进系统服务注册表，迁移后驱动无法加载，轻则降级到基本显示模式，重则蓝屏。",
                        "办公软件" => "Microsoft Office 使用 ClickToRun 虚拟化安装机制，迁移后自动修复服务会覆盖 Junction，COM 注册表记录无法跟随迁移。",
                        "运行时"   => ".NET 运行时路径被大量应用和系统组件硬编码引用，迁移后依赖 .NET 的应用将无法启动。",
                        "开发工具" => "开发工具目录含被 Windows 内核内存映射的 DLL 和后台语言服务，复制阶段容易失败，迁移前需完全退出所有相关进程。",
                        _          => "该目录包含系统级组件，不支持迁移。",
                    };
                    return Some((DangerLevel::Blocked, format!(
                        "🚫 无法迁移：{label} 属于「{category}」，不支持通过 Junction 迁移。\n\n{tip}",
                        label = rule.label,
                        category = rule.category,
                        tip = tip,
                    )));
                }
                DangerLevel::Warning => {
                    // WARNING 返回简短标签信息；详细风险说明由前端弹窗展示
                    return Some((DangerLevel::Warning, format!(
                        "高风险目录：{label}（{category}）",
                        label = rule.label,
                        category = rule.category,
                    )));
                }
            }
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

        // 及时响应取消（大目录 WalkDir 可能耗时较长）
        if cancel_flag.load(Ordering::Relaxed) {
            return Err("用户取消了迁移".to_string());
        }

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
            let locked_files = check_directory_file_locks(source_path, cancel_flag);
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
            let _ = fs::remove_dir_all(&target_path);
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
            // 删除源目录失败，可能原因：文件被进程锁定 / 权限不足 / 只读保护
            // 必须清理 target，将状态恢复到迁移前（source 完整，target 不存在）
            let _ = fs::remove_dir_all(&target_path);
            return Ok(MigrationResult {
                success: false,
                message: format!(
                    "删除原目录失败，迁移中止。\n\
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
            Err(symlink_err) => {
                // source 已删除，target 是唯一数据副本
                // 策略：优先尝试 rename 把 target 移回 source（同盘原子操作，成功率高）
                // rename 失败（跨盘）时不再做 copy-then-delete（可能制造两个残缺副本），
                // 而是保留 target 并返回明确的恢复指引
                let rollback_result = fs::rename(&target_path, source_path);

                match rollback_result {
                    Ok(_) => {
                        // 回滚成功：source 已恢复，target 已清除，用户数据安全
                        // 清理可能遗留的空父目录（如 D:\Apps 是本次迁移新建的，rename 后成为空目录）
                        if let Some(parent) = target_path.parent() {
                            let _ = fs::remove_dir(parent);
                        }
                        Ok(MigrationResult {
                            success: false,
                            message: format!(
                                "创建目录链接失败，已自动将数据恢复到原位置。\n\n\
                                 失败原因：{}\n\n\
                                 常见原因及解决方案：\n\
                                 • 权限不足 → 请以管理员身份运行本程序后重试\n\
                                 • 源路径被占用 → 请重启后重试\n\n\
                                 您的数据已完整恢复到：{}",
                                symlink_err, source
                            ),
                            new_path: None,
                        })
                    }
                    Err(rename_err) => {
                        // rename 失败（通常是跨盘 os error 17），target 仍是唯一副本
                        // 此时数据安全但位置不在原处，必须明确告知用户
                        orbit_log!(
                            "ERROR", "migration",
                            "symlink 失败且 rename 回滚也失败。symlink_err={}, rename_err={}, data_at={}",
                            symlink_err, rename_err, target_path_str
                        );
                        Ok(MigrationResult {
                            success: false,
                            // 用特殊前缀让前端识别「数据已转移但链接失败」状态
                            message: format!(
                                "SYMLINK_FAILED_DATA_AT_TARGET:{target}\n\
                                 创建目录链接失败，自动回滚也未成功。\n\n\
                                 ⚠️ 您的数据完整保存在新位置，原位置已清空：\n\
                                 数据位置：{target}\n\n\
                                 请选择以下任一方式恢复：\n\
                                 方式一（推荐）：将数据移回原位置\n\
                                 　把「{target}」整个目录移动到「{source}」\n\n\
                                 方式二：手动创建目录链接\n\
                                 　以管理员身份运行 CMD，执行：\n\
                                 　mklink /J \"{source}\" \"{target}\"\n\n\
                                 链接失败原因：{symlink_err}",
                                target = target_path_str,
                                source = source,
                                symlink_err = symlink_err,
                            ),
                            // 返回 target 路径，让前端知道数据在哪
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
