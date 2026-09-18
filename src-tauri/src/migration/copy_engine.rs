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

use super::links::create_directory_link;
use crate::migration::format_bytes;

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
    /// 源根目录：相对路径据此还原为绝对路径
    pub(crate) source_root: PathBuf,
    /// 目标根目录
    pub(crate) target_root: PathBuf,
    /// 待复制文件（相对源根目录的路径 + 大小）
    ///
    /// 只存相对路径而不是源/目标两份绝对路径：大目录（几十万文件）时
    /// 每个条目能省下一份长前缀，峰值内存明显下降。
    pub(crate) file_list: Vec<(PathBuf, u64)>,
    /// 待创建的空目录（相对路径），保留它们避免应用依赖占位目录时异常
    pub(crate) dir_list: Vec<PathBuf>,
    /// 待重建的目录链接（源目录内嵌套的联接 / 目录符号链接）
    pub(crate) link_list: Vec<PlannedLink>,
    /// 源目录内是否存在可执行文件（深度 5 以内），决定是否需要做进程占用预检
    pub(crate) has_executable: bool,
    /// 计划复制的总字节数，用于空间检查和复制进度计算。
    pub(crate) total_size: u64,
}

/// 待重建的目录链接
pub(crate) struct PlannedLink {
    /// 链接在源树中的相对位置（目标盘上的位置由它推导）
    pub(crate) relative_path: PathBuf,
    /// 链接应指向的位置（原本指向源树内部时会改写到目标树）
    pub(crate) target: PathBuf,
}

/// 扫描时对重解析点（联接 / 符号链接）的处理方式
enum ReparsePlan {
    /// 目录链接：在目标盘重建链接本身
    DirectoryLink(PlannedLink),
    /// 文件链接：直接复制其内容（符号链接需要开发者模式或管理员权限，内容自包含更稳妥）
    FileContent { size: u64 },
}

/// 进度上报回调：与 Tauri 解耦，单测可直接跑完整的「扫描 + 复制 + 重建链接」流程
/// 参数依次为：(步骤, 百分比, 文案, 已处理字节, 总字节)
/// 带 Sync 约束是因为并行复制分支要在多线程中共享同一个回调。
pub(crate) type ProgressReporter<'a> = &'a (dyn Fn(&str, f64, &str, u64, u64) + Sync);

pub(crate) fn build_copy_plan(
    source: &Path,
    target: &Path,
    cancel_flag: &Arc<AtomicBool>,
    progress: ProgressReporter<'_>,
) -> Result<CopyPlan, String> {
    progress("counting", 1.0, "正在扫描文件列表...", 0, 0);

    let mut file_list: Vec<(PathBuf, u64)> = Vec::new();
    let mut dir_list: Vec<PathBuf> = Vec::new();
    let mut link_list: Vec<PlannedLink> = Vec::new();
    let mut total_size: u64 = 0;
    let mut scanned_files: u64 = 0;
    let mut has_executable = false;
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

        // 重解析点必须单独判断：Windows 下目录联接既不是 is_dir() 也不是 is_file()，
        // 按普通目录/文件分流会被静默丢弃（迁移后链接消失，且体积校验发现不了）
        if entry.file_type().is_symlink() {
            match plan_reparse_point(entry.path(), source, target) {
                Some(ReparsePlan::DirectoryLink(link)) => link_list.push(link),
                Some(ReparsePlan::FileContent { size }) => {
                    total_size += size;
                    scanned_files += 1;
                    has_executable |= is_executable_within_depth(rel_path, 5);
                    file_list.push((rel_path.to_path_buf(), size));
                }
                // 悬空链接没有可复制的内容，跳过（已在 plan_reparse_point 中记录日志）
                None => {}
            }
        } else if entry.file_type().is_dir() {
            dir_list.push(rel_path.to_path_buf());
        } else if entry.file_type().is_file() {
            let size = entry.metadata()
                .map_err(|e| format!("读取文件元数据失败 {}: {}", entry.path().display(), e))?
                .len();
            total_size += size;
            scanned_files += 1;
            has_executable |= is_executable_within_depth(rel_path, 5);
            file_list.push((rel_path.to_path_buf(), size));
        }

        if last_emit.elapsed() >= Duration::from_millis(250) {
            // 扫描阶段没有总文件数，百分比只表示整体迁移已进入准备区间，真实进展放在文案里。
            let percent = (1.0 + (scanned_files as f64 / 500.0)).min(8.0);
            progress(
                "counting",
                percent,
                &format!("已扫描 {} 个文件，{}", scanned_files, format_bytes(total_size)),
                total_size,
                0,
            );
            last_emit = Instant::now();
        }
    }

    progress(
        "counting",
        9.0,
        &format!("扫描完成：{} 个文件，{}", scanned_files, format_bytes(total_size)),
        total_size,
        total_size,
    );

    Ok(CopyPlan {
        source_root: source.to_path_buf(),
        target_root: target.to_path_buf(),
        file_list,
        dir_list,
        link_list,
        has_executable,
        total_size,
    })
}

