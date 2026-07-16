// 主题切换 Hook
// 支持三种模式：浅色 (light)、深色 (dark)、跟随系统 (system)
//
// "跟随系统"逻辑说明（中文）：
// 1. 使用 window.matchMedia('(prefers-color-scheme: dark)') 检测系统主题偏好
// 2. 添加 change 事件监听器，实时响应系统主题变化
// 3. 当用户选择 "system" 模式时，根据系统偏好自动切换 light/dark
// 4. 用户选择存储在 localStorage 中，刷新页面后保持选择
// 5. 通过修改 document.documentElement 的 data-theme 属性实现主题切换

import { useState, useLayoutEffect, useCallback } from 'react';
import { readLocalUserSettings, persistUserSettings, type UserThemeMode } from '../utils/userSettings';

// 主题类型定义
export type ThemeMode = UserThemeMode;
export type ResolvedTheme = 'light' | 'dark';

// 获取系统主题偏好
function getSystemTheme(): ResolvedTheme {
  if (typeof window === 'undefined') return 'light';
  return window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
}

// 从 localStorage 读取用户主题选择
function getStoredTheme(): ThemeMode {
  if (typeof window === 'undefined') return 'system';
  return readLocalUserSettings().theme;
}

// 应用主题到 DOM
function applyTheme(theme: ResolvedTheme) {
  const root = document.documentElement;
  
  // 添加过渡类，确保切换平滑
  root.classList.add('theme-transition');
  
  // 设置主题属性
  root.setAttribute('data-theme', theme);
  
  // 移除过渡类（延迟执行，让过渡完成）
  setTimeout(() => {
    root.classList.remove('theme-transition');
  }, 300);
}

// 主题 Hook
export function useTheme() {
  // 用户选择的主题模式
  const [mode, setMode] = useState<ThemeMode>(() => getStoredTheme());
  
  // 实际应用的主题（解析 system 后的结果）
  const [resolvedTheme, setResolvedTheme] = useState<ResolvedTheme>(() => {
    const stored = getStoredTheme();
    return stored === 'system' ? getSystemTheme() : stored;
  });

  // 切换主题模式
  const setTheme = useCallback((newMode: ThemeMode) => {
    setMode(newMode);
    // 主题与其他用户设置写入同一个 Rust 文件，便携版复制目录后可以完整保留。
    persistUserSettings({ ...readLocalUserSettings(), theme: newMode }).catch(() => undefined);
    
    // 计算实际主题
    const resolved = newMode === 'system' ? getSystemTheme() : newMode;
    setResolvedTheme(resolved);
    applyTheme(resolved);
  }, []);

  // 初始化和监听系统主题变化
  useLayoutEffect(() => {
    // 在浏览器绘制启动页前应用主题，避免深色模式先闪过浅色背景。
    const initialResolved = mode === 'system' ? getSystemTheme() : mode;
    setResolvedTheme(initialResolved);
    applyTheme(initialResolved);

    // 监听系统主题变化（仅在 system 模式下生效）
    const mediaQuery = window.matchMedia('(prefers-color-scheme: dark)');
    
    const handleSystemChange = (e: MediaQueryListEvent) => {
      // 仅当用户选择"跟随系统"时响应系统变化
      const currentMode = readLocalUserSettings().theme;
      if (currentMode === 'system') {
        const newTheme = e.matches ? 'dark' : 'light';
        setResolvedTheme(newTheme);
        applyTheme(newTheme);
      }
    };

    // 添加监听器
    mediaQuery.addEventListener('change', handleSystemChange);

    // 清理监听器
    return () => {
      mediaQuery.removeEventListener('change', handleSystemChange);
    };
  }, [mode]);

  return {
    mode,           // 用户选择的模式：'light' | 'dark' | 'system'
    theme: resolvedTheme,  // 实际应用的主题：'light' | 'dark'
    setTheme,       // 设置主题模式
    isDark: resolvedTheme === 'dark',  // 便捷属性：是否为深色模式
  };
}

// 导出默认 Hook
export default useTheme;
