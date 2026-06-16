// 设置页面 — 桌面工具风格
// 克制配色，紧凑布局

import { useState, useEffect, type CSSProperties } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { open, confirm } from '@tauri-apps/plugin-dialog';
import { getVersion } from '@tauri-apps/api/app';
import { useUpdater } from '../hooks/useUpdater';
import AppIconSvg from '../assets/icon.svg';
import {
  FolderCog, ChevronRight, Copy, Check,
  FolderArchive, Trash2, RefreshCw,
  AppWindow, Loader2, Sun, Moon, Monitor, Database,
  Github, ExternalLink, BookOpen, Heart, Rocket,
  Video, Users, MessageSquare, Activity,
} from 'lucide-react';
import { useThemeContext } from '../App';
import type { ThemeMode } from '../hooks/useTheme';
import Toast, { useToast } from '../components/Toast';
import UserManual from '../components/UserManual';
import DonateModal from '../components/DonateModal';
import ProjectPromoModal from '../components/ProjectPromoModal';
import Modal from '../components/Modal';
import type { DataDirConfig, GhostLinkPreview } from '../types';
import {
  applyFontSize,
  DEFAULT_FONT_SIZE_PX,
  MAX_FONT_SIZE_PX,
  MIN_FONT_SIZE_PX,
  normalizeFontSizePx,
} from '../utils/fontSize';

interface MigrationStats {
  total_space_saved: number;
  active_migrations: number;
  restored_count: number;
  app_migrations: number;
  folder_migrations: number;
}

interface CleanupResult {
  cleaned_count: number;
  cleaned_size: number;
  errors: string[];
}

function formatSize(bytes: number): string {
  if (bytes === 0) return '0 B';
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}

const APP_INFO = {
  name: 'Viap',
  description: 'Windows 应用管理与存储重定向工具',
  email: '1378813463@qq.com',
};

const SETTINGS_KEY = 'viap_settings';
// 默认目标路径初始为空，由用户手动配置；仅允许选择 C 盘以外的目录
const DEFAULT_SETTINGS = {
  defaultAppTargetPath: '',
  defaultDataTargetPath: '',
  useRecycleBin: true,
  showScanDebug: false,
  fontSizePx: DEFAULT_FONT_SIZE_PX,
};

/** 迁移旧版设置：将 defaultTargetPath 升迁为 defaultAppTargetPath */
function migrateOldSettings(raw: Record<string, unknown>): Record<string, unknown> {
  if (typeof raw.defaultTargetPath === 'string' && raw.defaultTargetPath) {
    return { ...raw, defaultAppTargetPath: raw.defaultTargetPath, defaultTargetPath: undefined };
  }
  return raw;
}

function loadSettings() {
  try {
    const saved = localStorage.getItem(SETTINGS_KEY);
    if (saved) {
      const raw = JSON.parse(saved);
      const migrated = migrateOldSettings(raw);
      const merged = { ...DEFAULT_SETTINGS, ...migrated };
      return { ...merged, fontSizePx: normalizeFontSizePx(merged.fontSizePx) };
    }
  } catch { /* ignore */ }
  return DEFAULT_SETTINGS;
}

function saveSettings(s: typeof DEFAULT_SETTINGS) {
  try { localStorage.setItem(SETTINGS_KEY, JSON.stringify(s)); } catch { /* ignore */ }
}

const FONT_SIZE_PRESETS = [
  { label: '标准', value: 13 },
  { label: '适中', value: 14 },
  { label: '较大', value: 15 },
];

function Toggle({ active, onChange }: { active: boolean; onChange: () => void }) {
  return (
    <button
      onClick={onChange}
      className="relative flex-shrink-0 rounded-full cursor-pointer transition-colors"
      style={{ width: '36px', height: '20px', background: active ? 'var(--color-primary)' : 'var(--color-gray-300)' }}
    >
      <span className="absolute top-0.5 w-4 h-4 bg-white rounded-full shadow-sm transition-all"
        style={{ left: active ? '18px' : '2px' }} />
    </button>
  );
}

function ThemeButton({ mode, currentMode, onClick, icon, label }: {
  mode: ThemeMode; currentMode: ThemeMode; onClick: () => void; icon: React.ReactNode; label: string;
}) {
  const isActive = mode === currentMode;
  return (
    <button onClick={onClick} title={label}
      className={`flex items-center justify-center w-8 h-6 rounded border-none cursor-pointer transition-all ${
        isActive ? '' : 'opacity-50 hover:opacity-100'
      }`}
      style={{
        color: isActive ? 'var(--color-primary)' : 'var(--text-tertiary)',
        background: isActive ? 'var(--color-primary-light)' : 'transparent',
      }}>
      {icon}
    </button>
  );
}

// section header
function SectionHeader({ label }: { label: string }) {
  return (
    <div className="text-[10px] uppercase tracking-wider font-medium mb-2 px-1"
      style={{ color: 'var(--text-tertiary)' }}>{label}</div>
  );
}