/// 判断相对路径是否为可执行文件且深度不超过 `max_depth`
///
/// 与旧的独立 WalkDir(max_depth = 5) 探测等价：源根目录的直接子项深度为 1。
/// 合并进复制计划后，同一次遍历同时产出文件清单、总大小和该项判断。
fn is_executable_within_depth(relative_path: &Path, max_depth: usize) -> bool {
    if relative_path.components().count() > max_depth {
        return false;
    }
    relative_path
        .extension()
        .map(|extension| extension.eq_ignore_ascii_case("exe"))
        .unwrap_or(false)
}

/// 规划单个重解析点的处理方式；返回 None 表示该链接被跳过（悬空）
fn plan_reparse_point(link_path: &Path, source: &Path, target: &Path) -> Option<ReparsePlan> {
    let relative_path = link_path.strip_prefix(source).ok()?.to_path_buf();

    // fs::metadata 跟随链接：据此判断链接指向目录还是文件；悬空链接在此报错
    let Ok(metadata) = fs::metadata(link_path) else {
        log_warn!(
            "migration",
            "跳过悬空目录链接（目标不存在）: {}",
            link_path.display()
        );
        return None;
    };

    if !metadata.is_dir() {
        return Some(ReparsePlan::FileContent { size: metadata.len() });
    }

    // 读取链接目标：junction 读出来可能带 \\?\ 前缀，必须走带归一化的工具函数，
    // 否则 strip_prefix(source) 会失配，指向源树内部的链接不会被改写
    let raw_target = crate::utils::get_junction_target(link_path)
        .map(PathBuf::from)
        .or_else(|| fs::read_link(link_path).ok())?;
    let resolved_target = resolve_link_target(link_path, &raw_target);
    // 指向源树内部时必须改写到目标树，否则迁移后链接会指向已被删除的旧位置
    let rewritten_target = match resolved_target.strip_prefix(source) {
        Ok(inner) => target.join(inner),
        Err(_) => resolved_target,
    };

    Some(ReparsePlan::DirectoryLink(PlannedLink {
        relative_path,
        target: rewritten_target,
    }))
}

/// 链接目标可能是相对路径，按链接自身所在目录解析为绝对路径
fn resolve_link_target(link_path: &Path, link_target: &Path) -> PathBuf {
    if link_target.is_absolute() {
        return link_target.to_path_buf();
    }
    match link_path.parent() {
        Some(parent) => parent.join(link_target),
        None => link_target.to_path_buf(),
    }
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
    /// 进度上报回调（并行复制要求 Sync，故类型上带 Sync 约束）
    progress: ProgressReporter<'a>,
}

