// MS Store / UWP（Appx 包）支持
//
// 这类应用没有 `...\CurrentVersion\Uninstall` 注册表条目，常规卸载命令解析必然落空，
// 需要走系统组件接口 `Get-AppxPackage | Remove-AppxPackage`。
//
// 设计要点：
// - 只调用一次 PowerShell 枚举全部包，匹配逻辑放在 Rust 里做（可单测、无注入风险）
// - 把 PackageFullName 拼进命令前必须校验字符集，避免命令注入
// - 所有 PowerShell 调用都隐藏窗口，避免闪黑框

use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};

/// 已安装的 Appx 包信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppxPackage {
    /// 包名（如 Microsoft.WindowsCalculator）
    #[serde(rename = "Name", alias = "name")]
    pub name: String,
    /// 完整包名（含版本/架构/发布者哈希），卸载时使用
    #[serde(rename = "PackageFullName", alias = "packageFullName")]
    pub package_full_name: String,
    /// 安装位置（通常在 C:\Program Files\WindowsApps\...）
    #[serde(rename = "InstallLocation", alias = "installLocation", default)]
    pub install_location: String,
}

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 枚举当前用户已安装的全部 Appx 包
///
/// 单次调用返回全部包（通常几百条、几十 KB JSON），匹配交给 Rust 完成：
/// 这样既不需要把用户输入拼进脚本，也能对匹配规则写单测。
pub fn list_installed_packages() -> Result<Vec<AppxPackage>, String> {
    #[cfg(windows)]
    {
        let script = "Get-AppxPackage | Select-Object Name,PackageFullName,InstallLocation | ConvertTo-Json -Compress";
        let stdout = run_powershell(script)?;
        Ok(parse_packages_json(&stdout))
    }
    #[cfg(not(windows))]
    {
        Ok(Vec::new())
    }
}

/// 解析 `ConvertTo-Json` 输出：可能是数组，也可能只有一条记录（对象）
pub(crate) fn parse_packages_json(json: &str) -> Vec<AppxPackage> {
    let trimmed = json.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    if let Ok(list) = serde_json::from_str::<Vec<AppxPackage>>(trimmed) {
        return list;
    }
    // 只有一条记录时 ConvertTo-Json 会输出对象而不是数组
    serde_json::from_str::<AppxPackage>(trimmed)
        .map(|single| vec![single])
        .unwrap_or_default()
}

/// 从包列表中挑出与目标应用匹配的包
///
/// 匹配规则（按可信度从高到低）：
/// 1. 安装目录位于 `\WindowsApps\<PackageFullName>\...` → 直接取出包全名
/// 2. 包名与应用名（去掉空格、连字符后）互相包含
pub(crate) fn match_packages<'a>(
    packages: &'a [AppxPackage],
    app_name: &str,
    install_location: Option<&Path>,
) -> Vec<&'a AppxPackage> {
    if let Some(full_name) = install_location.and_then(package_full_name_from_location) {
        let matched: Vec<&AppxPackage> = packages
            .iter()
            .filter(|package| package.package_full_name.eq_ignore_ascii_case(&full_name))
            .collect();
        if !matched.is_empty() {
            return matched;
        }
    }

    let normalized_app = normalize_for_match(app_name);
    if normalized_app.len() < 3 {
        return Vec::new();
    }
    packages
        .iter()
        .filter(|package| {
            let normalized_package = normalize_for_match(&package.name);
            !normalized_package.is_empty()
                && (normalized_package.contains(&normalized_app)
                    || normalized_app.contains(&normalized_package))
        })
        .collect()
}

/// 从 `...\WindowsApps\<PackageFullName>\...` 形式的路径中取出包全名
fn package_full_name_from_location(location: &Path) -> Option<String> {
    let text = location.to_string_lossy();
    let mut components = text.split(['\\', '/']).filter(|part| !part.is_empty());
    while let Some(component) = components.next() {
        if component.eq_ignore_ascii_case("WindowsApps") {
            return components.next().map(|value| value.to_string());
        }
    }
    None
}

/// 归一化用于模糊匹配：转小写并去掉空格、连字符、下划线
fn normalize_for_match(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect::<String>()
        .to_lowercase()
}

/// 校验 PackageFullName 是否只包含安全字符
///
/// 包全名形如 `Microsoft.WindowsCalculator_11.2210.0.0_x64__8wekyb3d8bbwe`，
/// 合法字符集很小；其余一律拒绝，避免把用户可控字符串拼进 PowerShell 命令。
pub(crate) fn is_valid_package_full_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 200
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
}

