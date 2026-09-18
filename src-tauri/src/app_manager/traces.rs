// 系统痕迹检测（服务 / 驱动 / 计划任务）
//
// 卸载应用后，这些痕迹往往仍然存在：残留的服务会继续在后台运行、计划任务会定期拉起
// 已删除的程序。它们不在文件系统的"残留"范畴里，但用户最需要知道。
//
// 安全边界：这里**只读检测、只做提示**，绝不自动删除。删除服务/驱动/计划任务会
// 直接影响系统与服务管理器状态，交给用户自己按官方方式处理更稳妥。
//
// 性能：全部走注册表枚举（不调用 PowerShell、不扫描磁盘），只应作为"打开卸载确认时"
// 的按需调用，不参与应用列表扫描。

use serde::Serialize;
use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ};
use winreg::RegKey;

/// 系统痕迹条目
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SystemTrace {
    /// 类型：service | driver | task
    pub kind: String,
    /// 名称（服务名 / 任务名）
    pub name: String,
    /// 命中原因或可执行路径，便于用户确认
    pub detail: String,
}

/// 痕迹类型：服务
const KIND_SERVICE: &str = "service";
/// 痕迹类型：内核驱动
const KIND_DRIVER: &str = "driver";
/// 痕迹类型：计划任务
const KIND_TASK: &str = "task";

/// 单次检测返回上限，避免命中过多时刷屏
const MAX_TRACES: usize = 30;
/// 计划任务树的最大遍历深度（任务名通常形如 \厂商\产品\任务）
const MAX_TASK_DEPTH: usize = 3;

/// 与服务/任务名匹配时需要忽略的通用词，避免"update"这种词命中一大堆系统项
const GENERIC_TOKENS: &[&str] = &[
    "app", "application", "service", "services", "update", "updater", "setup", "install",
    "installer", "helper", "client", "server", "manager", "launcher", "runtime", "x64", "x86",
    "the", "and", "for", "inc", "ltd", "llc", "corp", "version", "win", "windows", "microsoft",
];

/// 把应用名切成用于匹配的 token（小写、去通用词、至少 3 个字符）
pub(crate) fn build_match_tokens(app_name: &str) -> Vec<String> {
    app_name
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .map(|token| token.to_lowercase())
        .filter(|token| token.len() >= 3 && !GENERIC_TOKENS.contains(&token.as_str()))
        .collect()
}

/// 判断某个服务/任务是否属于目标应用
///
/// 两条判据（任一成立）：
/// 1. 可执行文件路径落在安装目录内（最强证据，独立于名称）
/// 2. 名称或显示名包含应用名 token
pub(crate) fn trace_matches(
    name: &str,
    display_name: &str,
    image_path: Option<&str>,
    tokens: &[String],
    install_location: Option<&str>,
) -> bool {
    if let (Some(image), Some(location)) = (image_path, install_location) {
        let normalized_image = normalize_image_path(image);
        let normalized_location = location.trim_end_matches('\\').to_lowercase();
        if !normalized_location.is_empty() && normalized_image.starts_with(&normalized_location) {
            return true;
        }
    }

    let haystack = format!("{} {}", name.to_lowercase(), display_name.to_lowercase());
    tokens.iter().any(|token| haystack.contains(token.as_str()))
}

