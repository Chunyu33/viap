// 危险路径检测 Hook
// 与后端 migration.rs 的 check_dangerous_path 保持规则同步，
// 在迁移前两级拦截：BLOCKED 直接终止，WARNING 弹窗确认后放行
// 前端提前拦截，后端兜底防线

import { useCallback } from 'react';

type DangerCategory = '系统目录' | '浏览器' | 'GPU驱动' | '虚拟化' | '数据库' | '缓存服务' | '安全软件' | '系统组件' | '开发工具';

interface DangerRule {
  pattern: string;
  level: 'BLOCKED' | 'WARNING';
  category: DangerCategory;
  label: string;
  reason: string;
}

export interface WarningInfo {
  label: string;
  category: string;
  reason: string;
  disclaimer: string;
}

// ── 规则表（与后端 migration.rs 完全同步）────────────────────────────────────
// 顺序至关重要：BLOCKED 全部在前，WARNING 全部在后；
// c:\programdata\microsoft\windows (BLOCKED) 必须在 c:\programdata (WARNING) 之前，
// 确保更具体的子路径先被 BLOCKED 命中，不会降级到 WARNING
const DANGER_RULES: DangerRule[] = [
  // ═══════════════════════════════════════════
  // BLOCKED — 系统核心目录
  // 特征：路径硬编码进 Windows 内核或系统服务，迁移后系统组件崩溃，无法开机
  // ═══════════════════════════════════════════
  {
    pattern: 'c:\\windows',
    level: 'BLOCKED', category: '系统目录', label: 'Windows 系统目录',
    reason: 'Windows 系统核心目录，迁移会导致系统崩溃，无法开机。',
  },
  {
    pattern: 'c:\\program files\\windowsapps',
    level: 'BLOCKED', category: '系统目录', label: 'Windows 应用商店目录',
    reason: 'Windows 应用商店目录由系统服务管理，迁移后商店应用全部失效。',
  },
  {
    pattern: 'c:\\programdata\\microsoft\\windows',
    level: 'BLOCKED', category: '系统目录', label: 'Windows 系统数据目录',
    reason: 'Windows 系统数据目录含系统激活、更新等关键信息，迁移后 Windows 更新和安全中心将失效。',
  },

  // ═══════════════════════════════════════════
  // BLOCKED — 系统级浏览器安装目录
  // 特征：含自动修复服务，会把 Junction 识别为损坏安装并覆盖；
  //       Chromium 把安装路径写死进扩展签名，迁移后所有插件损坏；
  //       WebView2 是 Viap 自身运行依赖，迁移后 Viap 无法启动
  // ═══════════════════════════════════════════
  {
    pattern: 'microsoft\\edge\\application',
    level: 'BLOCKED', category: '浏览器', label: 'Microsoft Edge 安装目录',
    reason: 'Edge 的自动修复服务会把 Junction 识别为损坏安装并覆盖，迁移无效。',
  },
  {
    pattern: 'microsoft\\msedge\\application',
    level: 'BLOCKED', category: '浏览器', label: 'Microsoft Edge 安装目录',
    reason: 'Edge 的自动修复服务会把 Junction 识别为损坏安装并覆盖，迁移无效。',
  },
  {
    pattern: 'microsoft\\edgewebview\\application',
    level: 'BLOCKED', category: '浏览器', label: 'Microsoft WebView2 运行时目录（Viap 自身依赖此组件）',
    reason: 'WebView2 是 Viap 的运行依赖，迁移后 Viap 自身将无法启动。',
  },
  {
    pattern: 'google\\chrome\\application',
    level: 'BLOCKED', category: '浏览器', label: 'Google Chrome 安装目录',
    reason: 'Chrome 把安装路径写死进扩展签名，迁移后所有扩展插件报损坏。',
  },
  {
    pattern: 'google\\chrome beta\\application',
    level: 'BLOCKED', category: '浏览器', label: 'Google Chrome Beta 安装目录',
    reason: 'Chrome Beta 把安装路径写死进扩展签名，迁移后所有扩展插件报损坏。',
  },
  {
    pattern: 'google\\chrome dev\\application',
    level: 'BLOCKED', category: '浏览器', label: 'Google Chrome Dev 安装目录',
    reason: 'Chrome Dev 把安装路径写死进扩展签名，迁移后所有扩展插件报损坏。',
  },
  {
    pattern: 'bromite\\application',
    level: 'BLOCKED', category: '浏览器', label: 'Bromite 安装目录',
    reason: 'Bromite 把安装路径写死进扩展签名，迁移后所有扩展插件报损坏。',
  },

  // ═══════════════════════════════════════════
  // BLOCKED — GPU / 显卡驱动
  // 特征：驱动 DLL 路径硬编码进系统服务注册表（HKLM\SYSTEM\CurrentControlSet\Services）；
  //       迁移后驱动服务找不到 DLL，轻则降级到基本显示适配器，重则蓝屏
  // ═══════════════════════════════════════════
  {
    pattern: 'nvidia corporation\\installer2',
    level: 'BLOCKED', category: 'GPU驱动', label: 'NVIDIA 驱动安装目录',
    reason: 'NVIDIA 驱动路径写死进系统服务注册表，迁移后显卡驱动失效。',
  },
  {
    pattern: 'nvidia\\displaydriver',
    level: 'BLOCKED', category: 'GPU驱动', label: 'NVIDIA 显卡驱动目录',
    reason: 'NVIDIA 驱动 DLL 路径写死进系统服务注册表，迁移后显卡驱动失效。',
  },
  {
    pattern: '\\nvidia\\',
    level: 'BLOCKED', category: 'GPU驱动', label: 'NVIDIA 驱动目录',
    reason: 'NVIDIA 驱动路径写死进系统服务注册表，迁移后驱动服务找不到 DLL。',
  },
  {
    pattern: 'amd\\ccc2',
    level: 'BLOCKED', category: 'GPU驱动', label: 'AMD 显卡控制中心目录',
    reason: 'AMD 驱动路径写死进系统服务注册表，迁移后显卡控制中心失效。',
  },
  {
    pattern: 'advanced micro devices',
    level: 'BLOCKED', category: 'GPU驱动', label: 'AMD 驱动目录',
    reason: 'AMD 驱动路径写死进系统服务注册表，迁移后驱动无法加载。',
  },
  {
    pattern: 'intel\\graphics',
    level: 'BLOCKED', category: 'GPU驱动', label: 'Intel 核显驱动目录',
    reason: 'Intel 核显驱动路径写死进系统服务注册表，迁移后驱动失效。',
  },
  {
    pattern: 'intel\\intelgraphicscontrolpanel',
    level: 'BLOCKED', category: 'GPU驱动', label: 'Intel 显卡控制面板目录',
    reason: 'Intel 显卡控制面板路径写死进系统服务注册表，迁移后控制面板失效。',
  },

  // ═══════════════════════════════════════════
  // WARNING — 虚拟化软件
  // 特征：虚拟机磁盘/配置含绝对路径引用，迁移后虚拟机需手动重新关联，但数据不丢失
  // ═══════════════════════════════════════════
  {
    pattern: 'vmware',
    level: 'WARNING', category: '虚拟化', label: 'VMware 目录',
    reason: 'VMware 虚拟机磁盘（.vmdk）和配置文件（.vmx）含绝对路径引用，迁移后虚拟机可能无法直接启动，需要在 VMware 中手动重新关联虚拟机文件。',
  },
  {
    pattern: 'virtualbox',
    level: 'WARNING', category: '虚拟化', label: 'VirtualBox 目录',
    reason: 'VirtualBox 虚拟磁盘（.vdi/.vmdk）含硬编码路径，迁移后需在 VirtualBox 管理器中手动重新注册虚拟机。',
  },
  {
    pattern: 'hyper-v',
    level: 'WARNING', category: '虚拟化', label: 'Hyper-V 目录',
    reason: 'Hyper-V 虚拟机由 Windows 系统服务管理，迁移后需通过 Hyper-V 管理器重新导入虚拟机。',
  },

  // ═══════════════════════════════════════════
  // WARNING — 数据库
  // 特征：数据目录含事务日志/锁文件，服务运行中迁移会损坏数据；
  //       停服务后迁移可成功，但可能需要修改配置文件中的路径
  // ═══════════════════════════════════════════
  {
    pattern: 'mysql',
    level: 'WARNING', category: '数据库', label: 'MySQL 数据目录',
    reason: 'MySQL 数据目录含事务日志和锁文件，迁移过程中若 MySQL 服务未完全停止会导致数据损坏。迁移后需修改 my.ini 中的 datadir 配置。',
  },
  {
    pattern: 'postgresql',
    level: 'WARNING', category: '数据库', label: 'PostgreSQL 数据目录',
    reason: 'PostgreSQL 数据目录需在 postgres 服务完全停止后操作，否则会损坏数据库文件。迁移后需修改配置文件中的数据目录路径。',
  },
  {
    pattern: 'mongodb',
    level: 'WARNING', category: '数据库', label: 'MongoDB 数据目录',
    reason: 'MongoDB 数据文件含内部路径引用，迁移前需停止 mongod 服务，迁移后 Junction 通常可透明使用，但建议验证数据完整性。',
  },
  {
    pattern: 'redis',
    level: 'WARNING', category: '缓存服务', label: 'Redis 数据目录',
    reason: 'Redis RDB/AOF 文件迁移后需修改 redis.conf 中的 dir 配置才能被服务正确识别，迁移前需停止 redis 服务。',
  },
  {
    pattern: 'microsoft sql server',
    level: 'WARNING', category: '数据库', label: 'SQL Server 数据目录',
    reason: 'SQL Server 数据文件（.mdf/.ldf）路径记录在系统目录中，迁移后需通过 SQL Server Management Studio 重新附加数据库。',
  },

  // ═══════════════════════════════════════════
  // WARNING — 安全软件
  // 特征：含内核级驱动，路径写进注册表服务项；
  //       迁移后驱动可能无法加载，但通过重装安全软件可恢复，不影响系统本身
  // ═══════════════════════════════════════════
  {
    pattern: 'windows defender',
    level: 'WARNING', category: '安全软件', label: 'Windows Defender 目录',
    reason: 'Windows Defender 由系统服务管理，迁移后实时防护可能失效，需通过 Windows 安全中心重新启用。',
  },
  {
    pattern: 'kaspersky',
    level: 'WARNING', category: '安全软件', label: 'Kaspersky 目录',
    reason: '卡巴斯基含内核级驱动组件，迁移后驱动可能加载失败，需重新安装卡巴斯基恢复防护。',
  },
  {
    pattern: 'eset',
    level: 'WARNING', category: '安全软件', label: 'ESET 目录',
    reason: 'ESET 含内核级驱动，迁移后驱动可能无法加载，需重新安装 ESET 恢复防护功能。',
  },

  // ═══════════════════════════════════════════
  // WARNING — 系统组件缓存
  // ═══════════════════════════════════════════
  {
    pattern: 'package cache',
    level: 'WARNING', category: '系统组件', label: 'Visual Studio Package Cache',
    reason: 'Package Cache 是 Visual Studio 的本地安装包缓存，迁移后 VS 的修复和更新功能可能失效，但 VS 本身仍可正常使用。',
  },

  // ═══════════════════════════════════════════
  // WARNING — 开发工具
  // 特征：含被 Windows 内核内存映射的 DLL，复制阶段会失败；
  //       后台语言服务/索引/编译进程的 exe 不在安装目录下，进程检测无法拦截
  // ═══════════════════════════════════════════
  {
    pattern: 'microsoft visual studio',
    level: 'WARNING', category: '开发工具', label: 'Visual Studio 安装目录',
    reason: 'Visual Studio 包含被 Windows 内核持续映射的编译器和语言服务组件（如 VBCSCompiler.exe、MSBuild.exe），这些 DLL 无法在运行时复制。迁移前需完全停止所有 VS 实例、关闭所有 .NET/C++ 项目，并在任务管理器中确认没有 MSBuild、VBCSCompiler、ServiceHub 相关进程。迁移成功后 VS 仍可正常运行，但建议通过「修复安装」验证完整性。',
  },
  {
    pattern: 'jetbrains',
    level: 'WARNING', category: '开发工具', label: 'JetBrains IDE 目录',
    reason: 'JetBrains IDE（IntelliJ、Rider、GoLand 等）有后台索引服务和 JVM 进程，迁移前需完全退出所有 JetBrains 产品和后台服务（系统托盘右键退出）。',
  },

  // ═══════════════════════════════════════════
  // WARNING — ProgramData 根目录
  // 注意：必须排在 BLOCKED 的 c:\programdata\microsoft\windows 之后，
  // 确保更具体的子路径先被 BLOCKED 命中，不会降级到 WARNING
  // ═══════════════════════════════════════════
  {
    pattern: 'c:\\programdata',
    level: 'WARNING', category: '系统目录', label: 'ProgramData 根目录',
    reason: 'ProgramData 根目录包含大量系统级应用配置，整体迁移极易导致多个系统组件和服务失效。建议只迁移其中特定应用的子目录，而非整个根目录。',
  },
];

