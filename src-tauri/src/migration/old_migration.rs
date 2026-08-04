// 应用迁移模块
// 负责应用目录迁移、空间校验、进度上报、回滚与历史写入

use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rayon::prelude::*;
use serde::Serialize;
use sysinfo::Disks;
use tauri::Emitter;
use walkdir::WalkDir;

use crate::models::{MigrationRecordType, MigrationResult};
use crate::utils;

#[cfg(windows)]
use std::os::windows::fs::symlink_dir;

// Windows 原生复制 API（CopyFileExW），仅在 Windows 目标上引入
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
#[cfg(windows)]
use windows::core::PCWSTR;
#[cfg(windows)]
use windows::Win32::Foundation::{GetLastError, HANDLE, WIN32_ERROR};
#[cfg(windows)]
use windows::Win32::Storage::FileSystem::{CopyFileExW, LPPROGRESS_ROUTINE_CALLBACK_REASON};

/// 复制并发线程数：Windows 单卷复制实测 8 线程为吞吐甜点，
/// 全核并发会争抢 NTFS 元数据锁（$MFT / $LogFile）反而降低总吞吐。
const COPY_PARALLELISM: usize = 8;
/// 大文件阈值：超过该大小的文件走顺序单流复制，保证 SSD 顺序吞吐峰值。
const LARGE_FILE_THRESHOLD: u64 = 32 * 1024 * 1024; // 32MB
/// 无缓冲复制阈值：>= 该大小的文件尝试 COPY_FILE_NO_BUFFERING 绕过系统缓存。
const NO_BUFFERING_THRESHOLD: u64 = 256 * 1024 * 1024; // 256MB
// CopyFileExW 标志（winbase.h 标准值，windows 0.58 绑定未导出常量，此处按规范值定义）
/// 允许复制到无法加密的目标（源为 EFS 加密文件时避免复制失败）
const COPY_FILE_ALLOW_DECRYPTED_DESTINATION: u32 = 0x0000_0008;
/// 大文件绕过系统缓存直写，达到近设备峰值吞吐
const COPY_FILE_NO_BUFFERING: u32 = 0x0000_1000;

/// 复制计划：扫描阶段一次性生成，后续空间检查和复制阶段复用同一份数据。
pub(crate) struct CopyPlan {
    /// 待复制文件列表，扫描阶段生成后直接复用，避免复制前二次遍历磁盘。
    pub(crate) file_list: Vec<(PathBuf, PathBuf, u64)>,
    /// 待创建目录列表，保留空目录，避免应用依赖占位目录时异常。
    pub(crate) dir_list: Vec<PathBuf>,
    /// 计划复制的总字节数，用于空间检查和复制进度计算。
    pub(crate) total_size: u64,
}

pub(crate) struct RestoreDirectoryResult {
    /// 已恢复到原路径的字节数，用于成功提示或后续日志。
    pub(crate) restored_size: u64,
    /// 目标副本清理失败时不影响数据完整性，但需要提示调用方。
    pub(crate) cleanup_warning: Option<String>,
}

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
pub(crate) fn build_copy_plan_with_progress(
    source: &Path,
    target: &Path,
    task_id: &str,
    cancel_flag: &Arc<AtomicBool>,
    app_handle: &tauri::AppHandle,
) -> Result<CopyPlan, String> {
    emit_progress(app_handle, task_id, 1.0, "counting", "正在扫描文件列表...", 0, 0);

    let mut file_list: Vec<(PathBuf, PathBuf, u64)> = Vec::new();
    let mut dir_list: Vec<PathBuf> = Vec::new();
    let mut total_size: u64 = 0;
    let mut scanned_files: u64 = 0;
    let mut last_emit = Instant::now();

    for entry_result in WalkDir::new(source).into_iter() {
        let entry = entry_result
            .map_err(|e| format!("目录遍历失败 {}: {}", source.display(), e))?;
        if cancel_flag.load(Ordering::Relaxed) {
            return Err("用户取消了迁移".to_string());
        }

        let rel_path = entry.path().strip_prefix(source)
            .map_err(|e| format!("路径解析失败: {}", e))?;
        if rel_path.as_os_str().is_empty() {
            continue;
        }

        if entry.file_type().is_dir() {
            dir_list.push(target.join(rel_path));
        } else if entry.file_type().is_file() {
            let dest = target.join(rel_path);
            let size = entry.metadata()
                .map_err(|e| format!("读取文件元数据失败 {}: {}", entry.path().display(), e))?
                .len();
            total_size += size;
            scanned_files += 1;
            file_list.push((entry.path().to_path_buf(), dest, size));
        }

        if last_emit.elapsed() >= Duration::from_millis(250) {
            // 扫描阶段没有总文件数，百分比只表示整体迁移已进入准备区间，真实进展放在文案里。
            let percent = (1.0 + (scanned_files as f64 / 500.0)).min(8.0);
            emit_progress(
                app_handle,
                task_id,
                percent,
                "counting",
                &format!("已扫描 {} 个文件，{}", scanned_files, format_bytes(total_size)),
                total_size,
                0,
            );
            last_emit = Instant::now();
        }
    }

    emit_progress(
        app_handle,
        task_id,
        9.0,
        "counting",
        &format!("扫描完成：{} 个文件，{}", scanned_files, format_bytes(total_size)),
        total_size,
        total_size,
    );

    Ok(CopyPlan { file_list, dir_list, total_size })
}

