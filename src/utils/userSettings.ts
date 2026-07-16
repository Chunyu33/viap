// 用户设置统一持久化工具
// Rust 文件是唯一持久化来源，localStorage 只作为旧版本兼容缓存和启动兜底。

import { invoke } from '@tauri-apps/api/core';
import { normalizeFontSizePx } from './fontSize';

export type UserThemeMode = 'light' | 'dark' | 'system';

export interface UserSettings {
  defaultAppTargetPath: string;
  defaultDataTargetPath: string;
  useRecycleBin: boolean;
  showScanDebug: boolean;
  fontSizePx: number;
  theme: UserThemeMode;
}

interface UserSettingsLoadResult {
  settings: UserSettings;
  initialized: boolean;
}

export const SETTINGS_KEY = 'viap_settings';
export const THEME_STORAGE_KEY = 'viap-theme';

export const DEFAULT_USER_SETTINGS: UserSettings = {
  defaultAppTargetPath: '',
  defaultDataTargetPath: '',
  useRecycleBin: true,
  showScanDebug: false,
  fontSizePx: 13,
  theme: 'system',
};

function isThemeMode(value: unknown): value is UserThemeMode {
  return value === 'light' || value === 'dark' || value === 'system';
}

function stringOrDefault(value: unknown, fallback: string): string {
  return typeof value === 'string' ? value : fallback;
}

function migrateLegacySettings(raw: Record<string, unknown>): Record<string, unknown> {
  if (typeof raw.defaultTargetPath === 'string' && raw.defaultTargetPath) {
    // 旧版本只有一个默认目录，继续把它作为应用迁移目录，避免升级后丢失选择。
    return { ...raw, defaultAppTargetPath: raw.defaultTargetPath };
  }
  return raw;
}

export function normalizeUserSettings(input: Partial<UserSettings>): UserSettings {
  return {
    defaultAppTargetPath: stringOrDefault(input.defaultAppTargetPath, DEFAULT_USER_SETTINGS.defaultAppTargetPath),
    defaultDataTargetPath: stringOrDefault(input.defaultDataTargetPath, DEFAULT_USER_SETTINGS.defaultDataTargetPath),
    useRecycleBin: input.useRecycleBin !== false,
    showScanDebug: input.showScanDebug === true,
    fontSizePx: normalizeFontSizePx(input.fontSizePx),
    theme: isThemeMode(input.theme) ? input.theme : DEFAULT_USER_SETTINGS.theme,
  };
}

export function readLocalUserSettings(): UserSettings {
  let raw: Record<string, unknown> = {};
  try {
    const saved = localStorage.getItem(SETTINGS_KEY);
    if (saved) raw = migrateLegacySettings(JSON.parse(saved) as Record<string, unknown>);
  } catch {
    // localStorage 损坏时使用默认值，随后由启动流程写回 Rust 设置文件。
  }

  let theme: UserThemeMode = DEFAULT_USER_SETTINGS.theme;
  try {
    const storedTheme = localStorage.getItem(THEME_STORAGE_KEY);
    if (isThemeMode(storedTheme)) theme = storedTheme;
  } catch {
    // WebView 存储不可用时保留 system 默认主题。
  }

  return normalizeUserSettings({ ...raw, theme });
}

export function mirrorUserSettings(settings: UserSettings): void {
  const normalized = normalizeUserSettings(settings);
  try {
    // 保持旧页面仍可同步读取这些字段，避免一次升级需要同时改动所有调用点。
    const { theme: _theme, ...legacySettings } = normalized;
    localStorage.setItem(SETTINGS_KEY, JSON.stringify(legacySettings));
    localStorage.setItem(THEME_STORAGE_KEY, normalized.theme);
  } catch {
    // Rust 设置文件仍是主存储，WebView 缓存写入失败不阻断应用启动。
  }
}

export async function bootstrapUserSettings(): Promise<UserSettings> {
  try {
    const result = await invoke<UserSettingsLoadResult>('get_user_settings');
    if (result.initialized) {
      const settings = normalizeUserSettings(result.settings);
      mirrorUserSettings(settings);
      return settings;
    }

    const legacySettings = readLocalUserSettings();
    await invoke('save_user_settings', { settings: legacySettings });
    mirrorUserSettings(legacySettings);
    return legacySettings;
  } catch {
    // 开发模式或旧版本后端没有该命令时，继续使用已有 WebView 设置。
    return readLocalUserSettings();
  }
}

export function persistUserSettings(settings: UserSettings): Promise<void> {
  const normalized = normalizeUserSettings(settings);
  mirrorUserSettings(normalized);
  return invoke('save_user_settings', { settings: normalized });
}
