// 卸载报告弹窗
//
// 卸载流程结束后给出"前后对比"：列表里已知的安装体积、本次实际释放的空间、
// 仍然残留的项目、以及需要用户自行处理的系统痕迹（服务/驱动/计划任务）。
// 报告文本可一键复制，方便用户留档或在反馈问题时提供现场信息。

import { useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { AlertTriangle, Check, ClipboardCopy, FolderOpen, GitCompare, HardDrive, Save, Server } from 'lucide-react';
import Modal from './Modal';
import type { SaveReportOutcome, SystemTrace, UninstallReportData } from '../types';

interface UninstallReportModalProps {
  isOpen: boolean;
  onClose: () => void;
  data: UninstallReportData | null;
}

function formatBytes(bytes: number): string {
  if (bytes <= 0) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  const index = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  return `${(bytes / 1024 ** index).toFixed(index === 0 ? 0 : 2)} ${units[index]}`;
}

/** 痕迹按类型分组统计，报告里只给数量与名称，避免刷屏 */
function summarizeTraces(traces: SystemTrace[]) {
  const groups: Record<string, SystemTrace[]> = { service: [], driver: [], task: [] };
  for (const trace of traces) {
    (groups[trace.kind] ??= []).push(trace);
  }
  return groups;
}

/**
 * 安装目录的真实体积变化
 *
 * 官方卸载器自己删除文件时我们拿不到"删了多少"，因此用卸载前后的体积差表示；
 * 没有快照（或安装目录不可用）时返回 null，由界面回退到列表中的估算值。
 */
function resolveDirectoryFootprint(data: UninstallReportData) {
  const diff = data.snapshotDiff;
  if (!diff?.has_snapshot || diff.install_dir_bytes_before <= 0) {
    return null;
  }
  const before = diff.install_dir_bytes_before;
  const after = diff.install_dir_bytes_after;
  return { before, after, released: Math.max(0, before - after) };
}

/** 组装可复制的纯文本报告 */
function buildReportText(data: UninstallReportData): string {
  const footprint = resolveDirectoryFootprint(data);
  const lines: string[] = [
    `Viap 卸载报告`,
    `应用：${data.appName}`,
    `安装目录：${data.installLocation || '未知'}`,
  ];

  if (footprint) {
    lines.push(
      `安装目录：卸载前 ${formatBytes(footprint.before)} → 现在 ${formatBytes(footprint.after)}`
        + `（已释放 ${formatBytes(footprint.released)}）`,
    );
  } else {
    lines.push(`预计释放（安装目录）：${formatBytes(data.estimatedBytes)}`);
  }
  if (data.cleanupFreedBytes > 0) {
    lines.push(
      `残留清理另删除：${formatBytes(data.cleanupFreedBytes)}`
        + `（位于安装目录内的部分已计入上面的差值，不重复累加）`,
    );
  }

  if (data.storePackage) {
    lines.push(`MS Store 包：${data.storePackage.package_full_name}`);
  }
  if (data.scheduledForReboot.length > 0) {
    lines.push(`重启后自动删除：${data.scheduledForReboot.length} 项`);
    lines.push(...data.scheduledForReboot.map((item) => `  · ${item}`));
  }
  if (data.failedItems.length > 0) {
    lines.push(`未能删除：${data.failedItems.length} 项`);
    lines.push(...data.failedItems.map((item) => `  · ${item}`));
  }

  if (data.snapshotDiff?.has_snapshot) {
    const { appeared, disappeared, remaining } = data.snapshotDiff;
    lines.push('卸载前后对比：');
    lines.push(`  消失 ${disappeared.length} 项（被卸载器删除）`);
    lines.push(`  新增 ${appeared.length} 项（卸载期间出现，需人工确认归属）`);
    lines.push(`  仍在 ${remaining.length} 项（卸载前后都在，可能是历史残留或其它应用）`);
    const describe = (entries: typeof appeared) => entries
      .slice(0, 15)
      .map((entry) => `    · [${entry.group}] ${entry.name}${entry.confidence === 'uncertain' ? '（归属待确认）' : ''}`);
    if (appeared.length > 0) lines.push(...describe(appeared));
    if (disappeared.length > 0) lines.push(...describe(disappeared));
  }

  const groups = summarizeTraces(data.systemTraces);
  if (data.systemTraces.length > 0) {
    lines.push('系统痕迹（需手动处理，Viap 不会自动删除）：');
    if (groups.service.length > 0) {
      lines.push(`  服务 ${groups.service.length} 个：${groups.service.map(t => t.name).join('、')}`);
    }
    if (groups.driver.length > 0) {
      lines.push(`  驱动 ${groups.driver.length} 个：${groups.driver.map(t => t.name).join('、')}`);
    }
    if (groups.task.length > 0) {
      lines.push(`  计划任务 ${groups.task.length} 个：${groups.task.map(t => t.name).join('、')}`);
    }
  } else {
    lines.push('系统痕迹：未检测到服务 / 驱动 / 计划任务');
  }

  return lines.join('\n');
}

export default function UninstallReportModal({ isOpen, onClose, data }: UninstallReportModalProps) {
  const [copied, setCopied] = useState(false);
  const [saving, setSaving] = useState(false);
  const [savedPath, setSavedPath] = useState('');
  const [saveError, setSaveError] = useState('');
  const reportText = useMemo(() => (data ? buildReportText(data) : ''), [data]);
  const groups = useMemo(() => summarizeTraces(data?.systemTraces ?? []), [data]);
  const diff = data?.snapshotDiff ?? null;
  const footprint = data ? resolveDirectoryFootprint(data) : null;

  /** 保存报告：把快照与差异一并写入数据目录，便于留档或反馈时附上 */
  async function handleSave() {
    if (!data) return;
    setSaving(true);
    setSaveError('');
    try {
      const outcome = await invoke<SaveReportOutcome>('save_uninstall_report', {
        input: {
          app_name: data.appName,
          install_location: data.installLocation,
          estimated_bytes: data.estimatedBytes,
          uninstall_freed_bytes: data.uninstallFreedBytes,
          cleanup_freed_bytes: data.cleanupFreedBytes,
          failed_items: data.failedItems,
          scheduled_for_reboot: data.scheduledForReboot,
        },
      });
      setSavedPath(outcome.path);
    } catch (error) {
      setSaveError(String(error));
    } finally {
      setSaving(false);
    }
  }

  async function handleCopy() {
    try {
      await navigator.clipboard.writeText(reportText);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1500);
    } catch {
      // 剪贴板不可用时保持按钮原状，用户仍可手动选中文本
    }
  }

  return (
    <Modal isOpen={isOpen} onClose={onClose} title="卸载报告" width={560}>
      {data && (
        <div className="flex flex-col gap-3 text-[12px]">
          <div className="rounded border px-3 py-2" style={{ borderColor: 'var(--border-color)', background: 'var(--bg-row)' }}>
            <p style={{ color: 'var(--text-primary)' }}>{data.appName}</p>
            <p className="mt-1 break-all" style={{ color: 'var(--text-tertiary)' }}>{data.installLocation || '未知安装目录'}</p>
          </div>

          <div className="grid grid-cols-3 gap-2">
            <div className="rounded border px-3 py-2" style={{ borderColor: 'var(--border-color)' }}>
              <p className="flex items-center gap-1" style={{ color: 'var(--text-tertiary)' }}>
                <HardDrive className="w-3 h-3" />卸载前占用
              </p>
              <p className="mt-1 text-[14px] font-semibold" style={{ color: 'var(--text-primary)' }}>
                {formatBytes(footprint ? footprint.before : data.estimatedBytes)}
              </p>
            </div>
            <div className="rounded border px-3 py-2" style={{ borderColor: 'var(--border-color)' }}>
              <p style={{ color: 'var(--text-tertiary)' }}>现在占用</p>
              <p className="mt-1 text-[14px] font-semibold" style={{ color: 'var(--text-primary)' }}>
                {footprint ? formatBytes(footprint.after) : '未知'}
              </p>
            </div>
            <div className="rounded border px-3 py-2" style={{ borderColor: 'var(--border-color)' }}>
              <p style={{ color: 'var(--text-tertiary)' }}>已释放</p>
              <p className="mt-1 text-[14px] font-semibold" style={{ color: 'var(--color-success)' }}>
                {formatBytes(footprint ? footprint.released : data.uninstallFreedBytes + data.cleanupFreedBytes)}
              </p>
            </div>
          </div>

          {/* 清理删除量单独说明：安装目录内的部分已经体现在上面的差值里，不能重复相加 */}
          {data.cleanupFreedBytes > 0 && (
            <p className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
              残留清理另删除 {formatBytes(data.cleanupFreedBytes)}（位于安装目录内的部分已计入差值）
            </p>
          )}

          {data.storePackage && (
            <p className="rounded px-3 py-2" style={{ background: 'var(--color-primary-light)', color: 'var(--text-secondary)' }}>
              MS Store 应用：{data.storePackage.package_full_name}
            </p>
          )}

          {data.scheduledForReboot.length > 0 && (
            <p className="rounded px-3 py-2" style={{ background: 'var(--color-warning-light)', color: 'var(--color-warning)' }}>
              <AlertTriangle className="w-3.5 h-3.5 inline mr-1" />
              {data.scheduledForReboot.length} 项文件被占用，已安排在下次重启时自动删除。
            </p>
          )}

          {data.failedItems.length > 0 && (
            <div className="rounded px-3 py-2" style={{ background: 'var(--color-danger-light)', color: 'var(--color-danger)' }}>
              <p>{data.failedItems.length} 项未能删除（可尝试以管理员身份重试）：</p>
              <div className="mt-1 max-h-[90px] overflow-y-auto text-[11px]">
                {data.failedItems.map((item, index) => <div key={index} className="break-all">· {item}</div>)}
              </div>
            </div>
          )}

          {diff?.has_snapshot && (
            <div className="rounded border px-3 py-2" style={{ borderColor: 'var(--border-color)' }}>
              <p className="flex items-center gap-1" style={{ color: 'var(--text-secondary)' }}>
                <GitCompare className="w-3 h-3" />卸载前后对比
              </p>
              <div className="mt-1 grid grid-cols-3 gap-2 text-[11px]">
                <span style={{ color: 'var(--color-success)' }}>消失 {diff.disappeared.length}</span>
                <span style={{ color: 'var(--color-warning)' }}>新增 {diff.appeared.length}</span>
                <span style={{ color: 'var(--text-tertiary)' }}>仍在 {diff.remaining.length}</span>
              </div>

              {diff.appeared.length > 0 && (
                <div className="mt-2 text-[11px]">
                  <p style={{ color: 'var(--text-secondary)' }}>卸载期间新出现（可作残留线索，归属需人工确认）：</p>
                  <div className="mt-1 max-h-[90px] overflow-y-auto" style={{ color: 'var(--text-tertiary)' }}>
                    {diff.appeared.slice(0, 20).map((entry) => (
                      <div key={`${entry.group}-${entry.name}`} className="break-all">
                        · [{entry.group}] {entry.name}
                        {entry.confidence === 'uncertain' && <span>（归属待确认）</span>}
                      </div>
                    ))}
                  </div>
                </div>
              )}

              {diff.disappeared.length > 0 && (
                <details className="mt-2 text-[11px]">
                  <summary className="cursor-pointer" style={{ color: 'var(--text-tertiary)' }}>
                    查看被删除的 {diff.disappeared.length} 项
                  </summary>
                  <div className="mt-1 max-h-[90px] overflow-y-auto" style={{ color: 'var(--text-tertiary)' }}>
                    {diff.disappeared.slice(0, 30).map((entry) => (
                      <div key={`${entry.group}-${entry.name}`} className="break-all">· [{entry.group}] {entry.name}</div>
                    ))}
                  </div>
                </details>
              )}

              {diff.appeared.length === 0 && diff.disappeared.length === 0 && (
                <p className="mt-1 text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
                  标准位置与安装目录均无变化
                </p>
              )}
            </div>
          )}

          <div className="rounded border px-3 py-2" style={{ borderColor: 'var(--border-color)' }}>
            <p className="flex items-center gap-1" style={{ color: 'var(--text-secondary)' }}>
              <Server className="w-3 h-3" />系统痕迹（需手动处理）
            </p>
            {data.systemTraces.length === 0 ? (
              <p className="mt-1" style={{ color: 'var(--text-tertiary)' }}>未检测到服务 / 驱动 / 计划任务</p>
            ) : (
              <div className="mt-1 flex flex-col gap-1">
                {(['service', 'driver', 'task'] as const).map((kind) => {
                  const items = groups[kind];
                  if (!items || items.length === 0) return null;
                  const label = kind === 'service' ? '服务' : kind === 'driver' ? '驱动' : '计划任务';
                  return (
                    <div key={kind}>
                      <span style={{ color: 'var(--text-secondary)' }}>{label} {items.length} 个：</span>
                      <span className="break-all" style={{ color: 'var(--text-tertiary)' }}>
                        {items.map(trace => trace.name).join('、')}
                      </span>
                    </div>
                  );
                })}
              </div>
            )}
          </div>

          <details>
            <summary className="cursor-pointer text-[11px]" style={{ color: 'var(--text-tertiary)' }}>查看可复制报告</summary>
            <pre
              className="mt-2 max-h-[160px] overflow-auto rounded px-2 py-2 text-[11px] whitespace-pre-wrap break-all"
              style={{ background: 'var(--bg-row)', color: 'var(--text-secondary)' }}
            >
              {reportText}
            </pre>
          </details>

          {savedPath && (
            <p className="rounded px-3 py-2 text-[11px] break-all" style={{ background: 'var(--color-success-light)', color: 'var(--color-success)' }}>
              报告已保存：{savedPath}
            </p>
          )}
          {saveError && (
            <p className="rounded px-3 py-2 text-[11px] break-all" style={{ background: 'var(--color-danger-light)', color: 'var(--color-danger)' }}>
              保存报告失败：{saveError}
            </p>
          )}

          <div className="flex items-center justify-end gap-2 pt-1">
            <button className="btn h-8 text-[12px]" onClick={handleCopy}>
              {copied ? <Check className="w-3.5 h-3.5" /> : <ClipboardCopy className="w-3.5 h-3.5" />}
              {copied ? '已复制' : '复制报告'}
            </button>
            <button className="btn h-8 text-[12px]" onClick={handleSave} disabled={saving}>
              {saving ? <FolderOpen className="w-3.5 h-3.5" /> : <Save className="w-3.5 h-3.5" />}
              {saving ? '保存中...' : '保存报告'}
            </button>
            <button className="btn btn-primary h-8 text-[12px]" onClick={onClose}>完成</button>
          </div>
        </div>
      )}
    </Modal>
  );
}