/// 单个文件的复制进度上下文（栈上分配，CopyFileExW 进度回调与调用线程同线程）
struct CopyProgressContext<'a> {
    /// 全局累计已复制字节数
    copied_size: Arc<AtomicU64>,
    /// 已上报进度百分比（CAS 节流，避免高频 emit 拖慢复制）
    last_report_pct: Arc<AtomicU64>,
    /// 本次迁移总字节数（进度百分比分母）
    total_size: u64,
    /// 本文件已回调累计传输字节（回调与调用线程同线程，用 Cell 免原子开销）
    last_transferred: Cell<u64>,
    /// 用户取消标志
    cancel_flag: Arc<AtomicBool>,
    /// 内部取消标志（首个错误出现后通知其余线程停止）
    internal_cancel: Arc<AtomicBool>,
    /// 进度事件发送句柄
    app_handle: &'a tauri::AppHandle,
    /// 任务标识（源路径）
    task_id: &'a str,
}

/// 按全局已复制字节数计算进度并 CAS 节流上报（复制回调与复制完成补账共用）。
fn try_report_progress(
    app_handle: &tauri::AppHandle,
    task_id: &str,
    last_report_pct: &AtomicU64,
    new_copied: u64,
    total_size: u64,
) {
    let current_pct = (10.0 + (new_copied as f64 / total_size as f64 * 78.0)) as u64;
    let prev = last_report_pct.load(Ordering::Relaxed);
    if current_pct > prev
        && last_report_pct
            .compare_exchange(prev, current_pct, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    {
        emit_progress(
            app_handle,
            task_id,
            current_pct as f64,
            "copying",
            &format!(
                "已复制 {} / {}",
                format_bytes(new_copied),
                format_bytes(total_size)
            ),
            new_copied,
            total_size,
        );
    }
}

/// CopyFileExW 进度回调：按块累加传输字节并节流上报进度，
/// 返回 PROGRESS_CANCEL(1) 实现取消（用户取消或内部错误取消）。
///
/// 注意：回调与 CopyFileExW 调用线程同线程，ctx 指针在调用期间始终有效。
#[cfg(windows)]
unsafe extern "system" fn copy_progress_routine(
    _total_file_size: i64,
    total_bytes_transferred: i64,
    _stream_size: i64,
    _stream_bytes_transferred: i64,
    _stream_number: u32,
    _callback_reason: LPPROGRESS_ROUTINE_CALLBACK_REASON,
    _h_source_file: HANDLE,
    _h_destination_file: HANDLE,
    lp_data: *const core::ffi::c_void,
) -> u32 {
    let ctx = &*(lp_data as *const CopyProgressContext);

    // 用户取消或内部错误触发取消：返回 PROGRESS_CANCEL 让 CopyFileExW 中止
    if ctx.cancel_flag.load(Ordering::Relaxed) || ctx.internal_cancel.load(Ordering::Relaxed) {
        return 1;
    }

    // 用增量累加避免重复统计（回调按块触发，传输量单调递增）
    let transferred = total_bytes_transferred as u64;
    let delta = transferred.saturating_sub(ctx.last_transferred.get());
    ctx.last_transferred.set(transferred);
    if delta > 0 {
        let new_copied = ctx.copied_size.fetch_add(delta, Ordering::Relaxed) + delta;
        try_report_progress(
            ctx.app_handle,
            ctx.task_id,
            &ctx.last_report_pct,
            new_copied,
            ctx.total_size,
        );
    }
    0 // PROGRESS_CONTINUE
}

/// 将 CopyFileExW 失败码转换为用户可读的错误消息。
#[cfg(windows)]
fn map_copy_error(src: &Path, err: WIN32_ERROR) -> String {
    let code = err.0;
    // 5 = ERROR_ACCESS_DENIED，32 = ERROR_SHARING_VIOLATION：文件被其他程序占用
    if code == 5 || code == 32 {
        format!(
            "复制过程中文件被程序占用: {}\n请关闭相关程序后重试。",
            src.display()
        )
    } else {
        // 只输出错误码即可定位问题（WIN32_ERROR 未实现 Display，错误名由上层提示覆盖）
        format!("复制文件失败 {}（错误码 {}）", src.display(), code)
    }
}

/// 通过 Windows 原生 CopyFileExW 复制单个文件。
///
/// 相比旧实现的手动分块读写：
/// 1. 单次 API 调用替代 6~10 次系统调用，小文件密集场景吞吐显著提升
/// 2. 自动保留文件时间戳 / NTFS 备用数据流 / 文件属性，修复大文件 mtime 丢失问题
/// 3. 进度回调支持块级进度累计与取消（回调返回 PROGRESS_CANCEL 中止）
///
/// 权限拒绝时中断迁移（步骤 0.5 已做预检，此处不应再出现被锁文件）。
/// 返回该文件大小（复制成功即完整写入），供跳过统计使用。
#[cfg(windows)]
fn copy_file_with_cancel(
    src: &Path,
    dest: &Path,
    cancel_flag: &Arc<AtomicBool>,
    internal_cancel: &Arc<AtomicBool>,
    copied_size: &Arc<AtomicU64>,
    last_report_pct: &Arc<AtomicU64>,
    total_size: u64,
    app_handle: &tauri::AppHandle,
    task_id: &str,
) -> Result<u64, String> {
    // 被锁文件：步骤 0.5 已做预检，此处出现说明文件在复制过程中被新进程锁定，直接中断
    let file_size = match fs::metadata(src) {
        Ok(meta) => meta.len(),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            return Err(format!(
                "复制过程中文件被程序占用: {}\n请关闭相关程序后重试。",
                src.display()
            ));
        }
        Err(e) => return Err(format!("读取文件元数据失败 {}: {}", src.display(), e)),
    };

    let src_wide: Vec<u16> = src
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let dest_wide: Vec<u16> = dest
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    // 大文件尝试无缓冲直写（绕过系统缓存）；失败自动降级为常规复制
    let mut use_no_buffering = file_size >= NO_BUFFERING_THRESHOLD;
    let flags = COPY_FILE_ALLOW_DECRYPTED_DESTINATION;

    let ctx = CopyProgressContext {
        copied_size: copied_size.clone(),
        last_report_pct: last_report_pct.clone(),
        total_size,
        last_transferred: Cell::new(0),
        cancel_flag: cancel_flag.clone(),
        internal_cancel: internal_cancel.clone(),
        app_handle,
        task_id,
    };

    // 最多两次尝试：无缓冲复制失败（部分卷/文件系统不支持）时降级重试
    for _ in 0..2 {
        let copy_flags = if use_no_buffering {
            flags | COPY_FILE_NO_BUFFERING
        } else {
            flags
        };
        // windows 0.58 绑定返回 Result：失败时用 GetLastError 取真实 Win32 错误码
        let result = unsafe {
            CopyFileExW(
                PCWSTR(src_wide.as_ptr()),
                PCWSTR(dest_wide.as_ptr()),
                Some(copy_progress_routine),
                Some(&ctx as *const CopyProgressContext as *const core::ffi::c_void),
                None, // 取消通过进度回调返回 PROGRESS_CANCEL 实现，无需 pbCancel
                copy_flags,
            )
        };
        if result.is_ok() {
            break;
        }

        let err = unsafe { GetLastError() };
        // 1235 = ERROR_REQUEST_ABORTED：进度回调返回了 PROGRESS_CANCEL（用户/内部取消）
        if err.0 == 1235 {
            let _ = fs::remove_file(dest);
            return Err("用户取消了迁移".to_string());
        }
        if use_no_buffering {
            // 无缓冲复制失败（卷不支持等），清掉半成品后降级重试
            let _ = fs::remove_file(dest);
            use_no_buffering = false;
            continue;
        }
        return Err(map_copy_error(src, err));
    }

    // 补齐进度回调未覆盖的尾差（小文件可能一次回调都没有），保证字节计数精确
    let counted = ctx.last_transferred.get();
    if counted < file_size {
        let new_copied =
            ctx.copied_size.fetch_add(file_size - counted, Ordering::Relaxed) + file_size;
        try_report_progress(app_handle, task_id, last_report_pct, new_copied, total_size);
    }

    Ok(file_size)
}

