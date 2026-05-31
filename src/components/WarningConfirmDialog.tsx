// 高风险操作确认弹窗
// WARNING 级别危险路径命中时弹出，展示风险原因 + 免责声明，
// 用户确认后才放行迁移流程
// 复用项目现有过渡动画体系（fadeIn/fadeOut + modalIn/modalOut）

import { useEffect, useState, useCallback } from 'react';
import { X, AlertTriangle } from 'lucide-react';
import type { WarningInfo } from '../hooks/useDangerousPathCheck';

interface WarningConfirmDialogProps {
  isOpen: boolean;
  warningInfo: WarningInfo | null;
  onConfirm: () => void;
  onCancel: () => void;
}

export default function WarningConfirmDialog({
  isOpen, warningInfo, onConfirm, onCancel,
}: WarningConfirmDialogProps) {
  const [visible, setVisible] = useState(false);
  const [leaving, setLeaving] = useState(false);

  useEffect(() => {
    if (isOpen) {
      setVisible(true);
      setLeaving(false);
    } else if (visible) {
      setLeaving(true);
      const timer = setTimeout(() => { setVisible(false); setLeaving(false); }, 150);
      return () => clearTimeout(timer);
    }
  }, [isOpen, visible]);

  const handleCancel = useCallback(() => {
    setLeaving(true);
    setTimeout(() => { setVisible(false); setLeaving(false); onCancel(); }, 150);
  }, [onCancel]);

  const handleConfirm = useCallback(() => {
    setLeaving(true);
    setTimeout(() => { setVisible(false); setLeaving(false); onConfirm(); }, 150);
  }, [onConfirm]);

  // 按 Escape 关闭
  useEffect(() => {
    if (!isOpen || !visible) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') handleCancel();
    };
    document.addEventListener('keydown', handler);
    return () => document.removeEventListener('keydown', handler);
  }, [isOpen, visible, handleCancel]);

  if (!visible || !warningInfo) return null;

  return (
    <div
      className="fixed inset-0 z-50 grid place-items-center p-4"
      style={{ animation: leaving ? 'fadeOut 150ms ease-in forwards' : 'fadeIn 150ms ease-out' }}
    >
      {/* 半透明遮罩 — 点击关闭 */}
      <div
        className="absolute inset-0"
        style={{ background: 'var(--bg-modal-overlay)', backdropFilter: 'blur(8px)' }}
        onClick={handleCancel}
      />

      {/* 弹窗主体 */}
      <div
        className={`relative w-full overflow-hidden rounded-xl shadow-lg ${leaving ? 'animate-modal-out' : 'animate-modal-in'}`}
        style={{
          maxWidth: '480px',
          background: 'var(--bg-modal)',
          border: '1px solid var(--border-color)',
        }}
      >
        {/* 标题栏 */}
        <div
          className="flex items-center justify-between px-5 pt-3.5 pb-3"
          style={{ borderBottom: '1px solid var(--border-color)' }}
        >
          <div className="flex items-center gap-2">
            <AlertTriangle style={{ width: 16, height: 16, color: 'var(--color-warning)' }} />
            <h2 className="text-sm font-semibold" style={{ color: 'var(--text-primary)' }}>
              高风险操作确认
            </h2>
          </div>
          <button onClick={handleCancel} className="btn btn-ghost btn-icon" aria-label="关闭">
            <X style={{ width: 14, height: 14 }} />
          </button>
        </div>

        {/* 内容区 */}
        <div className="px-5 py-4" style={{ maxHeight: '60vh', overflowY: 'auto' }}>
          {/* 命中信息 */}
          <p className="text-sm font-medium mb-3" style={{ color: 'var(--text-primary)' }}>
            {warningInfo.label} 属于「{warningInfo.category}」类目录
          </p>

          {/* 风险原因 */}
          <p className="text-xs mb-3 leading-relaxed" style={{ color: 'var(--text-secondary)' }}>
            {warningInfo.reason}
          </p>

          {/* 免责声明区域 */}
          <div
            className="rounded-lg p-3"
            style={{
              background: 'var(--bg-row-hover)',
              maxHeight: 180,
              overflowY: 'auto',
            }}
          >
            <p
              className="text-xs leading-relaxed whitespace-pre-line"
              style={{ color: 'var(--text-tertiary)' }}
            >
              {warningInfo.disclaimer}
            </p>
          </div>
        </div>

        {/* 按钮区 */}
        <div
          className="flex items-center justify-end gap-2 px-5 py-3"
          style={{
            borderTop: '1px solid var(--border-color)',
            background: 'var(--bg-toolbar)',
          }}
        >
          <button onClick={handleCancel} className="btn btn-sm">
            取消
          </button>
          <button
            onClick={handleConfirm}
            className="btn btn-sm"
            style={{
              background: 'var(--color-warning)',
              color: 'var(--text-inverse)',
              borderColor: 'var(--color-warning)',
            }}
          >
            我已了解风险，继续迁移
          </button>
        </div>
      </div>
    </div>
  );
}