/// 归一化服务可执行路径：去掉引号、`\??\` 前缀，并把 `\SystemRoot\` 还原成 Windows 目录
fn normalize_image_path(image_path: &str) -> String {
    let cleaned = image_path
        .trim()
        .trim_matches('"')
        .trim_start_matches(r"\??\")
        .to_string();
    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
    let expanded = cleaned.replace(r"\SystemRoot\", &format!("{}\\", system_root.trim_end_matches('\\')));
    // 服务路径常带启动参数（如 ...\svchost.exe -k netsvcs），只保留可执行文件部分
    let executable = expanded
        .split(" -")
        .next()
        .unwrap_or(&expanded)
        .trim()
        .trim_matches('"');
    executable.to_lowercase()
}

/// 检测目标应用遗留的服务、驱动与计划任务
pub fn detect_system_traces(
    app_name: &str,
    install_location: Option<&str>,
) -> Vec<SystemTrace> {
    let tokens = build_match_tokens(app_name);
    let install_location = install_location
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    if tokens.is_empty() && install_location.is_none() {
        return Vec::new();
    }

    let mut traces = Vec::new();
    collect_service_traces(&tokens, install_location.as_deref(), &mut traces);
    collect_task_traces(&tokens, &mut traces);

    traces.sort_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    traces.truncate(MAX_TRACES);
    traces
}

/// 枚举服务树，按名称/路径匹配（驱动也在同一棵树里，用 Type 区分）
fn collect_service_traces(
    tokens: &[String],
    install_location: Option<&str>,
    output: &mut Vec<SystemTrace>,
) {
    let services = match RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags(r"SYSTEM\CurrentControlSet\Services", KEY_READ)
    {
        Ok(key) => key,
        Err(error) => {
            log_warn!("uninstall", "读取服务列表失败: {}", error);
            return;
        }
    };

    for service_name in services.enum_keys().filter_map(|name| name.ok()) {
        if output.len() >= MAX_TRACES {
            return;
        }
        let Ok(key) = services.open_subkey_with_flags(&service_name, KEY_READ) else {
            continue;
        };
        let display_name: String = key.get_value("DisplayName").unwrap_or_default();
        let image_path: String = key.get_value("ImagePath").unwrap_or_default();
        // Type: 1/2 为内核驱动，其余为服务
        let service_type: u32 = key.get_value("Type").unwrap_or(0x10);

        if !trace_matches(
            &service_name,
            &display_name,
            Some(&image_path),
            tokens,
            install_location,
        ) {
            continue;
        }

        let kind = if service_type == 1 || service_type == 2 {
            KIND_DRIVER
        } else {
            KIND_SERVICE
        };
        let detail = if display_name.trim().is_empty() {
            image_path.clone()
        } else {
            format!("{} · {}", display_name, image_path)
        };
        output.push(SystemTrace {
            kind: kind.to_string(),
            name: service_name,
            detail: detail.trim_matches(|ch| ch == ' ' || ch == '·').to_string(),
        });
    }
}

/// 枚举计划任务
///
/// 优先读 `%SystemRoot%\System32\Tasks` 下的任务文件：普通用户即可读取；
/// 注册表的 TaskCache 树在非管理员下会被拒绝访问，只作为补充来源。
fn collect_task_traces(tokens: &[String], output: &mut Vec<SystemTrace>) {
    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
    let tasks_dir = std::path::Path::new(&system_root).join("System32").join("Tasks");
    if tasks_dir.is_dir() {
        collect_tasks_from_directory(&tasks_dir, tokens, output);
    }
    if output.len() < MAX_TRACES {
        collect_tasks_from_registry(tokens, output);
    }
}

/// 从任务文件目录收集（文件名即任务名，子目录对应任务路径）
fn collect_tasks_from_directory(
    tasks_dir: &std::path::Path,
    tokens: &[String],
    output: &mut Vec<SystemTrace>,
) {
    for entry in walkdir::WalkDir::new(tasks_dir)
        .max_depth(MAX_TASK_DEPTH)
        .into_iter()
        .filter_map(|entry| entry.ok())
    {
        if output.len() >= MAX_TRACES {
            return;
        }
        if !entry.file_type().is_file() {
            continue;
        }
        let Ok(relative) = entry.path().strip_prefix(tasks_dir) else {
            continue;
        };
        let name = relative.to_string_lossy().replace('/', "\\");
        if !tokens
            .iter()
            .any(|token| name.to_lowercase().contains(token.as_str()))
        {
            continue;
        }
        output.push(SystemTrace {
            kind: KIND_TASK.to_string(),
            name: format!("\\{}", name),
            detail: "计划任务：应用卸载后仍可能被定期拉起".to_string(),
        });
    }
}

/// 从注册表任务树收集（需要管理员权限，取不到就安静跳过）
fn collect_tasks_from_registry(tokens: &[String], output: &mut Vec<SystemTrace>) {
    let Ok(root) = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags(
        r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Schedule\TaskCache\Tree",
        KEY_READ,
    ) else {
        // 非管理员下这里会失败，属于正常情况，不需要打扰用户
        return;
    };

    let mut stack: Vec<(RegKey, String, usize)> = vec![(root, String::new(), 0)];
    while let Some((key, prefix, depth)) = stack.pop() {
        if output.len() >= MAX_TRACES {
            return;
        }
        for child_name in key.enum_keys().filter_map(|name| name.ok()) {
            let Ok(child) = key.open_subkey_with_flags(&child_name, KEY_READ) else {
                continue;
            };
            let full_name = format!("{}\\{}", prefix, child_name);

            if tokens
                .iter()
                .any(|token| child_name.to_lowercase().contains(token.as_str()))
            {
                output.push(SystemTrace {
                    kind: KIND_TASK.to_string(),
                    name: full_name.clone(),
                    detail: "计划任务：应用卸载后仍可能被定期拉起".to_string(),
                });
            }

            if depth + 1 < MAX_TASK_DEPTH {
                stack.push((child, full_name, depth + 1));
            }
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn tokens_drop_generic_and_short_words() {
        let tokens = build_match_tokens("Adobe Acrobat Update Service x64");
        assert!(tokens.contains(&"adobe".to_string()));
        assert!(tokens.contains(&"acrobat".to_string()));
        // 通用词与短词不参与匹配，避免误报一大堆系统项
        assert!(!tokens.contains(&"update".to_string()));
        assert!(!tokens.contains(&"service".to_string()));
        assert!(!tokens.contains(&"x64".to_string()));
    }

    #[test]
    fn matches_by_install_location_even_with_unrelated_name() {
        let tokens = build_match_tokens("Unrelated Name");
        let matched = trace_matches(
            "SomeVendorSvc",
            "Some Vendor Helper",
            Some(r#""C:\Program Files\SomeApp\svc.exe" -k netsvcs"#),
            &tokens,
            Some(r"C:\Program Files\SomeApp"),
        );
        assert!(matched, "可执行文件落在安装目录内时必须命中");
    }

    #[test]
    fn matches_by_name_token_and_ignores_unrelated_services() {
        let tokens = build_match_tokens("Clash Verge");
        assert!(trace_matches(
            "clash_verge_service",
            "Clash Verge Service",
            Some(r"C:\Windows\System32\svchost.exe -k netsvcs"),
            &tokens,
            Some(r"D:\software\other\Clash Verge"),
        ));
        assert!(!trace_matches(
            "WSearch",
            "Windows Search",
            Some(r"C:\Windows\System32\SearchIndexer.exe"),
            &tokens,
            Some(r"D:\software\other\Clash Verge"),
        ));
    }

    #[test]
    fn image_path_normalization_strips_quotes_prefix_and_arguments() {
        assert_eq!(
            normalize_image_path(r#""C:\Program Files\Some App\svc.exe" -k netsvcs"#),
            r"c:\program files\some app\svc.exe"
        );
        assert_eq!(
            normalize_image_path(r"\??\C:\Tools\agent.exe"),
            r"c:\tools\agent.exe"
        );
        // \SystemRoot\ 形式要还原成真实路径，否则永远匹配不上安装目录
        let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
        assert_eq!(
            normalize_image_path(r"\SystemRoot\System32\drivers\foo.sys"),
            format!(r"{}\system32\drivers\foo.sys", system_root.to_lowercase())
        );
    }
}