/// 非 Windows 平台回退实现：迁移功能仅支持 Windows，此处仅保证跨平台编译。
#[cfg(not(windows))]
fn copy_file_with_cancel(
    src: &Path,
    dest: &Path,
    _cancel_flag: &Arc<AtomicBool>,
    _internal_cancel: &Arc<AtomicBool>,
    _copied_size: &Arc<AtomicU64>,
    _last_report_pct: &Arc<AtomicU64>,
    _total_size: u64,
    _app_handle: &tauri::AppHandle,
    _task_id: &str,
) -> Result<u64, String> {
    fs::copy(src, dest).map_err(|e| format!("复制文件失败 {}: {}", src.display(), e))
}

/// 带进度上报和取消支持的文件复制
///
/// 替代 fs_extra::copy_items，改用 CopyFileExW 逐个文件复制以便：
/// 1. 在复制进度回调中检查取消标志，任意时刻可中止
/// 2. 按实际复制量上报进度百分比（回调内累计 + 完成补账）
/// 3. 大文件顺序复制、小文件 8 线程并行，兼顾吞吐与元数据开销
///
/// 返回 (总文件大小, 因权限拒绝跳过的字节数)
fn copy_dir_with_progress(
    plan: CopyPlan,
    task_id: &str,
    cancel_flag: &Arc<AtomicBool>,
    app_handle: &tauri::AppHandle,
) -> Result<(u64, u64), String> {
    let CopyPlan { file_list, dir_list, total_size } = plan;

    // 阶段 1.5：预建所有目标目录；空目录也必须迁移，否则部分应用会因缺少占位目录异常。
    {
        let mut dirs: std::collections::BTreeSet<PathBuf> = std::collections::BTreeSet::new();
        for dir in dir_list {
            dirs.insert(dir);
        }
        for (_, dest, _) in &file_list {
            if let Some(parent) = dest.parent() {
                dirs.insert(parent.to_path_buf());
            }
        }
        for dir in dirs {
            fs::create_dir_all(&dir)
                .map_err(|e| format!("创建目录失败 {}: {}", dir.display(), e))?;
        }
    }

    if total_size == 0 {
        emit_progress(app_handle, task_id, 88.0, "copying", "源目录为空，已复制目录结构", 0, 0);
        return Ok((0, 0));
    }

    // 阶段 2：按大小分流复制
    // 大文件顺序单流复制保证 SSD 顺序吞吐峰值；小文件 8 线程并行分摊元数据开销
    // （全核并发会争抢 NTFS 元数据锁，实测反而降低总吞吐）
    let (mut large_files, small_files): (Vec<_>, Vec<_>) = file_list
        .into_iter()
        .partition(|(_, _, size)| *size > LARGE_FILE_THRESHOLD);
    // 大文件按体积降序，优先搬走最大的文件
    large_files.sort_by(|a, b| b.2.cmp(&a.2));

    emit_progress(app_handle, task_id, 10.0, "copying", "开始复制文件...", 0, total_size);

    let internal_cancel = Arc::new(AtomicBool::new(false));
    let copied_size = Arc::new(AtomicU64::new(0));
    let skipped_size = Arc::new(AtomicU64::new(0));
    let last_report_pct = Arc::new(AtomicU64::new(0));
    let error_slot: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    // 大文件顺序复制：一次一个大文件，避免多条大流同时打盘
    for (src, dest, size) in &large_files {
        // 任一条件触发即跳过：已有错误 / 用户取消 / 内部取消
        if error_slot.lock().unwrap().is_some()
            || internal_cancel.load(Ordering::Relaxed)
            || cancel_flag.load(Ordering::Relaxed)
        {
            break;
        }
        match copy_file_with_cancel(
            src,
            dest,
            cancel_flag,
            &internal_cancel,
            &copied_size,
            &last_report_pct,
            total_size,
            app_handle,
            task_id,
        ) {
            Ok(actually_copied) => {
                if actually_copied == 0 && *size > 0 {
                    skipped_size.fetch_add(*size, Ordering::Relaxed);
                }
                // 进度已在 CopyFileExW 回调内上报，此处无需重复 emit
            }
            Err(e) => {
                let mut slot = error_slot.lock().unwrap();
                if slot.is_none() {
                    *slot = Some(e);
                    internal_cancel.store(true, Ordering::Relaxed);
                }
            }
        }
    }

    // 小文件并行复制：受限线程池（COPY_PARALLELISM 线程）避免全核争抢 NTFS 元数据锁
    if error_slot.lock().unwrap().is_none()
        && !internal_cancel.load(Ordering::Relaxed)
        && !cancel_flag.load(Ordering::Relaxed)
    {
        let copy_pool = rayon::ThreadPoolBuilder::new()
            .num_threads(COPY_PARALLELISM)
            .build()
            .map_err(|e| format!("创建复制线程池失败: {}", e))?;
        copy_pool.install(|| {
            small_files.par_iter().for_each(|(src, dest, size)| {
                // 任一条件触发即跳过：已有错误 / 用户取消 / 内部取消
                if error_slot.lock().unwrap().is_some()
                    || internal_cancel.load(Ordering::Relaxed)
                    || cancel_flag.load(Ordering::Relaxed)
                {
                    return;
                }
                match copy_file_with_cancel(
                    src,
                    dest,
                    cancel_flag,
                    &internal_cancel,
                    &copied_size,
                    &last_report_pct,
                    total_size,
                    app_handle,
                    task_id,
                ) {
                    Ok(actually_copied) => {
                        if actually_copied == 0 && *size > 0 {
                            skipped_size.fetch_add(*size, Ordering::Relaxed);
                        }
                    }
                    Err(e) => {
                        let mut slot = error_slot.lock().unwrap();
                        if slot.is_none() {
                            *slot = Some(e);
                            internal_cancel.store(true, Ordering::Relaxed);
                        }
                    }
                }
            });
        });
    }

    // 检查并发复制是否有错误
    if let Some(err) = error_slot.lock().unwrap().take() {
        return Err(err);
    }
    if cancel_flag.load(Ordering::Relaxed) {
        return Err("用户取消了迁移".to_string());
    }

    let skipped_size = skipped_size.load(Ordering::Relaxed);
    // 返回 WalkDir 阶段统计的 total_size 而非 AtomicU64 累加的 copied_size，
    // 确保完整性校验基准不受并行取消影响
    Ok((total_size, skipped_size))
}

