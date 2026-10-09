// 高风险操作确认弹窗
// WARNING 级别危险路径命中时弹出，展示风险原因 + 免责声明，
// 用户确认后才放行迁移流程

import { X, AlertTriangle } from 'lucide-react';
import Modal from './Modal';
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
  // 没有风险信息就不渲染：父组件结束本次确认后会把 warningInfo 清空
  if (!warningInfo) return null;

  return (
    <Modal
      isOpen={isOpen}
      onClose={onCancel}
      width={480}
      // 标题带警告图标，塞不进默认标题栏（默认只接受文字），改用自定义头部
      header={
        <div
          className="flex items-center justify-between flex-shrink-0 px-5 pt-3.5 pb-3"
          style={{ borderBottom: '1px solid var(--border-color)' }}
        >
          <div className="flex items-center gap-2">
            <AlertTriangle style={{ width: 16, height: 16, color: 'var(--color-warning)' }} />
            <h2 className="text-sm font-semibold" style={{ color: 'var(--text-primary)' }}>
              高风险操作确认
            </h2>
          </div>
          <button onClick={onCancel} className="btn btn-ghost btn-icon" aria-label="关闭">
            <X style={{ width: 14, height: 14 }} />
          </button>
        </div>
      }
      footer={
        <div
          className="flex items-center justify-end gap-2 px-5 py-3"
          style={{ borderTop: '1px solid var(--border-color)', background: 'var(--bg-modal-footer)' }}
        >
          <button onClick={onCancel} className="btn btn-sm">
            取消
          </button>
          <button
            onClick={onConfirm}
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
      }
    >
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
          // 面板内的底衬：普通模式下 --bg-inset 就是原来的 --bg-row-hover，
          // 玻璃下换成很淡的一档，避免叠出「贴上去的白块」
          background: 'var(--bg-inset)',
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
    </Modal>
  );
}
