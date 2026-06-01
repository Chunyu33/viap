// 应用列表全局缓存（模块级单例）
// 生命周期与整个应用一致，不受组件 mount/unmount 影响
// Tab 切换回应用管理页时直接恢复 state，不重新扫描

import type { InstalledApp } from '../types';

export interface AppStoreState {
  apps: InstalledApp[];
  sizeMap: Map<string, number>;
  /** 启动后是否已完成过一次完整扫描 */
  isScanned: boolean;
  /** 大小是否已计算完毕 */
  isSizesLoaded: boolean;
  scanPhase: 'idle' | 'tier1' | 'tier2' | 'tier3' | 'icons' | 'done';
  scanTotalCount: number;
}

export const appStore: AppStoreState = {
  apps: [],
  sizeMap: new Map(),
  isScanned: false,
  isSizesLoaded: false,
  scanPhase: 'idle',
  scanTotalCount: 0,
};