/// 检测目录内文件是否被其他进程独占持有
///
/// 原理：以独占模式（FILE_SHARE_NONE）尝试打开每个文件。
/// 若其他进程持有写锁（如 Chrome 正在写 Cache），此调用返回
/// ERROR_SHARING_VIOLATION(32) 或 ERROR_ACCESS_DENIED(5)。
///
/// 适用场景：
/// - 数据目录：检测真实写锁（浏览器缓存等）
/// - 应用目录：检测被 explorer/系统服务加载的 DLL（如 shell extension），
///   这类文件虽不阻塞复制（读共享），但会导致迁移后的备份目录清理失败
///
/// 注意：此方法检测不到应用本体——exe 被内存映射时不阻塞独占打开，
/// 应用本体需用进程 exe 路径匹配检测，两者互补使用。
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
pub(crate) fn remove_directory_robust(path: &Path) -> Result<(), std::io::Error> {
    match fs::remove_dir_all(path) {
        Ok(_) => return Ok(()),
        Err(e) if e.kind() != std::io::ErrorKind::PermissionDenied => return Err(e),
        Err(_) => {}
    }

    // 因只读文件导致失败：遍历清除只读属性后逐个删除。
    // 尽量删除所有可删文件：单个文件被进程占用（如资源管理器加载的
    // shell extension DLL）不应阻止其余文件清理，残留目录只保留真正
    // 被占用的文件，便于用户稍后手动处理。
    let mut first_file_error: Option<(PathBuf, std::io::Error)> = None;
    let mut first_dir_error: Option<(PathBuf, std::io::Error)> = None;

    for entry in WalkDir::new(path).contents_first(true).into_iter().filter_map(|e| e.ok()) {
        let entry_path = entry.path();
        // 清除只读属性，否则 remove_file / remove_dir 会失败
        if let Ok(mut perms) = fs::metadata(entry_path).map(|m| m.permissions()) {
            perms.set_readonly(false);
            let _ = fs::set_permissions(entry_path, perms);
        }
        if entry_path.is_file() || entry_path.is_symlink() {
            // 文件被进程持有时删除失败：记录第一个失败，继续清理其余文件
            if let Err(e) = fs::remove_file(entry_path) {
                if first_file_error.is_none() {
                    first_file_error = Some((entry_path.to_path_buf(), e));
                }
            }
        } else if entry_path.is_dir() && entry_path != path {
            if let Err(e) = fs::remove_dir(entry_path) {
                // 记录第一个失败的子目录，用于最终错误消息诊断
                if first_dir_error.is_none() {
                    first_dir_error = Some((entry_path.to_path_buf(), e));
                }
            }
        }
    }

    // 最终删除根目录；优先用文件失败信息（更具体，能指出被占用项）
    fs::remove_dir(path).map_err(|e| {
        if let Some((failed_file, file_err)) = first_file_error {
            std::io::Error::new(
                file_err.kind(),
                format!(
                    "文件删除失败（被占用：{}，原因：{}）；根目录删除失败：{}",
                    failed_file.display(), file_err, e
                ),
            )
        } else if let Some((failed_dir, dir_err)) = first_dir_error {
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

/// 创建目录链接，自动选择最兼容的方式：
/// - 优先 Junction（无需软链接权限，跨本地盘也通常可用）
/// - Junction 失败时再降级软链接
///
/// 返回 Ok(link_type_str) 供日志记录，Err 附带用户可读的原因和解决方案
#[cfg(windows)]
pub(crate) fn create_directory_link(target: &Path, link: &Path) -> Result<&'static str, String> {
    // Junction 不依赖开发者模式/管理员软链接权限，优先尝试可降低跨盘迁移失败率。
    match junction::create(target, link) {
        Ok(_) => return Ok("Junction"),
        Err(e) => {
            log_warn!("migration", "Junction 创建失败，降级软链接: {}", e);
        }
    }

    // Junction 降级：尝试软链接，兼容少数 Junction 不可用的文件系统或路径形态。
    match symlink_dir(target, link) {
        Ok(_) => Ok("Symlink"),
        Err(e) => {
            let os_err = e.raw_os_error().unwrap_or(0);
            let reason = if os_err == 1314 || os_err == 5 {
                format!(
                    "创建目录链接失败：权限不足（错误码 {}）。\n\n\
                     Junction 和软链接均失败，请以管理员身份运行本程序后重试。",
                    os_err
                )
            } else {
                format!(
                    "创建目录链接失败（错误码 {}）：{}\n\n\
                     请以管理员身份运行本程序后重试。",
                    os_err, e
                )
            };
            Err(reason)
        }
    }
}

/// 删除临时目录链接只用 remove_dir，避免 remove_dir_all 误递归到链接目标。
#[cfg(windows)]
fn remove_directory_link(link: &Path) -> Result<(), String> {
    fs::remove_dir(link)
        .map_err(|e| format!("清理临时目录链接失败 {}: {}", link.display(), e))
}

/// 在删除源目录前先用临时名称验证链接能力，失败时源目录仍完整保留。
#[cfg(windows)]
fn preflight_directory_link(target: &Path, source: &Path) -> Result<(), String> {
    let parent = source.parent()
        .ok_or_else(|| format!("源目录缺少父目录，无法预检链接: {}", source.display()))?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let probe_name = format!(".viap_link_probe_{}_{}", std::process::id(), timestamp);
    let probe_link = parent.join(probe_name);

    if probe_link.exists() || probe_link.is_symlink() {
        return Err(format!("临时链接路径已存在: {}", probe_link.display()));
    }

    // 先验证同一父目录下可创建链接；若失败直接中止，不删除源目录。
    create_directory_link(target, &probe_link)?;
    remove_directory_link(&probe_link)
}

/// 为源目录生成同父目录下的临时备份路径。
///
/// 迁移切换必须使用同卷 rename，临时目录放在源目录父级可以保证改名是原子的，
/// 同时避免把正在迁移的目录再次纳入目标路径扫描。
#[cfg(windows)]
fn create_migration_backup_path(source: &Path) -> Result<PathBuf, String> {
    let parent = source
        .parent()
        .ok_or_else(|| format!("源目录缺少父目录，无法创建迁移备份：{}", source.display()))?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);

    for attempt in 0..10 {
        let backup_name = format!(
            ".viap_migration_backup_{}_{}_{}",
            std::process::id(),
            timestamp,
            attempt
        );
        let backup_path = parent.join(backup_name);

        // symlink_metadata 能识别悬空重解析点，避免临时路径碰撞后误改用户目录。
        if backup_path.symlink_metadata().is_err() {
            return Ok(backup_path);
        }
    }

    Err(format!(
        "无法创建唯一的迁移备份路径，请清理 {} 下的 .viap_migration_backup_* 后重试",
        parent.display()
    ))
}

/// 校验原路径已经是指向预期目标的目录链接。
///
/// create_directory_link 返回成功后仍做一次实际路径确认，防止链接创建过程被外部程序
/// 干扰时误进入清理备份阶段。
#[cfg(windows)]
fn verify_directory_link(link: &Path, target: &Path) -> Result<(), String> {
    if !crate::utils::is_junction(link) {
        return Err(format!("原路径未成为目录链接：{}", link.display()));
    }

    let actual_target = crate::utils::get_junction_target(link)
        .ok_or_else(|| format!("无法读取目录链接目标：{}", link.display()))?;
    let expected = fs::canonicalize(target)
        .map_err(|e| format!("无法解析目标目录：{}，原因：{}", target.display(), e))?;
    let actual = fs::canonicalize(Path::new(&actual_target))
        .map_err(|e| format!("无法解析目录链接目标：{}，原因：{}", actual_target, e))?;

    if actual != expected {
        return Err(format!(
            "目录链接目标不一致：预期 {}，实际 {}",
            expected.display(),
            actual.display()
        ));
    }

    Ok(())
}

/// 在链接创建失败时，只移除 Viap 创建的目录链接并恢复原目录。
///
/// 对非链接目录绝不递归删除，避免异常状态下把用户新建的数据目录当成半成品清理。
#[cfg(windows)]
fn restore_source_from_backup(source: &Path, backup: &Path, target: &Path) -> Result<(), String> {
    if crate::utils::is_junction(source) {
        verify_directory_link(source, target)?;
        remove_directory_link(source)?;
    } else if source.symlink_metadata().is_ok() {
        return Err(format!(
            "原路径出现非目录链接内容，拒绝覆盖以保护数据：{}",
            source.display()
        ));
    }

    fs::rename(backup, source).map_err(|e| {
        format!(
            "恢复原目录失败：{} -> {}，原因：{}",
            backup.display(),
            source.display(),
            e
        )
    })
}

#[cfg(windows)]
fn rollback_restore_link(original_path: &Path, target_path: &Path) -> String {
    let cleanup_result = if original_path.exists() && !crate::utils::is_junction(original_path) {
        remove_directory_robust(original_path)
            .map_err(|e| format!("清理原路径半成品失败: {}", e))
    } else {
        Ok(())
    };

    let link_result = if !original_path.exists() {
        create_directory_link(target_path, original_path)
            .map(|_| ())
            .map_err(|e| format!("重建目录链接失败: {}", e))
    } else {
        Ok(())
    };

    match (cleanup_result, link_result) {
        (Ok(_), Ok(_)) => format!(
            "已自动恢复目录链接，数据仍完整保存在：{}",
            target_path.display()
        ),
        (cleanup, link) => format!(
            "自动回滚未完全成功。目标数据仍完整保存在：{}；cleanup={:?}；link={:?}",
            target_path.display(), cleanup.err(), link.err()
        ),
    }
}

#[cfg(windows)]
pub(crate) fn restore_directory_with_progress(
    original_path: &Path,
    target_path: &Path,
    task_id: &str,
    app_handle: &tauri::AppHandle,
) -> Result<RestoreDirectoryResult, String> {
    let cancel_flag = Arc::new(AtomicBool::new(false));
    let restore_plan = build_copy_plan_with_progress(
        target_path,
        original_path,
        task_id,
        &cancel_flag,
        app_handle,
    )?;
    let total_size = restore_plan.total_size;

    let original_parent = original_path
        .parent()
        .ok_or("无法获取原路径的父目录")?;
    crate::utils::check_disk_space_for_restore(original_parent, total_size)?;

    emit_progress(app_handle, task_id, 9.0, "linking", "正在移除原目录链接...", 0, total_size);
    if crate::utils::is_junction(original_path) {
        fs::remove_dir(original_path)
            .map_err(|e| format!("删除目录链接失败: {}", e))?;
    } else if original_path.exists() {
        return Err(format!(
            "原路径 {} 已存在且不是目录链接，拒绝恢复以保护数据。",
            original_path.display()
        ));
    }

    fs::create_dir_all(original_path)
        .map_err(|e| format!("创建原路径目录失败 {}: {}", original_path.display(), e))?;

    if let Err(e) = copy_dir_with_progress(restore_plan, task_id, &cancel_flag, app_handle) {
        let rollback = rollback_restore_link(original_path, target_path);
        return Err(format!("恢复复制失败：{}\n{}", e, rollback));
    }

    emit_progress(app_handle, task_id, 90.0, "verifying", "正在校验恢复完整性...", total_size, total_size);
    let restored_size = crate::utils::get_dir_size_safe(original_path);
    let tolerance = (total_size as f64 * 0.01) as u64 + 1024 * 1024;
    if (restored_size as i64 - total_size as i64).abs() > tolerance as i64 {
        let rollback = rollback_restore_link(original_path, target_path);
        return Err(format!(
            "恢复完整性校验失败：预期 {}，实际 {}。\n{}",
            format_bytes(total_size),
            format_bytes(restored_size),
            rollback
        ));
    }

    emit_progress(app_handle, task_id, 96.0, "linking", "正在清理目标副本...", restored_size, total_size);
    let cleanup_warning = if target_path.exists() {
        remove_directory_robust(target_path)
            .err()
            .map(|e| format!("恢复已完成，但目标副本清理失败：{}", e))
    } else {
        None
    };

    emit_progress(app_handle, task_id, 100.0, "done", "恢复完成", restored_size, total_size);
    Ok(RestoreDirectoryResult { restored_size, cleanup_warning })
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
                        "🚫 无法迁移：{label} 属于「{category}」，不支持迁移。\n\n{tip}",
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
/// 将应用从源路径迁移到目标路径，并创建目录链接（同卷 Junction / 跨卷软链接）
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
                // 清理失败（多为 explorer/服务加载的 DLL 被占用）时迁移仍成功，
                // 但必须给出明确提示，避免残留备份目录被误认成新应用。
                let cleanup_warning = remove_directory_robust(&backup_path).err().map(|e| {
                    format!(
                        "临时备份目录清理失败，仍保留在：{}。\n\
                         原因：{}。\n\
                         该目录已被扫描过滤，不会被识别为新应用；\n\
                         其中被占用的文件通常在注销或重启系统后释放，届时可手动删除此目录。",
                        backup_path.display(),
                        e
                    )
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
