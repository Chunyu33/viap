// 窗口尺寸记忆
//
// 拖动窗口时会连续触发 resize 事件（一次拖动几十次），而每次保存都是一次
// 「序列化 + 写临时文件 + 重命名」的磁盘写入，因此这里用**尾触发防抖**而不是节流：
// 拖动过程中完全不落盘，等用户松手停下来 400ms 后才写一次，一次拖动只产生 1 次写入。
// 节流会在拖动过程中按固定间隔反复写入，对 SSD 是无意义的写放大，故不采用。
//
// 另外只记录逻辑像素（CSS 像素），这样在不同缩放比的显示器上切换时，
// 界面元素的视觉大小保持一致。

import { getCurrentWindow } from '@tauri-apps/api/window';
import { invoke } from '@tauri-apps/api/core';
import { logger } from './logger';

/** 松手后等待多久再落盘：足够覆盖拖动的间隔，又不会让用户等太久 */
const SAVE_DEBOUNCE_MS = 400;

/** 启动窗口尺寸跟踪；返回取消函数（应用生命周期内一般不需要调用） */
export async function startWindowSizeTracking(): Promise<() => void> {
  try {
    const appWindow = getCurrentWindow();
    let timer: number | null = null;

    const persistSize = async () => {
      // 最大化/全屏时记录的是屏幕尺寸，会把用户偏好的窗口大小覆盖掉，必须跳过
      const [maximized, fullscreen, minimized, innerSize, scaleFactor] = await Promise.all([
        appWindow.isMaximized(),
        appWindow.isFullscreen(),
        appWindow.isMinimized(),
        appWindow.innerSize(),
        appWindow.scaleFactor(),
      ]);
      if (maximized || fullscreen || minimized || scaleFactor <= 0) {
        return;
      }

      const logical = innerSize.toLogical(scaleFactor);
      await invoke('save_window_state', {
        width: Math.round(logical.width),
        height: Math.round(logical.height),
      });
    };

    const unlisten = await appWindow.onResized(() => {
      if (timer !== null) window.clearTimeout(timer);
      timer = window.setTimeout(() => {
        timer = null;
        // 防抖回调里再读取当前尺寸，保证写入的是最终值而不是事件里的中间值
        void persistSize().catch((error) => logger.error('保存窗口尺寸失败:', error));
      }, SAVE_DEBOUNCE_MS);
    });

    return () => {
      if (timer !== null) window.clearTimeout(timer);
      unlisten();
    };
  } catch (error) {
    // 浏览器环境下没有窗口 API，仅记录不影响应用启动
    logger.warn('窗口尺寸跟踪未启用:', error);
    return () => undefined;
  }
}
