// 迁移记录重建弹窗
//
// 用途：数据目录被误删导致迁移历史丢失后，从**原路径侧**的目录联接反推并重建迁移记录。
// 交互：用户手动触发（懒加载：应用启动、页面进入都不做任何检查）→ 选目录 → 扫描 →
// 人工确认 → 导入。联接无法证明「由 Viap 创建」，因此候选必须由用户确认，不做静默导入。

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { open } from '@tauri-apps/plugin-dialog';
import {
  AlertTriangle, Check, CheckCircle2, FolderSearch, HardDriveDownload,
  Info, Link2, LoaderCircle, RefreshCw, X,
} from 'lucide-react';
import type {
  LinkRecoveryImportResult, LinkRecoveryProgressEvent, LinkRecoveryScanResult,
  MigrationRecordType, MirrorBackupInfo, MirrorImportResult, RecoveredLinkEntry,
} from '../types';
import Checkbox from './Checkbox';
import { logger } from '../utils/logger';

interface LinkRecoveryModalProps {
  isOpen: boolean;
  onClose: () => void;
  /** 导入成功后回调：父页面据此刷新迁移记录与目录列表 */
  onImported: () => void;
}

/** 可选的扫描深度：默认 4 覆盖 C:\Users\<用户>\AppData\Local\Programs\<应用> */
const SCAN_DEPTH_OPTIONS = [1, 2, 3, 4, 5, 6];

/** 提示条：区分错误与中性说明，避免把用户主动取消渲染成失败 */
interface Feedback {
  tone: 'error' | 'info';
  text: string;
}

/** 默认勾选规则：高置信 + 目标有数据 + 尚未记录 */
function isDefaultSelected(entry: RecoveredLinkEntry): boolean {
  return entry.confidence === 'high' && entry.target_exists && !entry.target_empty && !entry.already_recorded;
}

function confidenceLabel(confidence: RecoveredLinkEntry['confidence']): { text: string; color: string; background: string } {
  if (confidence === 'high') {
    return { text: '可直接导入', color: 'var(--color-success)', background: 'var(--color-success-light)' };
  }
  if (confidence === 'medium') {
    return { text: '需确认', color: 'var(--color-warning)', background: 'var(--color-warning-light)' };
  }
  return { text: '存疑', color: 'var(--text-tertiary)', background: 'var(--bg-row-hover)' };
}

function formatSize(bytes: number): string {
  if (bytes <= 0) return '未统计';
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}

function formatTime(millis: number): string {
  if (millis <= 0) return '时间未知';
  return new Date(millis).toLocaleString('zh-CN', {
    year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit',
  });
}

