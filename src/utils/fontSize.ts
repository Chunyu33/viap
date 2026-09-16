export const DEFAULT_FONT_SIZE_PX = 13;
export const MIN_FONT_SIZE_PX = 12;
/** 上限放宽到 20px，兼顾 4K 屏幕在 100% 缩放下的可读性 */
export const MAX_FONT_SIZE_PX = 20;

/** 限制字号范围，避免用户设置过大导致桌面工具界面被撑破。 */
export function normalizeFontSizePx(input: unknown): number {
  const parsed = typeof input === 'number' ? input : Number(input);
  if (!Number.isFinite(parsed)) return DEFAULT_FONT_SIZE_PX;
  return Math.min(MAX_FONT_SIZE_PX, Math.max(MIN_FONT_SIZE_PX, Math.round(parsed)));
}

/** 将用户字号写入根节点 CSS 变量，让四个功能模块统一跟随。 */
export function applyFontSize(fontSizePx: unknown) {
  const normalized = normalizeFontSizePx(fontSizePx);
  document.documentElement.style.setProperty('--font-size-base', `${normalized}px`);
}