/// 生成卸载命令；包名不合法时返回 None（调用方转为提示用户手动处理）
pub fn build_remove_command(package_full_name: &str) -> Option<String> {
    if !is_valid_package_full_name(package_full_name) {
        return None;
    }
    Some(format!(
        "powershell -NoProfile -NonInteractive -Command \"Get-AppxPackage -Package '{}' | Remove-AppxPackage\"",
        package_full_name
    ))
}

/// 为指定应用查找可卸载的 Appx 包（最多返回一个，多个匹配时视为不确定）
pub fn resolve_package(app_name: &str, install_location: Option<&Path>) -> Option<AppxPackage> {
    let packages = list_installed_packages().ok()?;
    let matched = match_packages(&packages, app_name, install_location);
    match matched.len() {
        1 => Some(matched[0].clone()),
        _ => None,
    }
}

/// 查询某个包是否仍然安装（用于卸载后的结果确认）
pub fn is_package_installed(package_full_name: &str) -> bool {
    if !is_valid_package_full_name(package_full_name) {
        return false;
    }
    #[cfg(windows)]
    {
        let script = format!(
            "@(Get-AppxPackage -Package '{}').Count",
            package_full_name
        );
        return run_powershell(&script)
            .map(|stdout| stdout.trim().parse::<u32>().unwrap_or(0) > 0)
            .unwrap_or(false);
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// 执行 PowerShell 脚本并返回标准输出
#[cfg(windows)]
fn run_powershell(script: &str) -> Result<String, String> {
    use std::os::windows::process::CommandExt;

    let output = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script,
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|error| format!("调用 PowerShell 失败: {}", error))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            "PowerShell 执行失败".to_string()
        } else {
            stderr
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    fn sample_packages() -> Vec<AppxPackage> {
        vec![
            AppxPackage {
                name: "Microsoft.WindowsCalculator".to_string(),
                package_full_name: "Microsoft.WindowsCalculator_11.2210.0.0_x64__8wekyb3d8bbwe".to_string(),
                install_location: r"C:\Program Files\WindowsApps\Microsoft.WindowsCalculator_11.2210.0.0_x64__8wekyb3d8bbwe".to_string(),
            },
            AppxPackage {
                name: "SomeVendor.Tool".to_string(),
                package_full_name: "SomeVendor.Tool_1.0.0.0_x64__abcdefghij".to_string(),
                install_location: r"C:\Program Files\WindowsApps\SomeVendor.Tool_1.0.0.0_x64__abcdefghij".to_string(),
            },
        ]
    }

    #[test]
    fn parses_both_array_and_single_object_json() {
        let array = r#"[{"Name":"A.B","PackageFullName":"A.B_1.0.0.0_x64__z","InstallLocation":"C:\\x"}]"#;
        assert_eq!(parse_packages_json(array).len(), 1);

        // 单条记录时 ConvertTo-Json 输出对象
        let single = r#"{"Name":"A.B","PackageFullName":"A.B_1.0.0.0_x64__z","InstallLocation":"C:\\x"}"#;
        let parsed = parse_packages_json(single);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].package_full_name, "A.B_1.0.0.0_x64__z");

        assert!(parse_packages_json("").is_empty());
        assert!(parse_packages_json("not json").is_empty());
    }

    #[test]
    fn matches_by_windows_apps_install_location_first() {
        let packages = sample_packages();
        let location = Path::new(r"C:\Program Files\WindowsApps\SomeVendor.Tool_1.0.0.0_x64__abcdefghij\app");
        let matched = match_packages(&packages, "完全无关的名字", Some(location));
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].name, "SomeVendor.Tool");
    }

    #[test]
    fn matches_by_normalized_name() {
        let packages = sample_packages();
        // 大小写、空格与连字符都不影响匹配
        let matched = match_packages(&packages, "windows calculator", None);
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].name, "Microsoft.WindowsCalculator");

        // 名字太短时不做匹配，避免命中一堆包
        assert!(match_packages(&packages, "a", None).is_empty());
        assert!(match_packages(&packages, "unrelated-app", None).is_empty());
    }

    #[test]
    fn rejects_unsafe_package_names_before_building_commands() {
        // 正常包名可用
        let command = build_remove_command("SomeVendor.Tool_1.0.0.0_x64__abcdefghij")
            .expect("合法包名应当生成命令");
        assert!(command.contains("Remove-AppxPackage"));
        assert!(command.contains("SomeVendor.Tool_1.0.0.0_x64__abcdefghij"));

        // 含引号、分号、空格等字符一律拒绝（防止命令注入）
        for unsafe_name in [
            "Tool'; Remove-Item C:\\ -Recurse; '",
            "Tool\" | rm",
            "Tool Name",
            "",
        ] {
            assert!(
                build_remove_command(unsafe_name).is_none(),
                "不合法包名不应生成命令: {}",
                unsafe_name
            );
        }
    }
}
