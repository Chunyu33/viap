// 通用开关组件
//
// 设置页原先有一个名为 Toggle 的局部实现，只在该页内部可用，其他模块无法复用。
// 这里抽成独立组件：交互与视觉沿用原有样式（绿色胶囊 + 白色滑块），保证新旧开关
// 外观完全一致；开关只关心「当前值 + 目标值回调」，不掺入业务语义。

interface SwitchProps {
  /** 当前是否处于打开状态 */
  checked: boolean;
  /** 切换回调：直接给出目标值，避免每个调用点都写一遍 !checked */
  onChange: (checked: boolean) => void;
  /** 是否禁用 */
  disabled?: boolean;
  /** 悬停提示，同时作为无障碍标签的兜底 */
  title?: string;
}

export default function Switch({ checked, onChange, disabled = false, title }: SwitchProps) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={title}
      title={title}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className="relative flex-shrink-0 rounded-full cursor-pointer transition-colors disabled:cursor-not-allowed"
      style={{
        width: '36px',
        height: '20px',
        background: checked ? 'var(--color-primary)' : 'var(--color-gray-300)',
      }}
    >
      <span
        className="absolute top-0.5 w-4 h-4 rounded-full shadow-sm transition-all"
        style={{ left: checked ? '18px' : '2px', background: '#FFFFFF' }}
      />
    </button>
  );
}
