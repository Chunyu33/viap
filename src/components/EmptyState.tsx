// 空状态占位组件 — 桌面工具风格
// 用于列表/表格无数据时，居中展示图标 + 提示文字

import type { ReactNode } from 'react';

interface EmptyStateProps {
  icon: ReactNode;
  title: string;
  description?: string;
  /** 可选的空态操作入口（如「恢复迁移记录」），由调用方提供按钮 */
  action?: ReactNode;
}

export default function EmptyState({ icon, title, description, action }: EmptyStateProps) {
  return (
    <div className="flex-1 flex flex-col items-center justify-center gap-3 select-none">
      <div
        className="w-12 h-12 rounded-full flex items-center justify-center"
        style={{ background: 'var(--bg-row-hover)' }}
      >
        <div className="w-5 h-5" style={{ color: 'var(--text-tertiary)' }}>
          {icon}
        </div>
      </div>
      <div className="text-center">
        <p className="text-[13px] font-medium" style={{ color: 'var(--text-secondary)' }}>
          {title}
        </p>
        {description && (
          <p className="text-[11px] mt-1" style={{ color: 'var(--text-tertiary)' }}>
            {description}
          </p>
        )}
      </div>
      {action}
    </div>
  );
}
