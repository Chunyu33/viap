// Viap 全局状态 — Zustand store
// 取代原 appStore 模块单例 + 各组件 module-level 缓存变量
// 数据在组件卸载后依然存活，切换 Tab 不丢失状态

import { create } from 'zustand';
import type { InstalledApp, LargeFolder, MigrationRecord } from '../types';

export interface ViapStore {
  // ═══ 应用列表数据（原 appStore 模块单例 → 响应式） ═══
  apps: InstalledApp[];
  sizeMap: Map<string, number>;
  isScanned: boolean;
  isSizesLoaded: boolean;
  scanPhase: string;
  scanTotalCount: number;
  sizesLoading: boolean;

  // ═══ 应用列表 UI 状态（跨 Tab 保持） ═══
  searchQuery: string;
  migrationFilter: 'all' | 'migrated' | 'not_migrated';
  driveFilter: string;
  sortKey: 'name' | 'size' | null;
  sortOrder: 'asc' | 'desc';

  // ═══ 迁移相关缓存 ═══
  migratedPaths: string[];
  appMigrationRecords: MigrationRecord[];

  // ═══ 大文件夹缓存（避免切 Tab 后重新 fetch） ═══
  largeFolders: LargeFolder[];
  largeFoldersLoaded: boolean;
  // 应用数据大小由用户主动触发，避免 HDD 进入页面时递归扫描。
  largeFoldersAppDataLoaded: boolean;

  // ═══ 迁移记录缓存 ═══
  historyRecords: MigrationRecord[];
  historyRecordsLoaded: boolean;

  // ═══ Actions ═══
  setApps: (apps: InstalledApp[]) => void;
  setSizeMap: (map: Map<string, number>) => void;
  updateSizes: (updates: Array<{ key: string; size: number }>) => void;
  resetScan: () => void;
  setSizesLoading: (v: boolean) => void;
  setScanPhase: (phase: string) => void;
  setScanTotalCount: (n: number) => void;

  setSearchQuery: (q: string) => void;
  setMigrationFilter: (f: 'all' | 'migrated' | 'not_migrated') => void;
  setDriveFilter: (f: string) => void;
  setSort: (key: 'name' | 'size' | null, order: 'asc' | 'desc') => void;
  resetUI: () => void;

  setMigratedPaths: (paths: string[]) => void;
  setAppMigrationRecords: (records: MigrationRecord[]) => void;

  setLargeFolders: (folders: LargeFolder[]) => void;
  setLargeFoldersAppDataLoaded: (loaded: boolean) => void;
  setHistoryRecords: (records: MigrationRecord[]) => void;
}

export const useViapStore = create<ViapStore>((set) => ({
  // ── 初始值 ──
  apps: [],
  sizeMap: new Map(),
  isScanned: false,
  isSizesLoaded: false,
  scanPhase: 'idle',
  scanTotalCount: 0,
  sizesLoading: false,

  searchQuery: '',
  migrationFilter: 'all',
  driveFilter: 'all',
  sortKey: null,
  sortOrder: 'asc',

  migratedPaths: [],
  appMigrationRecords: [],

  largeFolders: [],
  largeFoldersLoaded: false,
  largeFoldersAppDataLoaded: false,

  historyRecords: [],
  historyRecordsLoaded: false,

  // ── 应用数据 actions ──
  setApps: (apps) => set({ apps, isScanned: true }),
  setSizeMap: (map) => set({ sizeMap: map, isSizesLoaded: true }),
  updateSizes: (updates) => set((state) => {
    const next = new Map(state.sizeMap);
    for (const u of updates) next.set(u.key, u.size);
    return { sizeMap: next };
  }),
  resetScan: () => set({
    apps: [], sizeMap: new Map(), isScanned: false, isSizesLoaded: false,
    scanPhase: 'idle', scanTotalCount: 0, sizesLoading: false,
  }),
  setSizesLoading: (v) => set({ sizesLoading: v }),
  setScanPhase: (phase) => set({ scanPhase: phase }),
  setScanTotalCount: (n) => set({ scanTotalCount: n }),

  // ── UI state actions ──
  setSearchQuery: (q) => set({ searchQuery: q }),
  setMigrationFilter: (f) => set({ migrationFilter: f }),
  setDriveFilter: (f) => set({ driveFilter: f }),
  setSort: (key, order) => set({ sortKey: key, sortOrder: order }),
  resetUI: () => set({ sortKey: null, sortOrder: 'asc' }),

  // ── 迁移记录 actions ──
  setMigratedPaths: (paths) => set({ migratedPaths: paths }),
  setAppMigrationRecords: (records) => set({ appMigrationRecords: records }),

  // ── 大文件夹 / 历史记录缓存 actions ──
  setLargeFolders: (folders) => set({ largeFolders: folders, largeFoldersLoaded: true }),
  setLargeFoldersAppDataLoaded: (loaded) => set({ largeFoldersAppDataLoaded: loaded }),
  setHistoryRecords: (records) => set({ historyRecords: records, historyRecordsLoaded: true }),
}));
