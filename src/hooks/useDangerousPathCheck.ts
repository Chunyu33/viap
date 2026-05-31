// 危险路径检测 Hook
// 与后端 migration.rs 的 check_dangerous_path 保持规则同步，
// 在迁移前拦截系统目录、浏览器安装目录、GPU 驱动等不可迁移路径
// 前端提前拦截，后端兜底防线

import { useCallback } from 'react';

type DangerCategory = '系统目录' | '浏览器' | 'GPU驱动';

interface DangerRule {
  pattern: string;
  category: DangerCategory;
  label: string;
}

const DANGER_RULES: DangerRule[] = [
  // 系统核心目录
  { pattern: 'c:\\windows', category: '系统目录', label: 'Windows 系统目录' },
  { pattern: 'c:\\program files\\windowsapps', category: '系统目录', label: 'Windows 应用商店目录' },
  { pattern: 'c:\\programdata\\microsoft\\windows', category: '系统目录', label: 'Windows 系统数据目录' },
  // 系统级浏览器安装目录（含注册和自动修复机制，不可 Junction）
  { pattern: 'microsoft\\edge\\application', category: '浏览器', label: 'Microsoft Edge 安装目录' },
  { pattern: 'microsoft\\msedge\\application', category: '浏览器', label: 'Microsoft Edge 安装目录' },
  { pattern: 'microsoft\\edgewebview\\application', category: '浏览器', label: 'Microsoft WebView2 运行时目录' },
  { pattern: 'google\\chrome\\application', category: '浏览器', label: 'Google Chrome 安装目录' },
  { pattern: 'google\\chrome beta\\application', category: '浏览器', label: 'Google Chrome Beta 安装目录' },
  { pattern: 'google\\chrome dev\\application', category: '浏览器', label: 'Google Chrome Dev 安装目录' },
  { pattern: 'bromite\\application', category: '浏览器', label: 'Bromite 安装目录' },
  // GPU 驱动目录（路径写死进系统服务注册表）
  { pattern: 'nvidia corporation\\installer2', category: 'GPU驱动', label: 'NVIDIA 驱动安装目录' },
  { pattern: 'nvidia\\displaydriver', category: 'GPU驱动', label: 'NVIDIA 显卡驱动目录' },
  { pattern: '\\nvidia\\', category: 'GPU驱动', label: 'NVIDIA 驱动目录' },
  { pattern: 'amd\\ccc2', category: 'GPU驱动', label: 'AMD 显卡控制中心目录' },
  { pattern: 'advanced micro devices', category: 'GPU驱动', label: 'AMD 驱动目录' },
  { pattern: 'intel\\graphics', category: 'GPU驱动', label: 'Intel 核显驱动目录' },
  { pattern: 'intel\\intelgraphicscontrolpanel', category: 'GPU驱动', label: 'Intel 显卡控制面板目录' },
];

const CATEGORY_TIPS: Record<DangerCategory, string> = {
  '系统目录': '迁移系统核心目录会导致 Windows 组件崩溃，无法开机。',
  '浏览器': '浏览器安装目录含系统级注册和自动修复机制，迁移后链接会被自动覆盖，且所有扩展插件将损坏。\n如需释放空间，请迁移浏览器缓存（在「数据迁移」页面的「应用数据」分区中）。',
  'GPU驱动': 'GPU 驱动路径写死进系统服务注册表，迁移后驱动无法加载，轻则降级到基本显示模式，重则蓝屏。',
};

/**
 * 危险路径检测 Hook
 * 返回一个纯函数，接收 sourcePath，命中危险规则时返回中文提示文案，否则返回 null
 * 规则表为模块级常量，不随组件渲染重建
 */
export function useDangerousPathCheck(): (sourcePath: string) => string | null {
  return useCallback((sourcePath: string): string | null => {
    const normalized = sourcePath.toLowerCase().replace(/\//g, '\\');

    for (const rule of DANGER_RULES) {
      if (normalized.includes(rule.pattern)) {
        const tip = CATEGORY_TIPS[rule.category] ?? '该目录包含系统级组件，不支持迁移。';
        return `🚫 无法迁移：${rule.label} 属于「${rule.category}」，不支持通过 Junction 迁移。\n\n${tip}`;
      }
    }

    return null;
  }, []);
}
