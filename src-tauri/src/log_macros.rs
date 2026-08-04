// 统一后端日志宏
// 替代零散的 eprintln!，所有日志统一前缀格式

/// 日志级别标记：INFO / WARN / ERROR
macro_rules! orbit_log {
    ($level:expr, $module:expr, $($arg:tt)*) => {
        eprintln!("[viap][{}][{}] {}", $level, $module, format!($($arg)*))
    };
}

macro_rules! log_warn {
    ($module:expr, $($arg:tt)*) => { orbit_log!("WARN", $module, $($arg)*) };
}