export default function LinkRecoveryModal({ isOpen, onClose, onImported }: LinkRecoveryModalProps) {
  const [rootPath, setRootPath] = useState('');
  const [maxDepth, setMaxDepth] = useState(4);
  const [recordSize, setRecordSize] = useState(false);
  const [scanning, setScanning] = useState(false);
  const [scanResult, setScanResult] = useState<LinkRecoveryScanResult | null>(null);
  const [progress, setProgress] = useState<LinkRecoveryProgressEvent | null>(null);
  const [selectedPaths, setSelectedPaths] = useState<Set<string>>(new Set());
  /** 用户对单个条目的类型修正（默认取扫描推测值） */
  const [typeOverrides, setTypeOverrides] = useState<Record<string, MigrationRecordType>>({});
  const [registerFolders, setRegisterFolders] = useState(true);
  const [importing, setImporting] = useState(false);
  const [importResult, setImportResult] = useState<LinkRecoveryImportResult | null>(null);
  const [feedback, setFeedback] = useState<Feedback | null>(null);
  const [mirrorInfo, setMirrorInfo] = useState<MirrorBackupInfo | null>(null);
  const [mirrorImporting, setMirrorImporting] = useState(false);
  const progressUnlistenRef = useRef<UnlistenFn | null>(null);

  const stopProgressListener = useCallback(() => {
    progressUnlistenRef.current?.();
    progressUnlistenRef.current = null;
  }, []);

  // 组件卸载时务必解绑事件，避免扫描进度回调打到已卸载组件
  useEffect(() => () => stopProgressListener(), [stopProgressListener]);

  const loadMirrorInfo = useCallback(async () => {
    try {
      setMirrorInfo(await invoke<MirrorBackupInfo>('get_mirror_backup_info'));
    } catch (error) {
      logger.error('读取自动备份信息失败:', error);
      setMirrorInfo(null);
    }
  }, []);

  useEffect(() => {
    if (!isOpen) return;
    setImportResult(null);
    setFeedback(null);
    loadMirrorInfo();
  }, [isOpen, loadMirrorInfo]);

  async function handlePickDirectory() {
    try {
      const picked = await open({
        directory: true,
        multiple: false,
        title: '选择原路径所在文件夹（例如 C:\\Users\\你的用户名）',
      });
      if (!picked || typeof picked !== 'string') return;
      setRootPath(picked);
      // 换了目录，上一次的扫描结果不再适用
      setScanResult(null);
      setSelectedPaths(new Set());
      setImportResult(null);
      setFeedback(null);
    } catch (error) {
      setFeedback({ tone: 'error', text: `选择目录失败：${String(error)}` });
    }
  }

  async function handleScan() {
    if (!rootPath) {
      setFeedback({ tone: 'error', text: '请先选择要扫描的文件夹（原路径一侧）' });
      return;
    }

    setScanning(true);
    setFeedback(null);
    setImportResult(null);
    setProgress(null);
    setScanResult(null);

    try {
      // 监听器必须先就绪再触发扫描，否则首帧进度事件会丢失
      stopProgressListener();
      progressUnlistenRef.current = await listen<LinkRecoveryProgressEvent>(
        'link-recovery-progress',
        (event) => setProgress(event.payload),
      );

      const result = await invoke<LinkRecoveryScanResult>('scan_migration_links', {
        rootPath,
        maxDepth,
        recordSize,
      });

      setScanResult(result);
      setSelectedPaths(new Set(result.entries.filter(isDefaultSelected).map((entry) => entry.original_path)));
      setTypeOverrides({});
    } catch (error) {
      const message = String(error);
      // 主动取消不是错误，用中性提示呈现
      setFeedback({ tone: message.includes('取消') ? 'info' : 'error', text: message });
    } finally {
      stopProgressListener();
      setScanning(false);
    }
  }

  async function handleCancelScan() {
    try {
      await invoke('cancel_link_recovery');
    } catch (error) {
      logger.error('取消链接识别失败:', error);
    }
  }

  function toggleEntry(originalPath: string) {
    setSelectedPaths((previous) => {
      const next = new Set(previous);
      if (next.has(originalPath)) next.delete(originalPath);
      else next.add(originalPath);
      return next;
    });
  }

  function selectableEntries(): RecoveredLinkEntry[] {
    return (scanResult?.entries ?? []).filter((entry) => !entry.already_recorded);
  }

  function handleToggleAll() {
    const selectable = selectableEntries();
    const allSelected = selectable.length > 0 && selectable.every((entry) => selectedPaths.has(entry.original_path));
    setSelectedPaths(allSelected ? new Set() : new Set(selectable.map((entry) => entry.original_path)));
  }

  function handleTypeChange(originalPath: string, recordType: MigrationRecordType) {
    setTypeOverrides((previous) => ({ ...previous, [originalPath]: recordType }));
  }

  function resolveRecordType(entry: RecoveredLinkEntry, overrides: Record<string, MigrationRecordType>): MigrationRecordType {
    return overrides[entry.original_path] ?? entry.record_type;
  }

  async function handleImport() {
    const entries = scanResult?.entries ?? [];
    const payload = entries
      .filter((entry) => selectedPaths.has(entry.original_path))
      .map((entry) => ({
        app_name: entry.app_name,
        original_path: entry.original_path,
        target_path: entry.target_path,
        record_type: resolveRecordType(entry, typeOverrides),
        migrated_at: entry.migrated_at,
        size: entry.size,
        register_custom_folder: registerFolders,
      }));

    if (payload.length === 0) {
      setFeedback({ tone: 'error', text: '请至少选择一条要恢复的记录' });
      return;
    }

    setImporting(true);
    setFeedback(null);
    try {
      const result = await invoke<LinkRecoveryImportResult>('import_recovered_links', { entries: payload });
      setImportResult(result);

      // 已写入的条目在下一次扫描前不应被重复导入，直接就地标记为已存在
      const importedPaths = new Set(
        payload
          .filter((item) => !result.failed.some((reason) => reason.startsWith(item.original_path)))
          .map((item) => item.original_path),
      );
      setScanResult((previous) => previous && {
        ...previous,
        entries: previous.entries.map((entry) => (
          importedPaths.has(entry.original_path) ? { ...entry, already_recorded: true } : entry
        )),
      });
      setSelectedPaths(new Set());
      setFeedback({
        tone: 'info',
        text: `已重建 ${result.imported} 条迁移记录`
          + (result.duplicated > 0 ? `，${result.duplicated} 条已存在未重复写入` : '')
          + (result.rejected > 0 ? `，${result.rejected} 条未通过校验` : '')
          + (result.custom_folders_added > 0 ? `，登记 ${result.custom_folders_added} 个自定义文件夹` : ''),
      });
      if (result.imported > 0 || result.custom_folders_added > 0) onImported();
      // 重建的记录无法从联接得知体积，交给后端在后台补全并通过事件刷新界面
      if (result.imported > 0) {
        invoke<number>('start_recovered_size_scan').catch((error) => {
          logger.error('启动记录大小补全失败:', error);
        });
      }
      loadMirrorInfo();
    } catch (error) {
      setFeedback({ tone: 'error', text: `导入失败：${String(error)}` });
    } finally {
      setImporting(false);
    }
  }

  async function handleMirrorImport() {
    setMirrorImporting(true);
    setFeedback(null);
    try {
      const result = await invoke<MirrorImportResult>('import_mirror_backup');
      setFeedback({
        tone: 'info',
        text: `备份导入完成：迁移记录 +${result.history_added}（跳过 ${result.history_skipped}）`
          + `，自定义文件夹 +${result.custom_folders_added}，兜底元数据 +${result.migrated_apps_added}`,
      });
      setImportResult(null);
      if (result.history_added > 0 || result.custom_folders_added > 0) onImported();
      loadMirrorInfo();
    } catch (error) {
      setFeedback({ tone: 'error', text: `备份导入失败：${String(error)}` });
    } finally {
      setMirrorImporting(false);
    }
  }

  const entries = scanResult?.entries ?? [];
  const selectableCount = useMemo(
    () => entries.filter((entry) => !entry.already_recorded).length,
    [entries],
  );
  const recordedCount = entries.length - selectableCount;
  const selectedCount = selectedPaths.size;
  const allSelectableSelected = selectableCount > 0 && selectedCount === selectableCount;

  if (!isOpen) return null;

  return (
    <div className="fixed inset-0 z-50 grid place-items-center p-4">
      <div
        className="absolute inset-0"
        style={{
          background: 'linear-gradient(180deg, rgba(15,23,42,0.42), rgba(2,6,23,0.62))',
          backdropFilter: 'blur(12px)',
        }}
        onClick={scanning || importing ? undefined : onClose}
      />

      <div
        className="relative w-full overflow-hidden rounded-xl shadow-2xl animate-modal-in flex flex-col"
        style={{
          maxWidth: '780px',
          maxHeight: 'min(680px, calc(100vh - 64px))',
          background: 'var(--bg-modal)',
          border: '1px solid var(--border-color)',
        }}
      >
        {/* 头部 */}
        <div className="flex items-start justify-between px-5 pt-4 pb-3 flex-shrink-0" style={{ borderBottom: '1px solid var(--border-color)' }}>
          <div className="pr-4 min-w-0">
            <h2 className="text-base font-semibold" style={{ color: 'var(--text-primary)' }}>
              恢复迁移记录
            </h2>
            <p className="text-xs mt-1" style={{ color: 'var(--text-secondary)' }}>
              扫描目录联接并重建丢失的迁移记录；也可从本地自动备份一键导入
            </p>
          </div>
          <button
            onClick={onClose}
            disabled={scanning || importing}
            className="btn btn-ghost btn-icon flex-shrink-0"
            aria-label="关闭弹窗"
          >
            <X className="w-4 h-4" />
          </button>
        </div>

        {/* 体部 */}
        <div className="flex-1 overflow-y-auto px-5 py-4 min-h-0">
          {/* 自动备份入口：数据目录被误删时最省事的一条路 */}
          {mirrorInfo?.exists && (
            <div
              className="rounded-lg border px-3 py-2.5 mb-3"
              style={{ borderColor: 'var(--border-color)', background: 'var(--bg-row)' }}
            >
              <div className="flex items-center justify-between gap-3">
                <div className="min-w-0">
                  <p className="text-[12px] font-medium flex items-center gap-1.5" style={{ color: 'var(--text-primary)' }}>
                    <HardDriveDownload className="w-3.5 h-3.5" />
                    {mirrorInfo.auto_backup_enabled ? '发现本地自动备份' : '本地自动备份（已关闭）'}
                  </p>
                  <p className="text-[11px] mt-1 truncate" style={{ color: 'var(--text-tertiary)' }} title={mirrorInfo.path}>
                    程序每次保存迁移数据时自动留的副本（不在数据目录内）· {mirrorInfo.history_count} 条迁移记录
                    · {mirrorInfo.custom_folder_count} 个自定义文件夹 · {mirrorInfo.migrated_app_count} 条应用兜底数据
                    · 备份于 {formatTime(mirrorInfo.saved_at)}
                  </p>
                </div>
                <button
                  onClick={handleMirrorImport}
                  disabled={mirrorImporting || scanning || importing}
                  className="btn btn-sm h-7 text-[11px] flex-shrink-0"
                >
                  {mirrorImporting ? <LoaderCircle className="w-3.5 h-3.5 animate-spin" /> : <RefreshCw className="w-3.5 h-3.5" />}
                  从备份导入
                </button>
              </div>
            </div>
          )}

          {/* 目录选择与扫描参数 */}
          <div className="rounded-lg border px-3 py-3" style={{ borderColor: 'var(--border-color)' }}>
            <div className="flex items-center gap-2">
              <div
                className="flex-1 min-w-0 rounded px-2.5 h-8 flex items-center text-[12px] truncate"
                style={{
                  background: 'var(--bg-input, var(--bg-row))',
                  border: '1px solid var(--border-color)',
                  color: rootPath ? 'var(--text-primary)' : 'var(--text-tertiary)',
                }}
                title={rootPath}
              >
                {rootPath || '尚未选择文件夹'}
              </div>
              <button onClick={handlePickDirectory} disabled={scanning} className="btn h-8 text-[12px] flex-shrink-0">
                <FolderSearch className="w-3.5 h-3.5" />
                选择文件夹
              </button>
              <button onClick={handleScan} disabled={scanning || !rootPath} className="btn btn-primary h-8 text-[12px] flex-shrink-0">
                {scanning ? <LoaderCircle className="w-3.5 h-3.5 animate-spin" /> : <Link2 className="w-3.5 h-3.5" />}
                {scanning ? '扫描中...' : '开始扫描'}
              </button>
            </div>

            <div className="flex items-center gap-3 mt-2 flex-wrap">
              <label className="flex items-center gap-1.5 text-[11px]" style={{ color: 'var(--text-secondary)' }}>
                扫描深度
                <select
                  value={maxDepth}
                  onChange={(event) => setMaxDepth(Number(event.target.value))}
                  disabled={scanning}
                  className="h-6 rounded px-1 text-[11px]"
                  style={{ background: 'var(--bg-row)', border: '1px solid var(--border-color)', color: 'var(--text-primary)' }}
                >
                  {SCAN_DEPTH_OPTIONS.map((depth) => (
                    <option key={depth} value={depth}>{depth} 层</option>
                  ))}
                </select>
              </label>
              <label className="flex items-center gap-1.5 text-[11px] cursor-pointer" style={{ color: 'var(--text-secondary)' }}>
                <Checkbox
                  size="sm"
                  checked={recordSize}
                  onChange={setRecordSize}
                  disabled={scanning}
                />
                统计目标目录大小（较慢，机械盘慎用）
              </label>
              <label className="flex items-center gap-1.5 text-[11px] cursor-pointer" style={{ color: 'var(--text-secondary)' }}>
                <Checkbox
                  size="sm"
                  checked={registerFolders}
                  onChange={setRegisterFolders}
                />
                把文件夹类目录登记为自定义文件夹
              </label>
            </div>

            <p className="text-[11px] mt-2 flex items-start gap-1.5" style={{ color: 'var(--text-tertiary)' }}>
              <Info className="w-3 h-3 mt-0.5 flex-shrink-0" />
              请选择「原路径」所在文件夹（如 C:\Users\你的用户名）。目录联接建在原路径上，
              选择迁移目标目录无法反推原路径；找不到预期条目时可提高扫描深度，
              或直接选择更靠近目标的父目录（如 AppData\Local\Programs）。
            </p>
          </div>

          {/* 扫描进度 */}
          {scanning && (
            <div className="mt-3 rounded-lg border px-3 py-3 flex items-center gap-3" style={{ borderColor: 'var(--border-color)' }}>
              <LoaderCircle className="w-4 h-4 animate-spin flex-shrink-0" style={{ color: 'var(--color-primary)' }} />
              <div className="flex-1 min-w-0">
                <p className="text-[12px]" style={{ color: 'var(--text-secondary)' }}>
                  已扫描 {progress?.scanned_dirs ?? 0} 个目录，发现 {progress?.found_links ?? 0} 个联接
                </p>
                <p className="text-[11px] truncate mt-0.5" style={{ color: 'var(--text-tertiary)' }} title={progress?.current_path}>
                  {progress?.current_path ?? '正在准备...'}
                </p>
              </div>
              <button onClick={handleCancelScan} className="btn btn-sm h-7 text-[11px] flex-shrink-0">取消</button>
            </div>
          )}

          {/* 反馈 */}
          {feedback && (
            <div
              className="mt-3 rounded-lg px-3 py-2 text-[12px]"
              style={{
                background: feedback.tone === 'error' ? 'var(--color-danger-light)' : 'var(--color-success-light)',
                color: feedback.tone === 'error' ? 'var(--color-danger)' : 'var(--color-success)',
              }}
            >
              {feedback.text}
            </div>
          )}

          {importResult && importResult.failed.length > 0 && (
            <div className="mt-2 rounded-lg px-3 py-2 text-[11px]" style={{ background: 'var(--color-warning-light)', color: 'var(--color-warning)' }}>
              {importResult.failed.map((reason, index) => <div key={index}>{reason}</div>)}
            </div>
          )}

          {/* 结果列表 */}
          {scanResult && !scanning && (
            <div className="mt-3">
              <div className="flex items-center justify-between gap-2 mb-2">
                <span className="text-[12px]" style={{ color: 'var(--text-secondary)' }}>
                  发现 <strong style={{ color: 'var(--text-primary)' }}>{entries.length}</strong> 个联接候选
                  {recordedCount > 0 && (
                    <span className="ml-2" style={{ color: 'var(--text-tertiary)' }}>
                      其中 {recordedCount} 条已有记录
                    </span>
                  )}
                  <span className="ml-2" style={{ color: 'var(--text-tertiary)' }}>
                    扫描 {scanResult.scanned_dirs} 个目录 · 跳过 {scanResult.skipped_dirs} 个 · 耗时 {scanResult.elapsed_ms} ms
                  </span>
                </span>
                {selectableCount > 0 && (
                  <button
                    onClick={handleToggleAll}
                    className="text-[11px] cursor-pointer"
                    style={{ color: 'var(--color-primary)', background: 'none', border: 'none' }}
                  >
                    {allSelectableSelected ? '取消全选' : '全选可导入项'}
                  </button>
                )}
              </div>

              {scanResult.truncated && (
                <div className="rounded px-3 py-2 text-[11px] mb-2 flex items-center gap-1.5" style={{ background: 'var(--color-warning-light)', color: 'var(--color-warning)' }}>
                  <AlertTriangle className="w-3 h-3" />
                  已达到扫描上限并提前结束，请缩小目录范围（例如只扫 C:\Users\你的用户名）后重试
                </div>
              )}

              {entries.length > 0 && selectableCount === 0 && (
                <div
                  className="rounded px-3 py-2 text-[11px] mb-2 flex items-center gap-1.5"
                  style={{ background: 'var(--color-success-light)', color: 'var(--color-success)' }}
                >
                  <CheckCircle2 className="w-3 h-3" />
                  该目录下的联接都已有迁移记录，无需重复恢复
                </div>
              )}

              {entries.length === 0 ? (
                <div className="py-8 text-center text-[12px]" style={{ color: 'var(--text-tertiary)' }}>
                  该目录下未发现可作为迁移记录的目录联接
                </div>
              ) : (
                <div className="flex flex-col gap-1.5">
                  {entries.map((entry) => {
                    const badge = confidenceLabel(entry.confidence);
                    const checked = selectedPaths.has(entry.original_path);
                    const disabled = entry.already_recorded;
                    return (
                      <label
                        key={entry.original_path}
                        className={`flex items-start rounded-lg border px-3 py-2 transition-all ${disabled ? 'cursor-default opacity-70' : 'cursor-pointer'}`}
                        style={{
                          borderColor: checked ? 'var(--color-primary)' : 'var(--border-color)',
                          background: checked ? 'var(--color-primary-light)' : 'var(--bg-row)',
                        }}
                      >
                        <div className="flex-shrink-0 mt-0.5">
                          <Checkbox
                            checked={checked}
                            disabled={disabled}
                            onChange={() => toggleEntry(entry.original_path)}
                            ariaLabel={`${checked ? '取消选择' : '选择'} ${entry.original_path}`}
                          />
                        </div>

                        <div className="min-w-0 flex-1 ml-3">
                          <div className="flex items-center gap-2 flex-wrap">
                            <span className="text-[12px] font-medium" style={{ color: 'var(--text-primary)' }}>
                              {entry.app_name}
                            </span>
                            <select
                              value={resolveRecordType(entry, typeOverrides)}
                              onChange={(event) => handleTypeChange(entry.original_path, event.target.value as MigrationRecordType)}
                              disabled={disabled}
                              className="h-6 rounded px-1 text-[11px]"
                              style={{ background: 'var(--bg-modal)', border: '1px solid var(--border-color)', color: 'var(--text-secondary)' }}
                            >
                              <option value="App">应用</option>
                              <option value="LargeFolder">文件夹</option>
                            </select>
                            <span className="badge" style={{ fontSize: '10px', color: badge.color, background: badge.background }}>
                              {badge.text}
                            </span>
                            {entry.already_recorded && (
                              <span className="badge" style={{ fontSize: '10px', color: 'var(--text-tertiary)', background: 'var(--bg-row-hover)' }}>
                                已有记录
                              </span>
                            )}
                            {entry.cross_drive && (
                              <span className="badge" style={{ fontSize: '10px', color: 'var(--text-tertiary)', background: 'var(--bg-row-hover)' }}>
                                跨盘
                              </span>
                            )}
                            <span className="ml-auto text-[11px] flex-shrink-0" style={{ color: 'var(--text-tertiary)' }}>
                              {formatSize(entry.size)} · {formatTime(entry.migrated_at)}
                            </span>
                          </div>

                          <p className="text-[11px] mt-1 truncate" style={{ color: 'var(--text-secondary)' }} title={entry.original_path}>
                            {entry.original_path}
                          </p>
                          <p className="text-[11px] truncate" style={{ color: 'var(--text-tertiary)' }} title={entry.target_path}>
                            → {entry.target_path}
                          </p>

                          {entry.warnings.length > 0 && (
                            <div className="mt-1 text-[11px]" style={{ color: 'var(--color-warning)' }}>
                              {entry.warnings.map((warning, index) => <div key={index}>· {warning}</div>)}
                            </div>
                          )}
                        </div>
                      </label>
                    );
                  })}
                </div>
              )}
            </div>
          )}
        </div>

        {/* 底部操作栏 */}
        <div
          className="flex items-center justify-between px-5 py-3 flex-shrink-0"
          style={{
            borderTop: '1px solid var(--border-color)',
            background: 'color-mix(in srgb, var(--color-gray-50) 74%, transparent)',
          }}
        >
          <span className="text-[12px]" style={{ color: 'var(--text-secondary)' }}>
            {scanResult ? `已选 ${selectedCount}/${entries.length}` : '尚未扫描'}
          </span>
          <div className="flex items-center gap-2">
            <button className="btn btn-sm" onClick={onClose} disabled={scanning || importing}>关闭</button>
            <button
              onClick={handleImport}
              disabled={importing || scanning || selectedCount === 0}
              className="btn btn-sm btn-primary"
            >
              {importing ? <LoaderCircle className="w-3.5 h-3.5 animate-spin" /> : <Check className="w-3.5 h-3.5" />}
              {importing ? '导入中...' : `导入选中 (${selectedCount})`}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
