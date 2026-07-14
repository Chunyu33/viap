// Viap - 主应用组件
// 企业级模块化设计
// 集成主题系统，支持浅色/深色/跟随系统三种模式

import { useEffect, useState, createContext, useContext, type ReactNode } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { FolderSync, FolderArchive, History, Settings as SettingsIcon } from 'lucide-react';
import TitleBar from './components/TitleBar';
import DiskUsageBar from './components/DiskUsageBar';
import UpdateNotification from './components/UpdateNotification';
import AppMigration from './pages/AppMigration';
import LargeFolders from './pages/LargeFolders';
import MigrationHistory from './pages/MigrationHistory';
import Settings from './pages/Settings';
import StartupScreen from './components/StartupScreen';
import { DiskUsage, TabType } from './types';
import { useTheme, ThemeMode, ResolvedTheme } from './hooks/useTheme';
import './App.css';

// 主题上下文 - 供子组件访问主题状态
interface ThemeContextType {
  mode: ThemeMode;
  theme: ResolvedTheme;
  setTheme: (mode: ThemeMode) => void;
  isDark: boolean;
}

export const ThemeContext = createContext<ThemeContextType | null>(null);

// Tab 导航上下文 - 供子页面切换到设置页
export const TabNavigationContext = createContext<((tab: TabType) => void) | null>(null);

// 便捷 Hook：在子组件中使用主题
export function useThemeContext() {
  const context = useContext(ThemeContext);
  if (!context) {
    throw new Error('useThemeContext 必须在 ThemeContext.Provider 内使用');
  }
  return context;
}

const tabs: { id: TabType; label: string; Icon: typeof FolderSync }[] = [
  { id: 'migration', label: '应用管理', Icon: FolderSync },
  { id: 'folders', label: '数据迁移', Icon: FolderArchive },
  { id: 'history', label: '迁移记录', Icon: History },
  { id: 'settings', label: '设置', Icon: SettingsIcon },
];

/** 页面容器 — absolute 填充父容器，底部滑入 + 淡入过渡 */
function PageContainer({ visible, children }: { visible: boolean; children: ReactNode }) {
  return (
    <div
      style={{
        position: 'absolute',
        inset: 0,
        opacity: visible ? 1 : 0,
        transform: visible ? 'translateY(0)' : 'translateY(18px)',
        pointerEvents: visible ? 'auto' : 'none',
        zIndex: visible ? 1 : 0,
        transition: 'opacity 200ms ease-out, transform 220ms cubic-bezier(0.16, 1, 0.3, 1)',
        overflow: 'hidden',
      }}
    >
      {children}
    </div>
  );
}

function App() {
  const [activeTab, setActiveTab] = useState<TabType>('migration');
  const [mountedTabs, setMountedTabs] = useState<Set<TabType>>(() => new Set(['migration']));
  const [disks, setDisks] = useState<DiskUsage[]>([]);
  const [diskLoading, setDiskLoading] = useState(true);
  const [diskRefreshing, setDiskRefreshing] = useState(false);
  const [startupVisible, setStartupVisible] = useState(true);

  // 初始化主题系统
  const themeState = useTheme();

  async function fetchDiskUsage() {
    try {
      setDiskLoading(true);
      const diskList = await invoke<DiskUsage[]>('get_disk_usage');
      setDisks(diskList);
    } catch (error) {
      console.error('获取全局磁盘信息失败:', error);
      setDisks([]);
    } finally {
      setDiskLoading(false);
    }
  }

  async function handleRefreshDiskUsage() {
    setDiskRefreshing(true);
    await fetchDiskUsage();
    setDiskRefreshing(false);
  }

  useEffect(() => {
    fetchDiskUsage();
  }, []);

  useEffect(() => {
    // 固定短暂展示启动页，让机械硬盘读取快照和注册表时也能先看到稳定的主题界面。
    const timer = window.setTimeout(() => setStartupVisible(false), 2400);
    return () => window.clearTimeout(timer);
  }, []);

  useEffect(() => {
    // 窗口默认隐藏，首帧挂载后再显示，避免低配机器看到 WebView 白屏。
    requestAnimationFrame(() => {
      invoke('frontend_ready').catch(() => {});
    });
  }, []);

  useEffect(() => {
    // 非首屏页面按需挂载，避免启动时一次初始化全部模块造成白屏等待。
    setMountedTabs(prev => {
      if (prev.has(activeTab)) return prev;
      const next = new Set(prev);
      next.add(activeTab);
      return next;
    });
  }, [activeTab]);

  return (
    <ThemeContext.Provider value={themeState}>
      <TabNavigationContext.Provider value={setActiveTab}>
      <div className="flex flex-col h-screen overflow-hidden" style={{ background: 'var(--bg-app)' }}>

        {/* 统一标题栏：Logo + Tab 导航 + 磁盘状态 + 窗口控制 */}
        <TitleBar
          centerContent={(
            <div className="flex items-center gap-1 p-0.5 rounded-lg" style={{ background: 'var(--bg-hover)' }}>
              {tabs.map((tab) => {
                const isActive = activeTab === tab.id;
                const Icon = tab.Icon;

                return (
                  <button
                    key={tab.id}
                    type="button"
                    onClick={() => setActiveTab(tab.id)}
                    className="relative flex items-center gap-1.5 h-7 px-3 rounded-md text-[12px] font-medium transition-all duration-200"
                    style={{
                      color: isActive ? 'var(--color-primary)' : 'var(--text-secondary)',
                      background: isActive ? 'var(--bg-content)' : 'transparent',
                    }}
                  >
                    {/* 激活态左侧小圆点 */}
                    {isActive && (
                      <span
                        className="absolute left-1.5 top-1/2 -translate-y-1/2 w-1 h-1 rounded-full"
                        style={{ background: 'var(--color-primary)' }}
                      />
                    )}
                    <Icon className="w-3.5 h-3.5" />
                    <span>{tab.label}</span>
                  </button>
                );
              })}
            </div>
          )}
          rightContent={(
            <DiskUsageBar
              disks={disks}
              loading={diskLoading}
              refreshing={diskRefreshing}
              onRefresh={handleRefreshDiskUsage}
            />
          )}
        />

        {/* 标题栏下方：更新通知条 */}
        <UpdateNotification />

        {/* 页面内容区域 — CSS display 切换，组件实例保持存活，opacity 过渡动画 */}
        <main className="flex-1 overflow-hidden" style={{ background: 'var(--bg-content)', position: 'relative' }}>
          {mountedTabs.has('migration') && (
            <PageContainer visible={activeTab === 'migration'}>
              <AppMigration visible={activeTab === 'migration'} />
            </PageContainer>
          )}
          {mountedTabs.has('folders') && (
            <PageContainer visible={activeTab === 'folders'}>
              <LargeFolders visible={activeTab === 'folders'} />
            </PageContainer>
          )}
          {mountedTabs.has('history') && (
            <PageContainer visible={activeTab === 'history'}>
              <MigrationHistory visible={activeTab === 'history'} />
            </PageContainer>
          )}
          {mountedTabs.has('settings') && (
            <PageContainer visible={activeTab === 'settings'}>
              <Settings visible={activeTab === 'settings'} />
            </PageContainer>
          )}
        </main>
      </div>
      {startupVisible && <StartupScreen />}
      </TabNavigationContext.Provider>
    </ThemeContext.Provider>
  );
}

export default App;
