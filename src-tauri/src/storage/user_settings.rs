// 用户界面设置持久化模块
//
// 设置文件放在 Viap 数据目录内，便携版复制整个目录后可以继续保留主题、字号和迁移偏好。

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use super::data_dir::ensure_data_dir;

const SETTINGS_FILE_NAME: &str = "ui_settings.json";

lazy_static::lazy_static! {
    // 多次快速切换设置时串行写入，避免多个调用同时操作同一个临时文件。
    static ref SETTINGS_WRITE_LOCK: Mutex<()> = Mutex::new(());
}

fn default_theme() -> String {
    "system".to_string()
}

fn default_font_size() -> u8 {
    13
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserSettings {
    #[serde(default)]
    pub default_app_target_path: String,
    #[serde(default)]
    pub default_data_target_path: String,
    #[serde(default = "default_true")]
    pub use_recycle_bin: bool,
    #[serde(default)]
    pub show_scan_debug: bool,
    #[serde(default = "default_font_size")]
    pub font_size_px: u8,
    #[serde(default = "default_theme")]
    pub theme: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserSettingsLoadResult {
    pub settings: UserSettings,
    pub initialized: bool,
}

fn default_true() -> bool {
    true
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            default_app_target_path: String::new(),
            default_data_target_path: String::new(),
            use_recycle_bin: true,
            show_scan_debug: false,
            font_size_px: default_font_size(),
            theme: default_theme(),
        }
    }
}

impl UserSettings {
    fn normalized(mut self) -> Self {
        // 后端再次校验边界，避免旧配置或外部修改把前端控件带到异常状态。
        self.font_size_px = self.font_size_px.clamp(12, 16);
        if !matches!(self.theme.as_str(), "light" | "dark" | "system") {
            self.theme = default_theme();
        }
        self
    }
}

fn settings_path() -> PathBuf {
    ensure_data_dir().join(SETTINGS_FILE_NAME)
}

/// 读取设置文件；文件不存在时返回默认值并交给前端导入旧 localStorage。
#[tauri::command]
pub fn get_user_settings() -> Result<UserSettingsLoadResult, String> {
    let path = settings_path();
    if !path.exists() {
        return Ok(UserSettingsLoadResult {
            settings: UserSettings::default(),
            initialized: false,
        });
    }

    let json =
        std::fs::read_to_string(&path).map_err(|error| format!("读取用户设置失败: {}", error))?;
    let settings = serde_json::from_str::<UserSettings>(&json)
        .map_err(|error| format!("解析用户设置失败: {}", error))?
        .normalized();
    Ok(UserSettingsLoadResult {
        settings,
        initialized: true,
    })
}

/// 保存完整用户设置，采用临时文件替换避免断电留下半个 JSON。
#[tauri::command]
pub fn save_user_settings(settings: UserSettings) -> Result<(), String> {
    let _guard = SETTINGS_WRITE_LOCK
        .lock()
        .map_err(|_| "用户设置写入锁已损坏，请重启应用后重试".to_string())?;
    let path = settings_path();
    let json = serde_json::to_string_pretty(&settings.normalized())
        .map_err(|error| format!("序列化用户设置失败: {}", error))?;
    let temp_path = path.with_extension("json.tmp");
    std::fs::write(&temp_path, json).map_err(|error| format!("写入用户设置失败: {}", error))?;
    std::fs::rename(&temp_path, &path).map_err(|error| format!("更新用户设置失败: {}", error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalized_settings_reject_invalid_theme_and_font_size() {
        let settings = UserSettings {
            theme: "unknown".to_string(),
            font_size_px: 99,
            ..UserSettings::default()
        }
        .normalized();

        assert_eq!(settings.theme, "system");
        assert_eq!(settings.font_size_px, 16);
    }

    #[test]
    fn settings_use_camel_case_json_for_frontend_ipc() {
        let json = serde_json::to_string(&UserSettings::default()).unwrap();
        assert!(json.contains("defaultAppTargetPath"));
        assert!(json.contains("fontSizePx"));
        assert!(!json.contains("default_app_target_path"));
    }
}