/// 按全局已复制字节数计算进度并 CAS 节流上报（复制回调与复制完成补账共用）。
fn try_report_progress(
    progress: ProgressReporter<'_>,
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
        progress(
            "copying",
            current_pct as f64,
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
            ctx.progress,
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
    } else if code == 3 || code == 206 {
        // 3 = ERROR_PATH_NOT_FOUND，206 = ERROR_FILENAME_EXCED_RANGE：
        // 修复长路径后仍出现时多为目标盘目录被外部删除或路径本身非法
        format!(
            "复制文件失败 {}（错误码 {}：路径不存在或路径过长）\n\
             请确认源文件与目标目录仍然存在；若目标目录被移动或删除，请重新选择迁移目录后重试。",
            src.display(),
            code
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
    progress: ProgressReporter<'_>,
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

    // 必须使用扩展长度路径：程序未声明 longPathAware，原生 CopyFileExW 在
    // 超过 260 字符时返回 ERROR_PATH_NOT_FOUND(3)（Yarn / npm 缓存极易触发）
    let src_wide = crate::utils::to_extended_length_wide(src);
    let dest_wide = crate::utils::to_extended_length_wide(dest);

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
        progress,
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
        try_report_progress(progress, last_report_pct, new_copied, total_size);
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
    _progress: ProgressReporter<'_>,
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
pub(crate) fn copy_dir(
    plan: CopyPlan,
    cancel_flag: &Arc<AtomicBool>,
    progress: ProgressReporter<'_>,
) -> Result<(u64, u64), String> {
    let CopyPlan { source_root, target_root, file_list, dir_list, link_list, total_size, .. } = plan;

    // 阶段 1：预建所有目标目录；空目录也必须迁移，否则部分应用会因缺少占位目录异常。
    {
        let mut dirs: std::collections::BTreeSet<PathBuf> = std::collections::BTreeSet::new();
        for dir in dir_list {
            dirs.insert(target_root.join(dir));
        }
        for (relative_path, _) in &file_list {
            if let Some(parent) = relative_path.parent() {
                if !parent.as_os_str().is_empty() {
                    dirs.insert(target_root.join(parent));
                }
            }
        }
        // 链接自身不能在目标盘建成真实目录，否则重建链接时会因路径已存在而失败
        for link in &link_list {
            if let Some(parent) = link.relative_path.parent() {
                if !parent.as_os_str().is_empty() {
                    dirs.insert(target_root.join(parent));
                }
            }
        }
        for dir in dirs {
            fs::create_dir_all(&dir)
                .map_err(|e| format!("创建目录失败 {}: {}", dir.display(), e))?;
        }
    }

    if total_size == 0 && link_list.is_empty() {
        progress("copying", 88.0, "源目录为空，已复制目录结构", 0, 0);
        return Ok((0, 0));
    }

    // 阶段 2：按大小分流复制
    // 大文件顺序单流复制保证 SSD 顺序吞吐峰值；小文件 8 线程并行分摊元数据开销
    // （全核并发会争抢 NTFS 元数据锁，实测反而降低总吞吐）
    let (mut large_files, small_files): (Vec<_>, Vec<_>) = file_list
        .into_iter()
        .partition(|(_, size)| *size > LARGE_FILE_THRESHOLD);
    // 大文件按体积降序，优先搬走最大的文件
    large_files.sort_by(|a, b| b.1.cmp(&a.1));

    if total_size > 0 {
        progress("copying", 10.0, "开始复制文件...", 0, total_size);
    }

    let internal_cancel = Arc::new(AtomicBool::new(false));
    let copied_size = Arc::new(AtomicU64::new(0));
    let skipped_size = Arc::new(AtomicU64::new(0));
    let last_report_pct = Arc::new(AtomicU64::new(0));
    let error_slot: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    // 大文件顺序复制：一次一个大文件，避免多条大流同时打盘
    for (relative_path, size) in &large_files {
        let src = source_root.join(relative_path);
        let dest = target_root.join(relative_path);
        // 任一条件触发即跳过：已有错误 / 用户取消 / 内部取消
        if error_slot.lock().unwrap().is_some()
            || internal_cancel.load(Ordering::Relaxed)
            || cancel_flag.load(Ordering::Relaxed)
        {
            break;
        }
        match copy_file_with_cancel(
            &src,
            &dest,
            cancel_flag,
            &internal_cancel,
            &copied_size,
            &last_report_pct,
            total_size,
            progress,
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
            small_files.par_iter().for_each(|(relative_path, size)| {
                // 任一条件触发即跳过：已有错误 / 用户取消 / 内部取消
                if error_slot.lock().unwrap().is_some()
                    || internal_cancel.load(Ordering::Relaxed)
                    || cancel_flag.load(Ordering::Relaxed)
                {
                    return;
                }
                let src = source_root.join(relative_path);
                let dest = target_root.join(relative_path);
                match copy_file_with_cancel(
                    &src,
                    &dest,
                    cancel_flag,
                    &internal_cancel,
                    &copied_size,
                    &last_report_pct,
                    total_size,
                    progress,
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

    // 阶段 3：重建源目录内的目录链接（放在文件之后，保证链接指向的位置已就绪）
    if !link_list.is_empty() {
        progress("linking", 89.0, "正在重建目录链接...", total_size, total_size);
        for link in &link_list {
            if cancel_flag.load(Ordering::Relaxed) {
                return Err("用户取消了迁移".to_string());
            }
            let link_dest = target_root.join(&link.relative_path);
            // 链接创建失败必须让整个迁移失败：目标树缺链接等于结构不完整，
            // 此时源目录尚未删除，中止后数据仍然安全
            create_directory_link(&link.target, &link_dest).map_err(|e| {
                format!(
                    "重建目录链接失败 {} -> {}: {}",
                    link_dest.display(),
                    link.target.display(),
                    e
                )
            })?;
        }
    }

    let skipped_size = skipped_size.load(Ordering::Relaxed);
    // 返回 WalkDir 阶段统计的 total_size 而非 AtomicU64 累加的 copied_size，
    // 确保完整性校验基准不受并行取消影响
    Ok((total_size, skipped_size))
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// 构造一条超过 260 字符的深层目录，返回（普通路径, 扩展长度路径）
    fn create_deep_directory(root: &Path, segments: usize) -> (PathBuf, String) {
        let leaf = "x".repeat(30);
        let mut plain = root.to_path_buf();
        for index in 0..segments {
            plain.push(format!("segment-{}-{}", index, leaf));
        }
        let extended = crate::utils::to_extended_length_wide(&plain);
        let extended_text = String::from_utf16_lossy(&extended[..extended.len() - 1]);
        std::fs::create_dir_all(Path::new(&extended_text)).expect("创建深层目录失败");
        (plain, extended_text)
    }

    /// 长路径回归测试：原生 CopyFileExW 必须带 `\\?\` 前缀，
    /// 否则 Yarn / npm 缓存这类深层目录会以 ERROR_PATH_NOT_FOUND(3) 失败。
    #[test]
    fn copy_file_ex_w_supports_paths_longer_than_max_path() {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("系统时间应有效")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("viap-longpath-{suffix}"));
        std::fs::create_dir_all(&root).expect("创建测试根目录失败");

        let (plain_dir, extended_dir) = create_deep_directory(&root, 6);
        let source = plain_dir.join("payload.bin");
        assert!(
            source.to_string_lossy().len() > 260,
            "测试路径需要超过 MAX_PATH，实际 {}",
            source.to_string_lossy().len()
        );
        std::fs::write(
            Path::new(&format!(r"{}\payload.bin", extended_dir)),
            b"long-path-payload",
        )
        .expect("写入测试文件失败");

        let destination = root.join("copied.bin");
        let source_wide = crate::utils::to_extended_length_wide(&source);
        let destination_wide = crate::utils::to_extended_length_wide(&destination);

        let copied = unsafe {
            CopyFileExW(
                PCWSTR(source_wide.as_ptr()),
                PCWSTR(destination_wide.as_ptr()),
                None,
                None,
                None,
                COPY_FILE_ALLOW_DECRYPTED_DESTINATION,
            )
        };
        assert!(copied.is_ok(), "扩展长度路径复制应当成功");
        assert_eq!(
            std::fs::metadata(&destination).map(|meta| meta.len()).unwrap_or(0),
            17
        );

        let _ = std::fs::remove_dir_all(Path::new(&format!(r"\\?\{}", root.display())));
    }
}

/// 端到端验证迁移引擎：一次遍历产出计划 → 复制文件 → 重建目录链接
#[cfg(all(test, windows))]
mod engine_tests {
    use super::*;
    use std::sync::Mutex;

    fn same_path(left: &Path, right: &Path) -> bool {
        let normalize = |path: &Path| {
            path.to_string_lossy().trim_start_matches(r"\\?\").trim_end_matches('\\').to_lowercase()
        };
        normalize(left) == normalize(right)
    }

    fn read_text(path: &Path) -> String {
        std::fs::read_to_string(path).expect("读取文件失败")
    }

    /// 构造超过 260 字符的深层目录并写入文件，返回（普通路径, 扩展长度路径）
    fn create_deep_directory(root: &Path, segments: usize, file_name: &str, content: &str) -> PathBuf {
        let leaf = "y".repeat(30);
        let mut plain = root.to_path_buf();
        for index in 0..segments {
            plain.push(format!("deep-{}-{}", index, leaf));
        }
        let extended = crate::utils::to_extended_length_wide(&plain);
        let extended_text = String::from_utf16_lossy(&extended[..extended.len() - 1]);
        std::fs::create_dir_all(Path::new(&extended_text)).expect("创建深层目录失败");
        std::fs::write(
            Path::new(&format!(r"{}\{}", extended_text, file_name)),
            content,
        )
        .expect("写入深层文件失败");
        plain.join(file_name)
    }

    #[test]
    fn copies_tree_and_recreates_nested_directory_links() {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("系统时间应有效")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("viap-engine-{suffix}"));
        let source = root.join("source");
        let target = root.join("target");
        let external = root.join("external-data");

        // 外部数据目录（被联接引用，位于源树之外）
        std::fs::create_dir_all(&external).expect("创建外部目录失败");
        std::fs::write(external.join("external.bin"), "external").expect("写入外部文件失败");

        // 源树：普通文件 + 空目录 + 内部联接 + 外部联接 + 超长路径文件
        std::fs::create_dir_all(source.join("inner").join("keep")).expect("创建源目录失败");
        std::fs::write(source.join("plain.bin"), "plain").expect("写入普通文件失败");
        std::fs::write(source.join("inner").join("keep").join("a.bin"), "aaa").expect("写入嵌套文件失败");
        std::fs::create_dir_all(source.join("empty-dir")).expect("创建空目录失败");
        junction::create(&external, &source.join("outer-link")).expect("创建外部联接失败");
        junction::create(&source.join("inner"), &source.join("inner-link")).expect("创建内部联接失败");
        let long_file = create_deep_directory(&source.join("deep"), 5, "payload.bin", "long-path");

        let cancel_flag = Arc::new(AtomicBool::new(false));
        let reported_steps: Mutex<Vec<String>> = Mutex::new(Vec::new());
        let reporter = |step: &str, _percent: f64, _message: &str, _copied: u64, _total: u64| {
            reported_steps.lock().unwrap().push(step.to_string());
        };

        let plan = build_copy_plan(&source, &target, &cancel_flag, &reporter).expect("扫描失败");
        assert_eq!(plan.link_list.len(), 2, "两个嵌套联接都必须进入计划");
        assert!(plan.total_size > 0);
        // 一次遍历同时产出「是否含 exe」：该判断决定要不要做进程占用预检
        assert!(!plan.has_executable, "fixture 中没有 exe，不应误报");

        // 指向源树内部的联接必须改写到目标树，否则迁移后会指向已删除的旧位置
        let inner_link = plan.link_list.iter()
            .find(|link| link.relative_path.ends_with("inner-link"))
            .expect("内部联接缺失");
        assert!(same_path(&inner_link.target, &target.join("inner")));
        let outer_link = plan.link_list.iter()
            .find(|link| link.relative_path.ends_with("outer-link"))
            .expect("外部联接缺失");
        assert!(same_path(&outer_link.target, &external));

        let (copied_total, skipped) = copy_dir(plan, &cancel_flag, &reporter).expect("复制失败");
        assert!(skipped == 0);
        assert!(copied_total > 0);

        // 普通文件、嵌套文件、空目录都必须在目标盘出现
        assert_eq!(read_text(&target.join("plain.bin")), "plain");
        assert_eq!(read_text(&target.join("inner").join("keep").join("a.bin")), "aaa");
        assert!(target.join("empty-dir").is_dir(), "空目录必须保留");
        // 超长路径文件（>260 字符）也要复制成功
        let long_relative = long_file.strip_prefix(&source).expect("长路径解析失败");
        assert_eq!(read_text(&target.join(long_relative)), "long-path");

        // 两个联接都必须是「联接」，并且可以真正读到数据
        assert!(crate::utils::is_junction(&target.join("outer-link")));
        assert!(crate::utils::is_junction(&target.join("inner-link")));
        assert_eq!(read_text(&target.join("outer-link").join("external.bin")), "external");
        assert_eq!(read_text(&target.join("inner-link").join("keep").join("a.bin")), "aaa");
        // 内部联接改写后不能还指向老位置
        let inner_target = crate::utils::get_junction_target(&target.join("inner-link")).unwrap_or_default();
        assert!(
            !same_path(Path::new(&inner_target), &source.join("inner")),
            "内部联接仍指向旧的源位置：{}",
            inner_target
        );

        assert!(
            reported_steps.lock().unwrap().iter().any(|step| step == "linking"),
            "重建链接阶段应当上报进度"
        );

        // 清理：先删链接再删目录，避免踩到联接目标
        std::fs::remove_dir(target.join("outer-link")).ok();
        std::fs::remove_dir(target.join("inner-link")).ok();
        std::fs::remove_dir(source.join("outer-link")).ok();
        std::fs::remove_dir(source.join("inner-link")).ok();
        std::fs::remove_dir_all(&root).ok();
    }
}