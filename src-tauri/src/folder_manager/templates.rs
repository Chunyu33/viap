//! 应用数据模板定义与持久化。
//!
//! 将模板配置从文件夹扫描流程中分离，新增应用时只需要维护本模块，
//! 避免模板变更影响目录大小扫描和迁移逻辑。

use crate::models::AppDataTemplate;
use crate::storage::data_dir::ensure_data_dir;
use crate::utils;

/// 默认内置模板列表。
pub fn default_app_data_templates() -> Vec<AppDataTemplate> {
    vec![
        // 动态目录由 detector 根据应用实际配置识别，兼容不同版本和安装位置。
        template("wechat", "微信", "wechat", &["WeChat.exe"], None),
        template("wxwork", "企业微信", "wxwork", &["WXWork.exe"], None),
        template("qq", "QQ", "qq", &["QQ.exe"], None),
        template("dingtalk", "钉钉", "dingtalk", &["DingTalk.exe"], None),
        template("feishu", "飞书", "feishu", &["Lark.exe", "Feishu.exe"], None),
        // Telegram 桌面版把消息、媒体与账号全都放在这一个目录里，是典型的大目录。
        // 便携版会把 tdata 放在程序同级目录，那种情况这里检测不到。
        template("telegram", "Telegram 数据", "telegram", &["Telegram.exe"], Some(r"%APPDATA%\Telegram Desktop")),
        template("chrome_cache", "Chrome 缓存", "chrome_cache", &["chrome.exe"], None),
        template("edge_cache", "Edge 缓存", "edge_cache", &["msedge.exe"], None),
        template("vscode_extensions", "VS Code 扩展", "vscode_extensions", &["code.exe"], None),
        template("vscode_user_data", "VS Code 用户数据", "vscode_user_data", &["code.exe"], Some(r"%APPDATA%\Code\User")),
        template("cursor_appdata", "Cursor 用户数据", "cursor_appdata", &["Cursor.exe"], Some(r"%APPDATA%\Cursor")),
        template("cursor_extensions", "Cursor 扩展", "cursor_extensions", &["Cursor.exe"], Some(r"%USERPROFILE%\.cursor\extensions")),
        template("npm_global", "npm 全局包", "npm_global", &[], None),
        template("npm_cache", "npm 缓存", "npm_cache", &["node.exe"], Some(r"%LOCALAPPDATA%\npm-cache")),
        template("yarn_cache", "Yarn 缓存", "yarn_cache", &["node.exe", "yarn.exe"], Some(r"%LOCALAPPDATA%\Yarn\Cache")),
        template("gradle_cache", "Gradle 缓存", "gradle_cache", &["java.exe", "gradle.exe", "gradlew.exe"], Some(r"%USERPROFILE%\.gradle")),
        template("maven_repository", "Maven 本地仓库", "maven_repository", &["java.exe", "mvn.exe"], Some(r"%USERPROFILE%\.m2\repository")),
        template("cargo_home", "Cargo 包缓存", "cargo_home", &["cargo.exe", "rustc.exe", "rust-analyzer.exe"], Some(r"%USERPROFILE%\.cargo")),
        template("rustup_home", "Rustup 工具链", "rustup_home", &["rustup.exe", "rustc.exe", "rust-analyzer.exe"], Some(r"%USERPROFILE%\.rustup")),
        template("pip_cache", "pip 缓存", "pip_cache", &["python.exe", "pip.exe"], Some(r"%LOCALAPPDATA%\pip\Cache")),
        template("uv_cache", "uv 缓存", "uv_cache", &["uv.exe", "python.exe"], Some(r"%LOCALAPPDATA%\uv\cache")),
        template("nuget_packages", "NuGet 包缓存", "nuget_packages", &["dotnet.exe", "nuget.exe", "devenv.exe"], Some(r"%USERPROFILE%\.nuget\packages")),
        template("docker_data", "Docker 配置", "docker_data", &["Docker Desktop.exe"], Some(r"%USERPROFILE%\.docker")),
        template("dotnet_data", ".NET 用户数据", "dotnet_data", &["dotnet.exe"], Some(r"%USERPROFILE%\.dotnet")),
        // AI 工具目录通常包含模型、索引和运行配置，按应用单独迁移，避免扫描整个用户目录。
        template("claude_code", "Claude Code 数据", "claude_code", &["node.exe", "claude.exe"], Some(r"%USERPROFILE%\.claude")),
        template("codex_data", "Codex 数据", "codex_data", &["node.exe", "codex.exe"], Some(r"%USERPROFILE%\.codex")),
        template("devin_data", "Devin 数据", "devin_data", &[], Some(r"%USERPROFILE%\.devin")),
        template("ollama_data", "Ollama 数据", "ollama_data", &["ollama.exe"], Some(r"%USERPROFILE%\.ollama")),
        template("comfyui_data", "ComfyUI 数据", "comfyui_data", &["python.exe"], Some(r"%USERPROFILE%\.comfyui")),
        template("gemini_data", "Gemini CLI 数据", "gemini_data", &["node.exe"], Some(r"%USERPROFILE%\.gemini")),
        // 主流 AI 客户端：会话、索引与模型都落在固定目录里，逐个列出方便单独迁移。
        // 这些路径随版本变动频繁，写错不会显示成可用项 —— 目录不存在的条目会被自动隐藏，
        // 所以这里宁可多列几条已知位置，也不要让装了应用的用户看到「未找到」。
        template("workbuddy_data", "WorkBuddy 数据", "ai_data", &["WorkBuddy.exe"], Some(r"%USERPROFILE%\.workbuddy")),
        // dsh 有两处：用户目录下的 .dsh 是真正的工作目录（会话、附件、配置），
        // AppData 里那个只是桌面端外壳自己的缓存，分开列便于判断哪边占地方。
        template("dsh_data", "DeepSeek Harness 数据", "ai_data", &["dsh.exe", "dsh-desktop.exe"], Some(r"%USERPROFILE%\.dsh")),
        template("dsh_desktop_app", "DeepSeek Harness 应用数据", "ai_data", &["dsh-desktop.exe"], Some(r"%APPDATA%\@deepseek-ai\dsh-desktop")),
        template("kimi_code", "Kimi Code 数据", "ai_data", &["kimi-desktop.exe", "kimi-code.exe"], Some(r"%USERPROFILE%\.kimi-code")),
        template("kimi_code_app", "Kimi Code 应用数据", "ai_data", &["kimi-desktop.exe", "kimi-code.exe"], Some(r"%APPDATA%\kimi-code-app")),
        template("kimi_desktop", "Kimi 桌面版数据", "ai_data", &["kimi-desktop.exe"], Some(r"%APPDATA%\kimi-desktop")),
        template("kimi_work", "Kimi Work 数据", "ai_data", &["kimi-desktop.exe", "kimi-code.exe"], Some(r"%USERPROFILE%\.kimi-work")),
        template("opencode_data", "opencode 数据", "ai_data", &["opencode.exe"], Some(r"%APPDATA%\ai.opencode.desktop")),
        template("openai_desktop", "ChatGPT / Codex 桌面版", "ai_data", &["ChatGPT.exe", "codex.exe"], Some(r"%LOCALAPPDATA%\OpenAI")),
        template("yuanbao_data", "腾讯元宝数据", "ai_data", &["Yuanbao.exe"], Some(r"%LOCALAPPDATA%\com.tencent.yuanbao")),
        template("zcode_data", "ZCode 数据", "ai_data", &["ZCode.exe"], Some(r"%APPDATA%\ZCode")),
        template("cherry_studio", "Cherry Studio 数据", "ai_data", &["Cherry Studio.exe"], Some(r"%APPDATA%\CherryStudio")),
        template("chatbox_data", "Chatbox 数据", "ai_data", &["Chatbox.exe"], Some(r"%APPDATA%\Chatbox")),
        template("jan_data", "Jan 数据", "ai_data", &["Jan.exe"], Some(r"%USERPROFILE%\jan")),
        template("anythingllm_data", "AnythingLLM 数据", "ai_data", &["AnythingLLM.exe"], Some(r"%APPDATA%\anythingllm-desktop\storage")),
        template("dify_data", "Dify 数据", "ai_data", &[], Some(r"%USERPROFILE%\.dify")),
        // 用户目录下的「.<厂商>」目录：AI 编程类工具普遍把索引、会话与引擎放这里。
        // 这几个在本机实测确实存在且有体积 —— .codeium 约 900 MB、.agent-browser 约 428 MB。
        // 这三个用的都是新 ID：codebuddy_data / codeium_data / continue_data 在 deprecated
        // 列表里（旧版本取消过），沿用旧 ID 会「先删后加」，每次加载都要重写配置文件。
        template("codebuddy_local", "CodeBuddy 数据", "ai_data", &["CodeBuddy.exe"], Some(r"%USERPROFILE%\.codebuddy")),
        template("codeium_local", "Codeium / Windsurf 数据", "ai_data", &["Windsurf.exe"], Some(r"%USERPROFILE%\.codeium")),
        template("continue_local", "Continue 数据", "ai_data", &[], Some(r"%USERPROFILE%\.continue")),
        template("kimi_webbridge", "Kimi 浏览器桥接数据", "ai_data", &["kimi-desktop.exe"], Some(r"%USERPROFILE%\.kimi-webbridge")),
        template("devin_shared", "Devin 共享数据", "ai_data", &[], Some(r"%USERPROFILE%\.devin-shared")),
        template("agent_browser_cache", "AI 浏览器内核缓存", "ai_data", &[], Some(r"%USERPROFILE%\.agent-browser")),
        // 模型缓存单独用「模型」图标：它们通常最大，也最值得优先搬走
        template("lmstudio_models", "LM Studio 模型", "ai_models", &["LM Studio.exe"], Some(r"%USERPROFILE%\.lmstudio")),
        template("lmstudio_cache", "LM Studio 缓存", "ai_models", &["LM Studio.exe"], Some(r"%USERPROFILE%\.cache\lm-studio")),
        template("hf_cache", "Hugging Face 模型缓存", "ai_models", &[], Some(r"%USERPROFILE%\.cache\huggingface\hub")),
        template("modelscope_cache", "ModelScope 模型缓存", "ai_models", &[], Some(r"%USERPROFILE%\.cache\modelscope\hub")),
        // Adobe 和剪映拆分 Roaming/LocalAppData，避免一次迁移过大的无关目录。
        template("adobe_appdata", "Adobe 用户数据", "adobe_appdata", &["Photoshop.exe", "Adobe Premiere Pro.exe"], Some(r"%APPDATA%\Adobe")),
        template("adobe_localdata", "Adobe 本地数据", "adobe_localdata", &["Photoshop.exe", "Adobe Premiere Pro.exe"], Some(r"%LOCALAPPDATA%\Adobe")),
        template("jianying_appdata", "剪映用户数据", "jianying_appdata", &["JianyingPro.exe"], Some(r"%APPDATA%\JianyingPro")),
        template("jianying_localdata", "剪映本地数据", "jianying_localdata", &["JianyingPro.exe"], Some(r"%LOCALAPPDATA%\JianyingPro")),
    ]
}

