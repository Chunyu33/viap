// 危险路径分级检测子模块
// 对源路径做黑名单匹配，拦截不可迁移的系统目录

/// 危险路径分级
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum DangerLevel {
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
/// 4. **结构性危险路径**：盘符根、用户配置根、Program Files 根等（见 check_structural_block）
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
pub(crate) fn check_dangerous_path(source: &str) -> Option<(DangerLevel, String)> {
    let source_lower = source.to_lowercase();
    let source_normalized = source_lower.replace('/', "\\");

    // ── 结构性拦截优先于关键词规则 ──────────────────────────────────────
    // C:\Users\chunyu、D:\ 这类路径不含任何关键词，只能按路径形态判断；
    // 必须排在关键词表之前，否则会被 ProgramData 之类的 WARNING 规则降级
    if let Some(message) = check_structural_block(&source_normalized) {
        return Some((DangerLevel::Blocked, message));
    }

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
                    return Some((
                        DangerLevel::Blocked,
                        blocked_message(rule.category, rule.label),
                    ));
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

/// 组装 BLOCKED 提示文案（关键词规则与结构性规则共用，避免两处文案漂移）
fn blocked_message(category: &str, label: &str) -> String {
    let tip = match category {
        "系统目录" => "迁移系统核心目录会导致 Windows 组件崩溃，无法开机。",
        "浏览器"   => "浏览器安装目录含有系统级注册和自动修复机制，迁移后 Junction 会被自动覆盖，且所有扩展插件将损坏。\n如需释放空间，请迁移浏览器的缓存目录（在「数据迁移」页面的快捷项中）。",
        "GPU驱动"  => "GPU 驱动路径写死进系统服务注册表，迁移后驱动无法加载，轻则降级到基本显示模式，重则蓝屏。",
        "办公软件" => "Microsoft Office 使用 ClickToRun 虚拟化安装机制，迁移后自动修复服务会覆盖 Junction，COM 注册表记录无法跟随迁移。",
        "运行时"   => ".NET 运行时路径被大量应用和系统组件硬编码引用，迁移后依赖 .NET 的应用将无法启动。",
        "开发工具" => "开发工具目录含被 Windows 内核内存映射的 DLL 和后台语言服务，复制阶段容易失败，迁移前需完全退出所有相关进程。",
        _          => "该目录包含系统级组件，不支持迁移。",
    };
    format!(
        "🚫 无法迁移：{label} 属于「{category}」，不支持迁移。\n\n{tip}",
        label = label,
        category = category,
        tip = tip,
    )
}

/// 结构性危险路径检测（关键词规则覆盖不到、但绝不允许迁移的路径形态）
///
/// 单独成函数的原因：`C:\Users\chunyu`、`D:\`、`C:\Program Files` 这类路径不含任何
/// 危险关键词，靠 contains 匹配识别不出来，但它们一旦被迁移就是不可逆的系统损坏。
/// 返回 Some(提示文案) 表示必须 BLOCKED。
fn check_structural_block(source_normalized: &str) -> Option<String> {
    let trimmed = source_normalized.trim_end_matches('\\');
    let segments: Vec<&str> = trimmed.split('\\').filter(|part| !part.is_empty()).collect();
    let is_drive_letter = |segment: &str| {
        let bytes = segment.as_bytes();
        bytes.len() == 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic()
    };

    if segments.is_empty() {
        return None;
    }

    // 1) 盘符根：迁移整个卷既不可能成功也没意义
    if segments.len() == 1 && is_drive_letter(segments[0]) {
        return Some(blocked_message("系统目录", "磁盘根目录"));
    }

    // 2) 用户配置根：C:\Users\<用户名>，或与 USERPROFILE / APPDATA / LOCALAPPDATA 完全一致
    //（配置目录被策略搬到别的盘时，后一条判断才覆盖得到）
    let is_profile_root = segments.len() == 3
        && is_drive_letter(segments[0])
        && segments[1] == "users"
        || ["userprofile", "appdata", "localappdata"].iter().any(|var| {
            std::env::var(var)
                .ok()
                .map(|value| value.to_lowercase().replace('/', "\\"))
                .map(|value| value.trim_end_matches('\\').to_string() == trimmed)
                .unwrap_or(false)
        });
    if is_profile_root {
        return Some(blocked_message("系统目录", "用户配置根目录"));
    }

    // 3) Program Files 系列根目录（子目录仍然允许迁移）
    if segments.len() >= 2 {
        let last = segments[segments.len() - 1];
        if matches!(last, "program files" | "program files (x86)" | "program files (arm)") {
            return Some(blocked_message("系统目录", "Program Files 根目录"));
        }
    }

    // 4) 盘符根下由系统独占的目录（任意盘符，带 $ 的名字也要覆盖）
    if segments.len() == 2 {
        let root_child = segments[1];
        const SYSTEM_ROOT_CHILDREN: &[&str] = &[
            "$recycle.bin",
            "system volume information",
            "recovery",
            "boot",
            "config.msi",
            "perflogs",
            "msocache",
            "$windows.~bt",
            "$windows.~ws",
        ];
        if SYSTEM_ROOT_CHILDREN.contains(&root_child) {
            return Some(blocked_message("系统目录", "系统保留目录"));
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_blocked(path: &str) -> bool {
        matches!(check_dangerous_path(path), Some((DangerLevel::Blocked, _)))
    }

    #[test]
    fn blocks_drive_roots_and_system_root_children() {
        assert!(is_blocked(r"C:\"));
        assert!(is_blocked(r"c:"));
        assert!(is_blocked(r"D:\"));
        assert!(is_blocked(r"C:\$Recycle.Bin"));
        assert!(is_blocked(r"D:\System Volume Information"));
        assert!(is_blocked(r"C:\Recovery"));
        assert!(is_blocked(r"C:\Boot"));
    }

    #[test]
    fn blocks_user_profile_root_but_not_its_subfolders() {
        // 用户配置根目录被迁移会导致登录服务失效，必须拦下
        assert!(is_blocked(r"C:\Users\chunyu"));
        assert!(is_blocked(r"D:\Users\someone"));
        // 但用户目录下的普通文件夹仍然允许迁移
        assert!(!is_blocked(r"C:\Users\chunyu\Documents"));
        assert!(!is_blocked(r"C:\Users\chunyu\AppData\Local\Wand"));
    }

    #[test]
    fn blocks_program_files_root_but_not_installed_apps() {
        assert!(is_blocked(r"C:\Program Files"));
        assert!(is_blocked(r"C:\Program Files (x86)"));
        assert!(!is_blocked(r"C:\Program Files\SomeApp"));
        assert!(!is_blocked(r"D:\Program Files\SomeApp"));
    }

    #[test]
    fn blocked_rules_still_win_over_warning_rules() {
        // c:\programdata\microsoft\windows 必须仍是 BLOCKED，不能被 programdata 的 WARNING 降级
        assert!(is_blocked(r"C:\ProgramData\Microsoft\Windows"));
        assert!(matches!(
            check_dangerous_path(r"C:\ProgramData"),
            Some((DangerLevel::Warning, _))
        ));
    }

    #[test]
    fn normal_data_folders_are_not_flagged() {
        assert!(check_dangerous_path(r"D:\software\other\Clash Verge").is_none());
        assert!(check_dangerous_path(r"C:\Users\chunyu\AppData\Local\Programs\antigravity").is_none());
        assert!(check_dangerous_path(r"D:\C_Map\link\Wand").is_none());
    }
}
