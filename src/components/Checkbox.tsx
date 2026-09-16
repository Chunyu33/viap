// 通用主题化复选框
//
// 原生 input[type=checkbox] 在深色主题下仍是系统默认配色，与主题色冲突；
// 这里保留原生 input 的可访问性与键盘操作，只替换视觉层，颜色由 CSS 变量控制。
//
// 只渲染方框本身（不渲染 <label>），因为它常被放在整行可点击的 <label> 内，
// 嵌套 <label> 会破坏点击行为。

import { Check } from 'lucide-react';

interface CheckboxProps {
  checked: boolean;
  onChange: (checked: boolean) => void;
  disabled?: boolean;
  /** 无外部文字标签时必填，供屏幕阅读器识别 */
  ariaLabel?: string;
  /** sm 用于密集选项行，md 为默认尺寸 */
  size?: 'sm' | 'md';
  className?: string;
}

export default function Checkbox({
  checked,
  onChange,
  disabled = false,
  ariaLabel,
  size = 'md',
  className = '',
}: CheckboxProps) {
  const isSmall = size === 'sm';

  return (
    <span
      className={`relative inline-flex flex-shrink-0 ${isSmall ? 'w-3.5 h-3.5' : 'w-4 h-4'} ${className}`}
      style={disabled ? { opacity: 0.5 } : undefined}
    >
      <input
        type="checkbox"
        checked={checked}
        disabled={disabled}
        onChange={(event) => onChange(event.target.checked)}
        aria-label={ariaLabel}
        className="peer absolute inset-0 z-10 m-0 h-full w-full cursor-pointer opacity-0 disabled:cursor-default"
      />
      <span
        aria-hidden="true"
        className={`absolute inset-0 flex items-center justify-center rounded-sm border transition-colors ${
          checked ? '' : 'opacity-60 peer-hover:opacity-100 peer-focus-visible:opacity-100'
        }`}
        style={{
          background: checked ? 'var(--color-primary)' : 'transparent',
          borderColor: checked ? 'var(--color-primary)' : 'var(--border-color-strong)',
        }}
      >
        {/* 勾选标记始终占位，避免切换时图标撑开方框 */}
        <Check
          strokeWidth={3}
          className={isSmall ? 'w-2.5 h-2.5' : 'w-3 h-3'}
          style={{
            color: 'var(--text-inverse)',
            visibility: checked ? 'visible' : 'hidden',
          }}
        />
      </span>
    </span>
  );
}
