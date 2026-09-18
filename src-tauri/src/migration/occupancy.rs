// 文件占用检测子模块
// 以独占模式打开文件探测占用，用于迁移前预检

use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

use rayon::prelude::*;

use std::path::{Path, PathBuf};

use super::copy_engine::{CopyPlan, ProgressReporter};

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
/// 文件清单直接复用复制计划（`CopyPlan`）：早前这里会再整棵遍历一次磁盘，
/// 大目录（Yarn/npm 缓存、node_modules）冷缓存下等于把同一份数据读两遍。
///
/// 返回：被占用文件的相对路径列表，最多 10 条；空列表表示无占用。
/// 取消时返回占位列表 ["检测已取消"]，由调用方按取消处理。
///
/// 实现说明：文件打开探测为独立系统调用，用 rayon 并行执行，
/// 大目录（数万文件）下可显著缩短预检耗时。
#[cfg(windows)]
pub(crate) fn check_file_locks(
    plan: &CopyPlan,
    cancel_flag: &Arc<AtomicBool>,
    progress: ProgressReporter<'_>,
) -> Vec<String> {
    use std::os::windows::fs::OpenOptionsExt;

    if cancel_flag.load(Ordering::Relaxed) {
        return vec!["检测已取消".to_string()];
    }

    let source_root = &plan.source_root;
    let total_files = plan.file_list.len() as u64;
    let locked_files: Mutex<Vec<String>> = Mutex::new(Vec::new());
    // 已收集满上限或用户取消时置位，其余线程立即退出
    let done = AtomicBool::new(false);
    let checked_count = AtomicU64::new(0);
    let last_report = AtomicU64::new(0);

    // 高风险扩展名优先：被加载的 DLL / 运行中的 exe 才会阻塞复制与备份清理，
    // 先查它们可以在真正有占用时立刻失败，而不是把整棵树扫完才报错
    let (risky, others): (Vec<&PathBuf>, Vec<&PathBuf>) = plan
        .file_list
        .iter()
        .map(|(relative_path, _)| relative_path)
        .partition(|relative_path| has_risky_extension(relative_path));

    let report_progress = |checked: u64| {
        // 每 2000 个文件上报一次，避免高频事件拖慢检测本身
        let previous = last_report.load(Ordering::Relaxed);
        if checked < previous + 2000 && checked != total_files {
            return;
        }
        if last_report
            .compare_exchange(previous, checked, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
        {
            return;
        }
        let percent = (checked as f64 / total_files.max(1) as f64 * 9.0).min(9.0);
        progress(
            "checking",
            percent,
            &format!("正在检测文件占用 {}/{}", checked, total_files),
            0,
            0,
        );
    };

    for group in [risky, others] {
        group.par_iter().for_each(|relative_path| {
            if done.load(Ordering::Relaxed) || cancel_flag.load(Ordering::Relaxed) {
                return;
            }
            // 每 200 个文件检查一次取消标志（并行下仍保留节流，避免无谓打开）
            let count = checked_count.fetch_add(1, Ordering::Relaxed) + 1;
            if count % 200 == 0 {
                if cancel_flag.load(Ordering::Relaxed) {
                    return;
                }
                report_progress(count);
            }

            let path = source_root.join(relative_path);
            // FILE_SHARE_NONE = 0，独占打开。若其他进程持有该文件句柄则失败。
            let result = fs::OpenOptions::new()
                .read(true)
                .share_mode(0)
                .open(&path);

            if let Err(e) = result {
                let os_err = e.raw_os_error().unwrap_or(0);
                // 32 = ERROR_SHARING_VIOLATION（文件被其他进程打开且不允许共享）
                // 5  = ERROR_ACCESS_DENIED（无访问权限，通常也意味着被占用）
                if os_err == 32 || os_err == 5 {
                    let mut list = locked_files.lock().unwrap();
                    if list.len() < 10 {
                        list.push(relative_path.to_string_lossy().to_string());
                        if list.len() >= 10 {
                            list.push("...（更多文件被占用）".to_string());
                            done.store(true, Ordering::Relaxed);
                        }
                    }
                }
                // 其他错误（文件消失等竞态）忽略，不视为占用
            }
        });
    }
    report_progress(total_files);

    if cancel_flag.load(Ordering::Relaxed) {
        return vec!["检测已取消".to_string()];
    }
    let mut result = locked_files.into_inner().unwrap();
    // 并行收集顺序不定，按路径排序让提示稳定
    result.sort();
    result
}

/// 非 Windows 平台回退：无占用检测，返回空列表
#[cfg(not(windows))]
pub(crate) fn check_file_locks(
    _plan: &CopyPlan,
    _cancel_flag: &Arc<AtomicBool>,
    _progress: ProgressReporter<'_>,
) -> Vec<String> {
    Vec::new()
}

/// 判断是否为「容易被加载 / 正在运行」的文件类型
///
/// 这类文件被占用时复制会失败、备份也删不掉，优先检测它们。
#[cfg(windows)]
fn has_risky_extension(relative_path: &Path) -> bool {
    const RISKY: &[&str] = &[
        "exe", "dll", "sys", "node", "so", "dylib", "ocx", "cpl", "scr", "drv", "pdb", "tmp",
    ];
    relative_path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| RISKY.iter().any(|risky| extension.eq_ignore_ascii_case(risky)))
        .unwrap_or(false)
}
