// 复制引擎子模块
// 扫描生成复制计划、CopyFileExW 原生复制（大文件顺序/小文件并行）、进度上报与取消

use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use rayon::prelude::*;
use walkdir::WalkDir;

use crate::migration::{emit_progress, format_bytes};

#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
#[cfg(windows)]
use windows::core::PCWSTR;
#[cfg(windows)]
use windows::Win32::Foundation::{GetLastError, HANDLE, WIN32_ERROR};
#[cfg(windows)]
use windows::Win32::Storage::FileSystem::{CopyFileExW, LPPROGRESS_ROUTINE_CALLBACK_REASON};

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
pub(crate) fn copy_dir_with_progress(
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