/// 统一构造模板，减少默认模板字段重复并降低新增条目的出错概率。
fn template(
    id: &str,
    display_name: &str,
    icon_id: &str,
    process_names: &[&str],
    path: Option<&str>,
) -> AppDataTemplate {
    AppDataTemplate {
        id: id.to_string(),
        display_name: display_name.to_string(),
        icon_id: icon_id.to_string(),
        process_names: process_names.iter().map(|name| (*name).to_string()).collect(),
        path: path.map(str::to_string),
    }
}

/// 加载模板并自动合并新版本内置项，保留用户已修改的字段。
pub fn load_app_data_templates() -> Vec<AppDataTemplate> {
    let path = utils::app_data_templates_path(&ensure_data_dir());
    if !path.exists() {
        let defaults = default_app_data_templates();
        let json = serde_json::to_string_pretty(&defaults).unwrap_or_default();
        let _ = std::fs::write(&path, &json);
        return defaults;
    }

    let templates = std::fs::read_to_string(&path)
        .ok()
        .and_then(|content| serde_json::from_str::<Vec<AppDataTemplate>>(&content).ok())
        .unwrap_or_else(default_app_data_templates);
    merge_missing_default_templates(templates)
}

/// 旧配置只按稳定 ID 合并缺失模板，避免覆盖用户自定义路径。
fn merge_missing_default_templates(mut templates: Vec<AppDataTemplate>) -> Vec<AppDataTemplate> {
    let mut changed = remove_deprecated_app_data_templates(&mut templates);
    let existing_ids: std::collections::HashSet<String> = templates
        .iter()
        .map(|template| template.id.to_lowercase())
        .collect();

    for default_template in default_app_data_templates() {
        if existing_ids.contains(&default_template.id.to_lowercase()) {
            continue;
        }
        templates.push(default_template);
        changed = true;
    }

    if changed {
        let path = utils::app_data_templates_path(&ensure_data_dir());
        if let Ok(json) = serde_json::to_string_pretty(&templates) {
            let _ = std::fs::write(&path, json);
        }
    }
    templates
}

fn remove_deprecated_app_data_templates(templates: &mut Vec<AppDataTemplate>) -> bool {
    let before_len = templates.len();
    templates.retain(|template| !deprecated_app_data_template_ids()
        .iter()
        .any(|deprecated_id| template.id.eq_ignore_ascii_case(deprecated_id)));
    templates.len() != before_len
}

fn deprecated_app_data_template_ids() -> &'static [&'static str] {
    // 这些条目曾被旧版本写入配置，升级时主动删除，避免继续展示已取消的非核心应用。
    &["pnpm_store", "windsurf_appdata", "codebuddy_data", "codeium_data", "continue_data", "zed_appdata"]
}

/// 保存设置页编辑后的模板。
pub fn save_app_data_templates(templates: Vec<AppDataTemplate>) -> Result<(), String> {
    let path = utils::app_data_templates_path(&ensure_data_dir());
    let json = serde_json::to_string_pretty(&templates)
        .map_err(|error| format!("序列化模板失败: {}", error))?;
    std::fs::write(&path, json)
        .map_err(|error| format!("写入模板文件失败: {}", error))
}
