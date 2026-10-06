// 液态玻璃外观开关
//
// 只在 <html> 上增删一个 class，全部视觉效果由 src/styles/glass.css 里
// 以 `html.glass` 开头的选择器承载。这样设计的原因：
// 1. 关闭时根节点没有 glass 类，玻璃样式选择器零匹配，界面与改动前完全一致，
//    因此「关闭态」本身就是最直接的回归验证；
// 2. 不需要给任何组件改类名、加内联样式，天然不会影响既有配色与布局。

/** 挂在 <html> 上的玻璃模式类名，与 glass.css 的选择器前缀保持一致 */
export const GLASS_ROOT_CLASS = 'glass';

/** 应用液态玻璃开关：仅切换根节点 class，不触碰组件自身样式。 */
export function applyGlass(enabled: boolean): void {
  if (typeof document === 'undefined') return;
  document.documentElement.classList.toggle(GLASS_ROOT_CLASS, enabled);
}