/** 问题反馈弹窗 — 复用 Modal 组件，展示邮箱/GitHub Issues/QQ群 */
function FeedbackModal({ isOpen, onClose, email, copiedLabel, onCopy }: {
  isOpen: boolean;
  onClose: () => void;
  email: string;
  copiedLabel: string | null;
  onCopy: (text: string, label: string) => void;
}) {
  const ISSUES_URL = 'https://github.com/Chunyu33/viap/issues';
  const QQ_GROUP = '834582563';

  // 弹窗内复制的样式：左侧标签 + 右侧值 + 复制/打开按钮
  const rowStyle: React.CSSProperties = {
    display: 'flex', alignItems: 'center', justifyContent: 'space-between',
    padding: '12px 0', borderBottom: '1px solid var(--border-color)',
  };
  const labelStyle: React.CSSProperties = {
    fontSize: '13px', fontWeight: 500, color: 'var(--text-primary)',
  };
  const valueStyle: React.CSSProperties = {
    fontSize: '12px', color: 'var(--text-tertiary)', fontFamily: 'monospace',
  };

  return (
    <Modal isOpen={isOpen} onClose={onClose} title="反馈/建议" width={480}>
      {/* 邮箱 */}
      <div style={rowStyle}>
        <span style={labelStyle}>📧 邮箱</span>
        <div className="flex items-center gap-2">
          <span style={valueStyle}>{email}</span>
          <button
            onClick={() => onCopy(email, 'email')}
            className="flex items-center gap-1 text-[11px] transition-colors cursor-pointer border-none bg-transparent"
            style={{ color: copiedLabel === 'email' ? 'var(--color-primary)' : 'var(--text-tertiary)' }}
          >
            {copiedLabel === 'email'
              ? <Check className="w-3 h-3" style={{ color: 'var(--color-primary)' }} />
              : <Copy className="w-3 h-3" />
            }
            {copiedLabel === 'email' ? '已复制' : '复制'}
          </button>
        </div>
      </div>

      {/* GitHub Issues */}
      <div style={rowStyle}>
        <span style={labelStyle}>🔗 GitHub Issues</span>
        <div className="flex items-center gap-2">
          <a
            href={ISSUES_URL}
            target="_blank"
            rel="noopener noreferrer"
            className="flex items-center gap-1 text-[11px] no-underline transition-colors cursor-pointer"
            style={{ color: 'var(--text-secondary)' }}
          >
            打开 <ExternalLink className="w-3 h-3" />
          </a>
          <span style={{ color: 'var(--border-color)', fontSize: '11px' }}>|</span>
          <button
            onClick={() => onCopy(ISSUES_URL, 'issues')}
            className="flex items-center gap-1 text-[11px] transition-colors cursor-pointer border-none bg-transparent"
            style={{ color: copiedLabel === 'issues' ? 'var(--color-primary)' : 'var(--text-tertiary)' }}
          >
            {copiedLabel === 'issues'
              ? <Check className="w-3 h-3" style={{ color: 'var(--color-primary)' }} />
              : <Copy className="w-3 h-3" />
            }
            {copiedLabel === 'issues' ? '已复制' : '复制'}
          </button>
        </div>
      </div>

      {/* QQ 交流群 */}
      <div style={{ ...rowStyle, borderBottom: 'none' }}>
        <span style={labelStyle}>💬 QQ交流群</span>
        <div className="flex items-center gap-2">
          <span style={valueStyle}>{QQ_GROUP}</span>
          <button
            onClick={() => onCopy(QQ_GROUP, 'feedback-qq')}
            className="flex items-center gap-1 text-[11px] transition-colors cursor-pointer border-none bg-transparent"
            style={{ color: copiedLabel === 'feedback-qq' ? 'var(--color-primary)' : 'var(--text-tertiary)' }}
          >
            {copiedLabel === 'feedback-qq'
              ? <Check className="w-3 h-3" style={{ color: 'var(--color-primary)' }} />
              : <Copy className="w-3 h-3" />
            }
            {copiedLabel === 'feedback-qq' ? '已复制' : '复制'}
          </button>
        </div>
      </div>
    </Modal>
  );
}