// BLOCKED 各 category 的通用提示文案
const BLOCKED_CATEGORY_TIPS: Record<string, string> = {
  '系统目录': '迁移系统核心目录会导致 Windows 组件崩溃，无法开机。',
  '浏览器': '浏览器安装目录含系统级注册和自动修复机制，迁移后链接会被自动覆盖，且所有扩展插件将损坏。\n如需释放空间，请迁移浏览器缓存（在「数据迁移」页面的「应用数据」分区中）。',
  'GPU驱动': 'GPU 驱动路径写死进系统服务注册表，迁移后驱动无法加载，轻则降级到基本显示模式，重则蓝屏。',
  '开发工具': '开发工具目录含被 Windows 内核内存映射的 DLL 和后台语言服务，复制阶段容易失败，迁移前需完全退出所有相关进程。',
};

// WARNING 统一免责声明
const WARNING_DISCLAIMER = `迁移此类目录存在以下风险：
• 相关服务或软件迁移后可能无法正常启动
• 部分配置文件含硬编码路径，迁移后需手动修改
• 若相关服务正在运行，强行迁移可能导致数据损坏

请确认已完成以下操作后再继续：
1. 已在「服务」管理器（services.msc）中完全停止相关服务
2. 已备份重要数据
3. 了解迁移后可能需要手动修改配置文件

Viap 作者对因迁移此类目录导致的数据损失不承担责任。`;

