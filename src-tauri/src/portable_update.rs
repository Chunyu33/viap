//! 便携版更新检查。
//!
//! 便携版不注册 updater 插件——安装器会把程序装成安装版，与便携版的发行形态冲突，
//! 因此便携版只能由用户手动下载新包替换。本模块只负责回答一个问题：
//! **线上是否真的存在比当前更高的版本**，由前端据此决定是否提示。
//!
//! 所有失败路径统一返回 `None`：宁可漏提示，也不能因为网络抖动让
//! 拿着最新包的用户每次启动都被告知"发现新版本"。

use std::time::Duration;

use serde::Serialize;

/// GitHub 最新 Release 接口。
///
/// 只读取 `tag_name`，不解析资源列表。该接口匿名访问有每小时 60 次的限流，
/// 超限会返回 403，届时按"无法确认"静默处理。
/// 注意必须与 integrity 模块指向同一个仓库，否则版本比对会失去意义。
const LATEST_RELEASE_API_URL: &str =
    "https://api.github.com/repos/Chunyu33/viap/releases/latest";

/// 便携版更新检查结果，字段名以 snake_case 暴露给前端
#[derive(Debug, Serialize)]
pub struct PortableUpdateCheck {
    /// 线上是否存在比当前更高的版本
    pub has_update: bool,
    /// 线上最新版本号（已去掉 v 前缀）
    pub latest_version: String,
    /// 当前运行版本号
    pub current_version: String,
}

/// 把 "v1.2.1" / "1.2.1" 解析成可逐段比较的数字序列。
///
/// 解析失败返回 `None`，调用方据此保守判定为"没有新版本"。
fn parse_version_segments(version: &str) -> Option<Vec<u64>> {
    let trimmed = version.trim().trim_start_matches(['v', 'V']);
    if trimmed.is_empty() {
        return None;
    }
    // 忽略 -beta / +build 之类的后缀，只比较主版本段
    let core = trimmed.split(['-', '+']).next().unwrap_or(trimmed);
    core.split('.')
        .map(|segment| segment.trim().parse::<u64>().ok())
        .collect()
}

/// 判断线上版本是否高于当前版本。
///
/// 必须按数字段逐个比较：字符串比较会把 1.10.0 判成小于 1.9.0，
/// 导致真实的新版本被静默漏报。
fn is_newer_version(latest: &str, current: &str) -> bool {
    match (
        parse_version_segments(latest),
        parse_version_segments(current),
    ) {
        // Vec<u64> 按字典序比较，段数不同时短的更小，符合语义化版本直觉
        (Some(latest_segments), Some(current_segments)) => latest_segments > current_segments,
        _ => false,
    }
}

/// 查询 GitHub 最新发布版本，供便携版判断是否需要提示手动更新。
///
/// 返回 `None` 表示无法确认（网络失败、限流、响应体异常），前端应保持静默。
#[tauri::command]
pub async fn check_portable_update(app_handle: tauri::AppHandle) -> Option<PortableUpdateCheck> {
    // 与 integrity 模块一致，取 Tauri 包信息而非 Cargo 版本，确保比较基准是实际发行版本
    let current_version = app_handle.package_info().version.to_string();

    let client = match reqwest::Client::builder()
        .user_agent(format!("Viap/{current_version}"))
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            log_warn!("updater", "便携版更新检查客户端创建失败: {}", error);
            return None;
        }
    };

    let response = match client.get(LATEST_RELEASE_API_URL).send().await {
        Ok(response) => response,
        Err(error) => {
            log_warn!("updater", "便携版更新检查请求失败: {}", error);
            return None;
        }
    };

    if !response.status().is_success() {
        // 403 多为匿名限流，属于预期内情况，不需要向用户暴露
        log_warn!(
            "updater",
            "便携版更新检查返回 HTTP {}，按无法确认处理",
            response.status()
        );
        return None;
    }

    // 刻意使用 text + serde_json 解析，而不是 response.json()，
    // 避免为单个调用开启 reqwest 的 json feature
    let body = match response.text().await {
        Ok(body) => body,
        Err(error) => {
            log_warn!("updater", "便携版更新检查读取响应失败: {}", error);
            return None;
        }
    };

    let payload: serde_json::Value = match serde_json::from_str(&body) {
        Ok(payload) => payload,
        Err(error) => {
            log_warn!("updater", "便携版更新检查响应解析失败: {}", error);
            return None;
        }
    };

    let tag = payload.get("tag_name")?.as_str()?;
    let latest_version = tag.trim_start_matches(['v', 'V']).to_string();

    Some(PortableUpdateCheck {
        has_update: is_newer_version(&latest_version, &current_version),
        latest_version,
        current_version,
    })
}

#[cfg(test)]
mod tests {
    use super::{is_newer_version, parse_version_segments};

    #[test]
    fn compares_versions_numerically_not_lexically() {
        // 1.10.0 必须大于 1.9.0，字符串比较会得出相反结论
        assert!(is_newer_version("1.10.0", "1.9.0"));
        assert!(!is_newer_version("1.9.0", "1.10.0"));
    }

    #[test]
    fn strips_v_prefix_and_build_suffix() {
        assert!(is_newer_version("v1.2.2", "1.2.1"));
        assert!(is_newer_version("1.2.2-beta.1", "1.2.1"));
        assert_eq!(parse_version_segments("V1.2.1"), Some(vec![1, 2, 1]));
    }

    #[test]
    fn treats_equal_or_unparsable_versions_as_no_update() {
        // 版本相同时不得提示更新，这是本次修复的核心诉求
        assert!(!is_newer_version("1.2.1", "1.2.1"));
        assert!(!is_newer_version("", "1.2.1"));
        assert!(!is_newer_version("latest", "1.2.1"));
    }
}