export default function Settings({ visible: _visible }: { visible: boolean }) {
  const [settings, setSettings] = useState(DEFAULT_SETTINGS);
  const [stats, setStats] = useState<MigrationStats | null>(null);
  const [cleaning, setCleaning] = useState(false);
  const [cleanResult, setCleanResult] = useState<CleanupResult | null>(null);
  const [ghostPreview, setGhostPreview] = useState<GhostLinkPreview | null>(null);
  const [ghostScanning, setGhostScanning] = useState(false);
  const [manualOpen, setManualOpen] = useState(false);
  const [donateModalOpen, setDonateModalOpen] = useState(false);
  const [promoModalOpen, setPromoModalOpen] = useState(false);
  const [feedbackModalOpen, setFeedbackModalOpen] = useState(false);
  const [appVersion, setAppVersion] = useState('...');
  const [dataDir, setDataDir] = useState('');
  const [dataDirLoading, setDataDirLoading] = useState(false);
  const [copiedLabel, setCopiedLabel] = useState<string | null>(null);
  const currentYear = new Date().getFullYear();

  /** 一键复制到剪贴板，成功后短暂显示已复制状态 */
  async function handleCopy(text: string, label: string) {
    try {
      await navigator.clipboard.writeText(text);
      setCopiedLabel(label);
      setTimeout(() => setCopiedLabel(null), 1500);
    } catch { /* 剪贴板不可用则静默忽略 */ }
  }

  const { toast, showToast, hideToast } = useToast();
  const themeState = useThemeContext();
  const { status: updateStatus, updateInfo, downloadProgress, checkForUpdate, downloadAndInstall } = useUpdater();

  useEffect(() => {
    setSettings(loadSettings());
    loadStats();
    loadDataDir();
    getVersion().then(setAppVersion).catch(() => setAppVersion('1.0.0'));
  }, []);

  async function loadStats() {
    try { setStats(await invoke<MigrationStats>('get_migration_stats')); }
    catch { /* ignore */ }
  }
  async function loadDataDir() {
    try { const info = await invoke<DataDirConfig>('get_data_dir_info'); setDataDir(info.data_dir); }
    catch { /* ignore */ }
  }

  async function handleChangeDataDir() {
    const selected = await open({ directory: true, multiple: false, title: '选择新的数据存储目录' });
    if (!selected || typeof selected !== 'string') return;
    const confirmed = await confirm(
      `数据目录将从:\n${dataDir}\n\n迁移到:\n${selected}\n\n所有数据将自动复制到新位置。`,
      { title: '确认迁移数据目录', kind: 'warning', okLabel: '确认迁移', cancelLabel: '取消' }
    );
    if (!confirmed) return;
    setDataDirLoading(true);
    try {
      await invoke('set_data_dir', { newPath: selected });
      setDataDir(selected);
      showToast('数据目录已成功迁移', 'success');
    } catch (e) { showToast(`迁移失败: ${e}`, 'error'); }
    finally { setDataDirLoading(false); }
  }

  async function handleOpenDataDir() {
    try { await invoke('open_data_dir'); }
    catch (e) { showToast(`打开失败: ${e}`, 'error'); }
  }

  async function handlePreviewGhostLinks() {
    try {
      setGhostScanning(true); setGhostPreview(null); setCleanResult(null);
      const result = await invoke<GhostLinkPreview>('preview_ghost_links');
      setGhostPreview(result);
      if (result.entries.length === 0) {
        showToast('未发现无效记录', 'info');
      }
    } catch { /* ignore */ }
    finally { setGhostScanning(false); }
  }

  async function handleCleanGhostLinks() {
    try {
      setCleaning(true); setCleanResult(null);
      const result = await invoke<CleanupResult>('clean_ghost_links');
      setCleanResult(result); setGhostPreview(null);
      await loadStats();
    } catch { /* ignore */ }
    finally { setCleaning(false); }
  }

  const updateSetting = <K extends keyof typeof DEFAULT_SETTINGS>(k: K, v: typeof DEFAULT_SETTINGS[K]) => {
    const nextValue = k === 'fontSizePx' ? normalizeFontSizePx(v) : v;
    const ns = { ...settings, [k]: nextValue };
    setSettings(ns);
    saveSettings(ns);
    // 字号是外观设置，保存后立即写入 CSS 变量，四个模块无需刷新即可生效。
    if (k === 'fontSizePx') applyFontSize(nextValue);
  };

  const fontPresetIndex = FONT_SIZE_PRESETS.findIndex(preset => preset.value === settings.fontSizePx);
  const fontRangeProgress = ((settings.fontSizePx - MIN_FONT_SIZE_PX) / (MAX_FONT_SIZE_PX - MIN_FONT_SIZE_PX)) * 100;

  /** 选择默认应用迁移目录（C 盘以外的目录） */
  const handleSelectAppTargetPath = async () => {
    const selected = await open({ directory: true, multiple: false, title: '选择默认应用迁移目录文件夹' });
    if (selected && typeof selected === 'string') updateSetting('defaultAppTargetPath', selected);
  };

  /** 选择默认数据迁移目录（C 盘以外的目录） */
  const handleSelectDataTargetPath = async () => {
    const selected = await open({ directory: true, multiple: false, title: '选择默认数据迁移目录文件夹' });
    if (selected && typeof selected === 'string') updateSetting('defaultDataTargetPath', selected);
  };

  return (
    <div className="h-full overflow-auto" style={{ padding: '16px 20px' }}>
      <div className="flex flex-col gap-4" style={{ maxWidth: '640px', margin: '0 auto' }}>

        {/* stats summary — 绿色强调分隔线 + 柔和背景 */}
        {stats && stats.active_migrations > 0 && (
          <div className="relative rounded-lg overflow-hidden" style={{ background: 'var(--color-primary-light)' }}>
            {/* 左侧强调线 */}
            <div className="absolute left-0 top-0 bottom-0 w-1" style={{ background: 'var(--color-primary)' }} />
            <div className="flex items-center gap-6 py-4 px-5 text-[12px]">
              <div className="flex items-baseline gap-1.5">
                <span style={{ color: 'var(--text-secondary)' }}>已节省</span>
                <strong style={{ color: 'var(--color-primary)', fontSize: '22px', fontWeight: 600, lineHeight: 1 }}>
                  {formatSize(stats.total_space_saved)}
                </strong>
              </div>
              <div className="flex items-center gap-4 ml-auto">
                <span className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
                  {stats.active_migrations} 次迁移
                </span>
                {stats.app_migrations > 0 && (
                  <span className="text-[11px] flex items-center gap-1" style={{ color: 'var(--text-secondary)' }}>
                    <AppWindow className="w-3.5 h-3.5" style={{ color: 'var(--color-primary)' }} />
                    {stats.app_migrations} 应用
                  </span>
                )}
                {stats.folder_migrations > 0 && (
                  <span className="text-[11px] flex items-center gap-1" style={{ color: 'var(--text-secondary)' }}>
                    <FolderArchive className="w-3.5 h-3.5" style={{ color: 'var(--color-warning)' }} />
                    {stats.folder_migrations} 文件夹
                  </span>
                )}
              </div>
            </div>
          </div>
        )}

        {/* appearance */}
        <section>
          <SectionHeader label="外观" />
          <div className="rounded border" style={{ borderColor: 'var(--border-color)' }}>
            <div className="setting-item" style={{ padding: '10px 14px' }}>
              <div className="flex items-center gap-3">
                <div className="w-8 h-8 rounded flex items-center justify-center" style={{ background: 'var(--bg-row-hover)' }}>
                  {themeState.isDark ? <Moon className="w-4 h-4" style={{ color: 'var(--color-primary)' }} />
                    : <Sun className="w-4 h-4" style={{ color: 'var(--color-primary)' }} />}
                </div>
                <div>
                  <p className="setting-label">主题模式</p>
                  <p className="setting-desc">浅色、深色或跟随系统</p>
                </div>
              </div>
              <div className="flex items-center rounded p-0.5 gap-0.5" style={{ background: 'var(--bg-row-hover)' }}>
                <ThemeButton mode="light" currentMode={themeState.mode} onClick={() => themeState.setTheme('light')}
                  icon={<Sun className="w-4 h-4" />} label="浅色" />
                <ThemeButton mode="dark" currentMode={themeState.mode} onClick={() => themeState.setTheme('dark')}
                  icon={<Moon className="w-4 h-4" />} label="深色" />
                <ThemeButton mode="system" currentMode={themeState.mode} onClick={() => themeState.setTheme('system')}
                  icon={<Monitor className="w-4 h-4" />} label="系统" />
              </div>
            </div>
            <div className="setting-item" style={{ padding: '10px 14px', borderTop: '1px solid var(--border-color)' }}>
              <div className="flex items-center gap-3">
                <div className="w-8 h-8 rounded flex items-center justify-center" style={{ background: 'var(--bg-row-hover)' }}>
                  <AppWindow className="w-4 h-4" style={{ color: 'var(--color-primary)' }} />
                </div>
                <div>
                  <p className="setting-label">字体大小</p>
                  <p className="setting-desc">调整应用内文字大小，范围 {MIN_FONT_SIZE_PX}-{MAX_FONT_SIZE_PX}px。</p>
                </div>
              </div>
              <div className="flex items-center gap-2 flex-shrink-0">
                <div
                  className="relative flex h-7 w-[132px] items-center overflow-hidden rounded p-0.5"
                  style={{ background: 'var(--bg-row-hover)' }}
                >
                  <span
                    className="absolute inset-y-0.5 left-0.5 rounded transition-all duration-200 ease-out"
                    style={{
                      width: 'calc((100% - 4px) / 3)',
                      opacity: fontPresetIndex >= 0 ? 1 : 0,
                      transform: `translateX(${fontPresetIndex * 100}%)`,
                      background: 'var(--color-primary)',
                    }}
                  />
                  {FONT_SIZE_PRESETS.map(preset => (
                    <button
                      key={preset.value}
                      type="button"
                      onClick={() => updateSetting('fontSizePx', preset.value)}
                      className="relative z-10 h-full flex-1 rounded text-[11px] transition-colors"
                      style={{
                        color: settings.fontSizePx === preset.value ? 'var(--text-inverse)' : 'var(--text-tertiary)',
                      }}
                    >
                      {preset.label}
                    </button>
                  ))}
                </div>
                <input
                  type="range"
                  min={MIN_FONT_SIZE_PX}
                  max={MAX_FONT_SIZE_PX}
                  step={1}
                  value={settings.fontSizePx}
                  onChange={(e) => updateSetting('fontSizePx', Number(e.target.value))}
                  className="theme-range w-24"
                  style={{ '--range-progress': `${fontRangeProgress}%` } as CSSProperties}
                  title="调整字体大小"
                />
                <input
                  type="number"
                  min={MIN_FONT_SIZE_PX}
                  max={MAX_FONT_SIZE_PX}
                  value={settings.fontSizePx}
                  onChange={(e) => updateSetting('fontSizePx', Number(e.target.value))}
                  className="h-7 w-14 rounded border px-2 text-[12px] outline-none"
                  style={{
                    borderColor: 'var(--border-color)',
                    background: 'var(--bg-input)',
                    color: 'var(--text-primary)',
                  }}
                />
              </div>
            </div>
          </div>
        </section>

        {/* migration settings */}
        <section>
          <SectionHeader label="迁移设置" />
          <div className="rounded border" style={{ borderColor: 'var(--border-color)' }}>
            {/* 默认应用迁移目录 */}
            <button onClick={handleSelectAppTargetPath}
              className="setting-item setting-item-clickable w-full text-left"
              style={{ padding: '10px 14px', borderBottom: '1px solid var(--border-color)', cursor: 'pointer' }}>
              <div className="flex items-center gap-3">
                <div className="w-8 h-8 rounded flex items-center justify-center" style={{ background: 'var(--bg-row-hover)' }}>
                  <FolderCog className="w-4 h-4" style={{ color: 'var(--color-primary)' }} />
                </div>
                <div>
                  <p className="setting-label">默认应用迁移目录</p>
                  <p className="setting-desc">
                    {settings.defaultAppTargetPath
                      ? settings.defaultAppTargetPath.startsWith('C:') || settings.defaultAppTargetPath.startsWith('c:')
                        ? '⚠ 请选择 C 盘以外的目录'
                        : settings.defaultAppTargetPath
                      : '未设置，迁移时将提示选择目录'}
                  </p>
                </div>
              </div>
              <ChevronRight className="w-3.5 h-3.5 flex-shrink-0" style={{ color: 'var(--text-tertiary)' }} />
            </button>
            {/* 默认数据迁移目录 */}
            <button onClick={handleSelectDataTargetPath}
              className="setting-item setting-item-clickable w-full text-left"
              style={{ padding: '10px 14px', borderBottom: '1px solid var(--border-color)', cursor: 'pointer' }}>
              <div className="flex items-center gap-3">
                <div className="w-8 h-8 rounded flex items-center justify-center" style={{ background: 'var(--bg-row-hover)' }}>
                  <FolderArchive className="w-4 h-4" style={{ color: 'var(--color-warning)' }} />
                </div>
                <div>
                  <p className="setting-label">默认数据迁移目录</p>
                  <p className="setting-desc">
                    {settings.defaultDataTargetPath
                      ? settings.defaultDataTargetPath.startsWith('C:') || settings.defaultDataTargetPath.startsWith('c:')
                        ? '⚠ 请选择 C 盘以外的目录'
                        : settings.defaultDataTargetPath
                      : '未设置，迁移时将提示选择目录'}
                  </p>
                </div>
              </div>
              <ChevronRight className="w-3.5 h-3.5 flex-shrink-0" style={{ color: 'var(--text-tertiary)' }} />
            </button>
            <div className="setting-item" style={{ padding: '10px 14px' }}>
              <div className="flex items-center gap-3">
                <div className="w-8 h-8 rounded flex items-center justify-center" style={{ background: 'var(--bg-row-hover)' }}>
                  <Trash2 className="w-4 h-4" style={{ color: 'var(--text-secondary)' }} />
                </div>
                <div>
                  <p className="setting-label">删除文件移入回收站</p>
                  <p className="setting-desc">关闭后直接彻底删除</p>
                </div>
              </div>
              <Toggle active={settings.useRecycleBin} onChange={() => updateSetting('useRecycleBin', !settings.useRecycleBin)} />
            </div>
          </div>
        </section>

        {/* data management */}
        <section>
          <SectionHeader label="数据管理" />
          <div className="rounded border" style={{ borderColor: 'var(--border-color)' }}>
            <div className="setting-item" style={{ padding: '10px 14px' }}>
              <div className="flex items-center gap-3 flex-1 min-w-0">
                <div className="w-8 h-8 rounded flex items-center justify-center flex-shrink-0" style={{ background: 'var(--bg-row-hover)' }}>
                  <Database className="w-4 h-4" style={{ color: 'var(--color-primary)' }} />
                </div>
                <div className="min-w-0 flex-1">
                  <p className="setting-label">数据存储目录</p>
                  {dataDir && <p className="text-[11px] truncate font-mono" style={{ color: 'var(--text-tertiary)' }} title={dataDir}>{dataDir}</p>}
                </div>
              </div>
              <div className="flex items-center gap-1.5 flex-shrink-0">
                <button onClick={handleChangeDataDir} disabled={dataDirLoading} className="btn h-7 text-[11px]">
                  {dataDirLoading ? <Loader2 className="w-3 h-3 animate-spin" /> : <FolderCog className="w-3 h-3" />}
                  {dataDirLoading ? '迁移中' : '更改'}
                </button>
                <button onClick={handleOpenDataDir} className="btn h-7 text-[11px]">
                  <FolderArchive className="w-3 h-3" />
                  前往
                </button>
              </div>
            </div>
          </div>
        </section>

        {/* other settings */}
        <section>
          <SectionHeader label="其他设置" />
          <div className="rounded border" style={{ borderColor: 'var(--border-color)' }}>
            <div className="setting-item" style={{ padding: '10px 14px' }}>
              <div className="flex items-center gap-3">
                <div className="w-8 h-8 rounded flex items-center justify-center" style={{ background: 'var(--bg-row-hover)' }}>
                  <Activity className="w-4 h-4" style={{ color: 'var(--color-primary)' }} />
                </div>
                <div>
                  <p className="setting-label">显示扫描耗时</p>
                  <p className="setting-desc">开启后在应用管理页右上角显示加载耗时，反馈问题时可截图排查。</p>
                </div>
              </div>
              <Toggle active={settings.showScanDebug} onChange={() => updateSetting('showScanDebug', !settings.showScanDebug)} />
            </div>
          </div>
        </section>

        {/* maintenance */}
        <section>
          <SectionHeader label="存储维护" />
          <div className="rounded border" style={{ borderColor: 'var(--border-color)', padding: '12px 14px' }}>
            <div className="flex items-start gap-3">
              <div className="w-8 h-8 rounded flex items-center justify-center flex-shrink-0" style={{ background: 'var(--color-danger-light)' }}>
                <Trash2 className="w-4 h-4" style={{ color: 'var(--color-danger)' }} />
              </div>
              <div className="flex-1 min-w-0">
                <p className="setting-label mb-1">清理无效记录</p>
                <p className="setting-desc" style={{ marginBottom: '20px' }}>
                  扫描并清理目标丢失、链接断裂或已消失的无效记录。先预览，再确认清理。
                </p>

                <button onClick={handlePreviewGhostLinks} disabled={ghostScanning} className="btn h-7 text-[12px]">
                  {ghostScanning ? <Loader2 className="w-3.5 h-3.5 animate-spin" /> : <Trash2 className="w-3.5 h-3.5" />}
                  {ghostScanning ? '扫描中...' : '扫描幽灵链接'}
                </button>

                {ghostPreview && ghostPreview.entries.length > 0 && (
                  <div className="mt-3">
                    <div className="rounded border p-3 mb-3 text-[11px]" style={{ borderColor: 'var(--border-color-strong)', maxHeight: '200px', overflowY: 'auto' }}>
                      <p className="font-medium mb-2" style={{ color: 'var(--color-warning)' }}>
                        发现 {ghostPreview.entries.length} 条幽灵链接（{formatSize(ghostPreview.total_size)}）
                      </p>
                      {ghostPreview.entries.map(e => (
                        <div key={e.record_id} className="py-1 border-b last:border-0" style={{ borderColor: 'var(--border-color)' }}>
                          <div className="flex items-center gap-2">
                            <span style={{ color: 'var(--text-primary)' }}>{e.app_name}</span>
                            <span className="badge text-[10px]" style={{
                              background: e.damage_type === 'target_missing'
                                ? 'var(--color-danger-light)'
                                : 'var(--color-warning-light)',
                              color: e.damage_type === 'target_missing'
                                ? 'var(--color-danger)'
                                : 'var(--color-warning)',
                            }}>
                              {e.damage_type === 'target_missing' && '目标丢失'}
                              {e.damage_type === 'junction_broken' && '链接断裂'}
                              {e.damage_type === 'original_missing' && '源路径消失'}
                            </span>
                          </div>
                          <p className="text-[10px] mt-0.5" style={{ color: 'var(--text-tertiary)' }}>
                            {e.damage_type === 'target_missing'
                              ? `目标: ${e.target_path}`
                              : e.damage_type === 'junction_broken'
                              ? `原路径不再是链接: ${e.original_path}`
                              : `原链接已消失: ${e.original_path}`
                            }
                          </p>
                        </div>
                      ))}
                    </div>
                    <div className="flex items-center gap-2">
                      <button onClick={handleCleanGhostLinks} disabled={cleaning} className="btn btn-danger h-7 text-[11px]">
                        {cleaning ? <Loader2 className="w-3 h-3 animate-spin" /> : <Trash2 className="w-3 h-3" />}
                        {cleaning ? '清理中...' : '确认清理'}
                      </button>
                      <button onClick={() => setGhostPreview(null)} disabled={cleaning} className="btn btn-ghost h-7 text-[11px]">取消</button>
                    </div>
                  </div>
                )}

                {cleanResult && (
                  <div className="rounded p-2 text-[11px]" style={{
                    background: cleanResult.cleaned_count > 0 ? 'var(--color-success-light)' : 'var(--bg-row-hover)',
                    color: cleanResult.cleaned_count > 0 ? 'var(--color-success)' : 'var(--text-tertiary)',
                  }}>
                    {cleanResult.cleaned_count > 0
                      ? `已清理 ${cleanResult.cleaned_count} 条记录（${formatSize(cleanResult.cleaned_size)}）
`
                      : '未发现无效记录'}
                    {cleanResult.errors.length > 0 && (
                      <div style={{ color: 'var(--color-danger)', marginTop: '4px', whiteSpace: 'pre-line' }}>
                        {cleanResult.errors.map((err, i) => <div key={i}>{err}</div>)}
                      </div>
                    )}
                  </div>
                )}

                {/* export/import */}
                <div className="mt-3 pt-3" style={{ borderTop: '1px solid var(--border-color)' }}>
                  <p className="text-[11px] mb-2" style={{ color: 'var(--text-tertiary)' }}>导入/导出历史记录</p>
                  <div className="flex items-center gap-2">
                    <button onClick={async () => {
                      try {
                        const sel = await open({ directory: true, multiple: false, title: '选择导出目录' });
                        if (!sel || typeof sel !== 'string') return;
                        await invoke('export_history', { destPath: `${sel}\\migration_history.json` });
                        showToast('历史记录已导出', 'success');
                      } catch (e) { showToast(`导出失败: ${e}`, 'error'); }
                    }} className="btn h-7 text-[11px]">
                      <Database className="w-3 h-3" />导出
                    </button>
                    <button onClick={async () => {
                      try {
                        const sel = await open({ multiple: false, title: '选择历史记录文件', filters: [{ name: 'JSON', extensions: ['json'] }] });
                        if (!sel || typeof sel !== 'string') return;
                        const added = await invoke<number>('import_history', { srcPath: sel });
                        showToast(`已导入 ${added} 条新记录`, 'success'); await loadStats();
                      } catch (e) { showToast(`导入失败: ${e}`, 'error'); }
                    }} className="btn h-7 text-[11px]">
                      <Database className="w-3 h-3" />导入
                    </button>
                  </div>
                </div>
              </div>
            </div>
          </div>
        </section>

        {/* 更新 */}
        <section>
          <SectionHeader label="更新" />
          <div className="rounded border" style={{ borderColor: 'var(--border-color)', padding: '10px 14px' }}>
            <div className="flex items-center gap-3">
              <div className="w-8 h-8 rounded flex items-center justify-center" style={{ background: 'var(--bg-row-hover)' }}>
                <RefreshCw className={`w-4 h-4 ${updateStatus === 'checking' || updateStatus === 'downloading' ? 'animate-spin' : ''}`}
                  style={{ color: updateStatus === 'available' ? 'var(--color-primary)' : 'var(--text-secondary)' }} />
              </div>
              <div className="flex-1 min-w-0">
                <p className="setting-label">
                  {updateStatus === 'idle' && '检查更新'}
                  {updateStatus === 'checking' && '检测中...'}
                  {updateStatus === 'up-to-date' && '已是最新版本'}
                  {updateStatus === 'available' && updateInfo && `发现新版本 v${updateInfo.version}`}
                  {updateStatus === 'downloading' && `正在下载 ${downloadProgress}%`}
                  {updateStatus === 'installing' && '安装中...'}
                  {updateStatus === 'error' && '更新失败'}
                </p>
                <p className="setting-desc">
                  当前版本：v{appVersion}
                  {updateStatus === 'available' && updateInfo?.notes && ` — ${updateInfo.notes}`}
                </p>
              </div>
              <div className="flex items-center gap-1.5 flex-shrink-0">
                {updateStatus === 'idle' || updateStatus === 'error' || updateStatus === 'up-to-date' ? (
                  <button onClick={() => checkForUpdate()}
                    className="btn h-7 text-[11px]">
                    <RefreshCw className="w-3 h-3" />
                    {updateStatus === 'up-to-date' ? '重新检测' : '检测更新'}
                  </button>
                ) : updateStatus === 'available' ? (
                  <button onClick={() => downloadAndInstall()} className="btn btn-primary h-7 text-[11px]">
                    <RefreshCw className="w-3 h-3" />
                    立即更新
                  </button>
                ) : null}
              </div>
            </div>
          </div>
        </section>

        {/* help */}
        <section>
          <SectionHeader label="帮助" />
          <div className="rounded border" style={{ borderColor: 'var(--border-color)' }}>
            <button onClick={() => setManualOpen(true)}
              className="setting-item setting-item-clickable w-full text-left"
              style={{ padding: '10px 14px', cursor: 'pointer' }}>
              <div className="flex items-center gap-3">
                <div className="w-8 h-8 rounded flex items-center justify-center" style={{ background: 'var(--color-primary-light)' }}>
                  <BookOpen className="w-4 h-4" style={{ color: 'var(--color-primary)' }} />
                </div>
                <div>
                  <p className="setting-label">用户手册</p>
                  <p className="setting-desc">了解功能原理、软链接机制及使用协议</p>
                </div>
              </div>
              <ChevronRight className="w-3.5 h-3.5" style={{ color: 'var(--text-tertiary)' }} />
            </button>
          </div>
        </section>

        {/* about */}
        <section>
          <SectionHeader label="关于" />
          <div className="rounded border" style={{ borderColor: 'var(--border-color)' }}>
            <div className="setting-item" style={{ padding: '10px 14px', borderBottom: '1px solid var(--border-color)' }}>
              <div className="flex items-center gap-3">
                <div className="w-8 h-8 rounded flex items-center justify-center overflow-hidden">
                  <img src={AppIconSvg} alt="" className="w-8 h-8" />
                </div>
                <div>
                  <p className="setting-label">{APP_INFO.name}</p>
                  <p className="setting-desc">{APP_INFO.description}</p>
                </div>
              </div>
              <span className="badge badge-primary">v{appVersion}</span>
            </div>
            {/* GitHub */}
            <a href="https://github.com/Chunyu33/viap" target="_blank" rel="noopener noreferrer"
              className="setting-item no-underline"
              style={{ padding: '10px 14px', borderBottom: '1px solid var(--border-color)', cursor: 'pointer' }}>
              <div className="flex items-center gap-3">
                <div className="w-8 h-8 rounded flex items-center justify-center" style={{ background: 'var(--bg-row-hover)' }}>
                  <Github className="w-4 h-4" style={{ color: 'var(--text-tertiary)' }} />
                </div>
                <span className="text-[12px]" style={{ color: 'var(--text-primary)' }}>GitHub</span>
              </div>
              <div className="flex items-center gap-1.5">
                <span className="text-[12px]" style={{ color: 'var(--text-tertiary)' }}>Chunyu33</span>
                <ExternalLink className="w-3 h-3" style={{ color: 'var(--text-tertiary)' }} />
              </div>
            </a>
            {/* B站/抖音同名 */}
            <div className="setting-item"
              style={{ padding: '10px 14px', borderBottom: '1px solid var(--border-color)' }}>
              <div className="flex items-center gap-3">
                <div className="w-8 h-8 rounded flex items-center justify-center" style={{ background: 'var(--bg-row-hover)' }}>
                  <Video className="w-4 h-4" style={{ color: 'var(--text-tertiary)' }} />
                </div>
                <span className="text-[12px]" style={{ color: 'var(--text-primary)' }}>B站/抖音同名</span>
              </div>
              <button
                onClick={() => handleCopy('Evan的像素空间', 'bilibili')}
                className="flex items-center gap-1.5 text-[12px] transition-colors cursor-pointer border-none bg-transparent"
                style={{ color: copiedLabel === 'bilibili' ? 'var(--color-primary)' : 'var(--text-tertiary)' }}
              >
                <span>Evan的像素空间</span>
                {copiedLabel === 'bilibili'
                  ? <Check className="w-3 h-3" style={{ color: 'var(--color-primary)' }} />
                  : <Copy className="w-3 h-3" />
                }
              </button>
            </div>
            {/* QQ交流群 */}
            <div className="setting-item"
              style={{ padding: '10px 14px', borderBottom: '1px solid var(--border-color)' }}>
              <div className="flex items-center gap-3">
                <div className="w-8 h-8 rounded flex items-center justify-center" style={{ background: 'var(--bg-row-hover)' }}>
                  <Users className="w-4 h-4" style={{ color: 'var(--text-tertiary)' }} />
                </div>
                <span className="text-[12px]" style={{ color: 'var(--text-primary)' }}>QQ交流群</span>
              </div>
              <button
                onClick={() => handleCopy('834582563', 'qq')}
                className="flex items-center gap-1.5 text-[12px] transition-colors cursor-pointer border-none bg-transparent"
                style={{ color: copiedLabel === 'qq' ? 'var(--color-primary)' : 'var(--text-tertiary)' }}
              >
                <span>834582563</span>
                {copiedLabel === 'qq'
                  ? <Check className="w-3 h-3" style={{ color: 'var(--color-primary)' }} />
                  : <Copy className="w-3 h-3" />
                }
              </button>
            </div>
            {/* 问题反馈 */}
            <button onClick={() => setFeedbackModalOpen(true)}
              className="setting-item setting-item-clickable w-full text-left"
              style={{ padding: '10px 14px', borderBottom: '1px solid var(--border-color)', cursor: 'pointer' }}>
              <div className="flex items-center gap-3">
                <div className="w-8 h-8 rounded flex items-center justify-center" style={{ background: 'var(--bg-row-hover)' }}>
                  <MessageSquare className="w-4 h-4" style={{ color: 'var(--text-tertiary)' }} />
                </div>
                <span className="text-[12px]" style={{ color: 'var(--text-primary)' }}>反馈/建议</span>
              </div>
              <ChevronRight className="w-3.5 h-3.5" style={{ color: 'var(--text-tertiary)' }} />
            </button>
            {/* 更多实用工具 */}
            <button
              onClick={() => setPromoModalOpen(true)}
              className="setting-item setting-item-clickable w-full text-left"
              style={{ padding: '10px 14px', borderBottom: '1px solid var(--border-color)', cursor: 'pointer' }}>
              <div className="flex items-center gap-3">
                <div className="w-8 h-8 rounded flex items-center justify-center" style={{ background: 'var(--color-primary-light)' }}>
                  <Rocket className="w-4 h-4" style={{ color: 'var(--color-primary)' }} />
                </div>
                <div>
                  <p className="setting-label">更多实用工具</p>
                  <p className="setting-desc">LightC × BinlockX — Windows 本地痛点一站式解决方案</p>
                </div>
              </div>
              <ChevronRight className="w-3.5 h-3.5" style={{ color: 'var(--text-tertiary)' }} />
            </button>
            {/* 支持作者 */}
            <button
              onClick={() => setDonateModalOpen(true)}
              className="setting-item setting-item-clickable w-full text-left"
              style={{ padding: '10px 14px', cursor: 'pointer' }}>
              <div className="flex items-center gap-3">
                <div className="w-8 h-8 rounded flex items-center justify-center" style={{ background: 'var(--color-danger-light)' }}>
                  <Heart className="w-4 h-4" style={{ color: 'var(--color-danger)' }} />
                </div>
                <div>
                  <p className="setting-label">支持作者</p>
                  <p className="setting-desc">如果 Viap 帮到了你，欢迎请我喝杯咖啡</p>
                </div>
              </div>
              <ChevronRight className="w-3.5 h-3.5" style={{ color: 'var(--text-tertiary)' }} />
            </button>
          </div>
        </section>

        {/* copyright */}
        <div className="text-center py-3 text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
          &copy; {currentYear} {APP_INFO.name} · All Right reserved.
        </div>
      </div>

      <UserManual isOpen={manualOpen} onClose={() => setManualOpen(false)} />
      <ProjectPromoModal isOpen={promoModalOpen} onClose={() => setPromoModalOpen(false)} />
      <DonateModal isOpen={donateModalOpen} onClose={() => setDonateModalOpen(false)} />
      <FeedbackModal
        isOpen={feedbackModalOpen}
        onClose={() => setFeedbackModalOpen(false)}
        email={APP_INFO.email}
        copiedLabel={copiedLabel}
        onCopy={handleCopy}
      />
      <Toast message={toast.message} type={toast.type} visible={toast.visible} onClose={hideToast} />
    </div>
  );
}