/**
 * 危险路径检测 Hook
 * 返回两级检测函数：checkBlocked 直接终止，checkWarning 弹窗确认后放行
 */
export function useDangerousPathCheck(): {
  checkBlocked: (sourcePath: string) => string | null;
  checkWarning: (sourcePath: string) => WarningInfo | null;
} {
  const checkBlocked = useCallback((sourcePath: string): string | null => {
    const normalized = sourcePath.toLowerCase().replace(/\//g, '\\');

    for (const rule of DANGER_RULES) {
      if (rule.level !== 'BLOCKED') continue;
      if (normalized.includes(rule.pattern)) {
        const tip = BLOCKED_CATEGORY_TIPS[rule.category]
          ?? '该目录包含系统级组件，不支持迁移。';
        return `🚫 无法迁移：${rule.label} 属于「${rule.category}」，不支持通过 Junction 迁移。\n\n${tip}`;
      }
    }
    return null;
  }, []);

  const checkWarning = useCallback((sourcePath: string): WarningInfo | null => {
    const normalized = sourcePath.toLowerCase().replace(/\//g, '\\');

    for (const rule of DANGER_RULES) {
      if (rule.level !== 'WARNING') continue;
      if (normalized.includes(rule.pattern)) {
        return {
          label: rule.label,
          category: rule.category,
          reason: rule.reason,
          disclaimer: WARNING_DISCLAIMER,
        };
      }
    }
    return null;
  }, []);

  return { checkBlocked, checkWarning };
}
