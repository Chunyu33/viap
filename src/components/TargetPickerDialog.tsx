// 迁移目标选择弹窗
// 替代原生 confirm：区分「使用默认」「自定义目录」和「X 取消」三个操作

import { FolderCheck, FolderSearch } from 'lucide-react';
import Modal from './Modal';

export interface TargetPickerDialogProps {
  isOpen: boolean;
  title: string;
  defaultPath: string;
  itemName: string;
  onUseDefault: () => void;
  onUseCustom: () => void;
  onClose: () => void;
}

export default function TargetPickerDialog({
  isOpen, title, defaultPath, itemName,
  onUseDefault, onUseCustom, onClose,
}: TargetPickerDialogProps) {
  return (
    // 遮罩点击与 Esc 都不关闭：这三个操作必须由用户显式选择，
    // 误触关闭会让调用方 await 的 Promise 永远悬着，迁移流程卡住
    <Modal isOpen={isOpen} onClose={onClose} title={title} width={400} closeOnOverlay={false}>
      <p className="text-xs text-center mb-4" style={{ color: 'var(--text-secondary)', lineHeight: 1.6 }}>
        {itemName}
      </p>

      <div
        className="rounded-lg p-3 mb-4 text-center"
        style={{ background: 'var(--color-primary-light)' }}
      >
        <p className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>默认迁移目录</p>
        <p className="text-xs font-medium font-mono mt-0.5" style={{ color: 'var(--text-primary)' }}>
          {defaultPath}
        </p>
      </div>

      {/* 两个操作按钮 */}
      <div className="flex flex-col gap-2">
        <button
          onClick={onUseDefault}
          className="btn h-9 text-[12px] w-full flex items-center justify-center gap-2"
          style={{ background: 'var(--color-primary)', color: 'var(--text-inverse)', borderColor: 'var(--color-primary)' }}
        >
          <FolderCheck className="w-4 h-4" />
          使用默认位置
        </button>
        <button
          onClick={onUseCustom}
          className="btn h-9 text-[12px] w-full flex items-center justify-center gap-2"
        >
          <FolderSearch className="w-4 h-4" />
          自定义目录
        </button>
      </div>
    </Modal>
  );
}
