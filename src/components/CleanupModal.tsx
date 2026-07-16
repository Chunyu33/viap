// 残留清理弹窗组件
// 极简风格：半透明背景 + 高对比风险操作按钮

import { useMemo } from 'react';
import { AlertTriangle, Check, LoaderCircle, ScanSearch, Trash2, X } from 'lucide-react';
import { LeftoverItem } from '../types';

interface CleanupModalProps {
  isOpen: boolean;
  appName: string;
  items: LeftoverItem[];
  loading: boolean;
  scanning: boolean;
  onClose: () => void;
  onToggleItem: (path: string) => void;
  onConfirm: () => void;
}

function formatItemSize(sizeMb: number): string {
  if (sizeMb <= 0) return '-';
  if (sizeMb < 1024) return `${sizeMb.toFixed(2)} MB`;
  return `${(sizeMb / 1024).toFixed(2)} GB`;
}

export default function CleanupModal({
  isOpen,
  appName,
  items,
  loading,
  scanning,
  onClose,
  onToggleItem,
  onConfirm,
}: CleanupModalProps) {
  const selectedCount = useMemo(() => items.filter((item) => item.selected).length, [items]);

  if (!isOpen) return null;

  return (
    <div className="fixed inset-0 z-50 grid place-items-center p-4">
      <div
        className="absolute inset-0"
        style={{
          background: 'linear-gradient(180deg, rgba(15,23,42,0.42), rgba(2,6,23,0.62))',
          backdropFilter: 'blur(12px)',
        }}
        onClick={loading || scanning ? undefined : onClose}
      />

      <div
        className="relative w-full overflow-hidden rounded-xl shadow-2xl animate-modal-in"
        style={{
          maxWidth: '640px',
          background: 'var(--bg-modal)',
          border: '1px solid var(--border-color)',
        }}
      >
        {/* 头部 */}
        <div className="flex items-start justify-between px-5 pt-4 pb-3" style={{ borderBottom: '1px solid var(--border-color)' }}>
          <div className="pr-4 min-w-0">
            <h2 className="text-base font-semibold" style={{ color: 'var(--text-primary)' }}>
              {scanning ? '残留扫描' : '残留清理'}
            </h2>
            <p className="text-xs mt-1" style={{ color: 'var(--text-secondary)' }}>
              {scanning
                ? `正在检测 ${appName} 的残留文件...`
                : `${appName} · 共 ${items.length} 项残留，已默认全部选中`}
            </p>
          </div>
          <button
            onClick={onClose}
            disabled={loading || scanning}
            className="btn btn-ghost btn-icon flex-shrink-0"
            aria-label="关闭弹窗"
          >
            <X className="w-4 h-4" />
          </button>
        </div>

        {/* 体部 */}
        <div className="overflow-y-auto px-5 py-3" style={{ maxHeight: 'min(360px, 50vh)' }}>
          {scanning ? (
            <div className="py-12 flex flex-col items-center justify-center gap-4">
              <ScanSearch className="w-8 h-8 animate-pulse" style={{ color: 'var(--color-primary)' }} />
              <div className="text-center">
                <p className="text-[13px] font-medium" style={{ color: 'var(--text-secondary)' }}>
                  正在扫描残留文件...
                </p>
                <p className="text-[11px] mt-1" style={{ color: 'var(--text-tertiary)' }}>
                  检测 AppData / LocalAppData / ProgramData 及注册表
                </p>
              </div>
            </div>
          ) : items.length === 0 ? (
            <div className="py-10 text-center">
              <p style={{ color: 'var(--text-tertiary)', fontSize: 'var(--font-size-sm)' }}>未发现可清理残留</p>
            </div>
          ) : (
            <div className="flex flex-col" style={{ gap: 'var(--spacing-2)' }}>
              {items.map((item) => (
                <label
                  key={item.path}
                  className={`flex items-center rounded-lg border transition-all ${loading ? 'cursor-default' : 'cursor-pointer'}`}
                  style={{
                    padding: '8px 12px',
                    borderColor: item.selected ? 'var(--color-primary)' : 'var(--border-color)',
                    background: item.selected ? 'var(--color-primary-light)' : 'var(--bg-row)',
                  }}
                >
                  <div className="relative flex-shrink-0 w-4 h-4">
                    {/* 保留原生 checkbox 的键盘/辅助功能，只替换视觉层以跟随首页主题色。 */}
                    <input
                      type="checkbox"
                      checked={item.selected}
                      onChange={() => onToggleItem(item.path)}
                      disabled={loading}
                      aria-label={`${item.selected ? '取消选择' : '选择'} ${item.path}`}
                      className="peer absolute inset-0 z-10 m-0 h-4 w-4 cursor-pointer opacity-0 disabled:cursor-default"
                    />
                    <span
                      aria-hidden="true"
                      className={`absolute inset-0 flex items-center justify-center rounded-sm border transition-colors ${
                        item.selected ? '' : 'opacity-60 peer-hover:opacity-100'
                      }`}
                      style={{
                        background: item.selected ? 'var(--color-primary)' : 'transparent',
                        borderColor: item.selected ? 'var(--color-primary)' : 'var(--border-color-strong)',
                      }}
                    >
                      {item.selected && <Check className="w-3 h-3 text-white" strokeWidth={3} />}
                    </span>
                  </div>
                  <div className="min-w-0 flex-1 ml-3">
                    <div className="flex items-center justify-between gap-2">
                      <span className="badge badge-primary" style={{ fontSize: '10px' }}>{item.item_type}</span>
                      <span style={{ color: 'var(--text-tertiary)', fontSize: '11px', flexShrink: 0 }}>
                        {formatItemSize(item.size_mb)}
                      </span>
                    </div>
                    <p
                      className="mt-1 truncate"
                      style={{
                        color: 'var(--text-primary)',
                        fontSize: '12px',
                      }}
                      title={item.path}
                    >
                      {item.path}
                    </p>
                  </div>
                </label>
              ))}
            </div>
          )}
        </div>

        {/* 底部操作栏 */}
        {!scanning && (
          <div
            className="flex items-center justify-between px-5 py-3"
            style={{
              borderTop: '1px solid var(--border-color)',
              background: 'color-mix(in srgb, var(--color-gray-50) 74%, transparent)',
            }}
          >
            <div className="flex items-center gap-2">
              <span style={{ color: 'var(--text-secondary)', fontSize: 'var(--font-size-sm)' }}>
                已选 {selectedCount}/{items.length}
              </span>
              {items.length > 0 && (
                <span
                  className="cursor-pointer"
                  style={{ color: 'var(--color-warning)', fontSize: 'var(--font-size-xs)' }}
                  title="删除后不可恢复"
                >
                  <AlertTriangle className="w-3 h-3 inline" />
                </span>
              )}
            </div>
            <div className="flex items-center" style={{ gap: 'var(--spacing-2)' }}>
              <button className="btn btn-sm" onClick={onClose} disabled={loading}>
                {items.length === 0 ? '关闭' : '取消'}
              </button>
              {items.length > 0 && (
                <button
                  onClick={onConfirm}
                  disabled={loading || selectedCount === 0}
                  className="btn btn-sm"
                  style={{
                    background: 'var(--color-danger)',
                    color: 'var(--text-inverse)',
                    borderColor: 'var(--color-danger)',
                  }}
                >
                  {loading ? (
                    <>
                      <LoaderCircle className="w-3.5 h-3.5 animate-spin" />
                      清理中...
                    </>
                  ) : (
                    <>
                      <Trash2 className="w-3.5 h-3.5" />
                      确认清理
                    </>
                  )}
                </button>
              )}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
