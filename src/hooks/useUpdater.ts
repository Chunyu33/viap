// 自动更新检测 Hook
// 封装 @tauri-apps/plugin-updater 的 check / downloadAndInstall 流程，
// 提供状态、进度和版本信息供 UpdateNotification 组件消费

import { check, type Update } from '@tauri-apps/plugin-updater';
import { relaunch } from '@tauri-apps/plugin-process';
import { invoke } from '@tauri-apps/api/core';
import { useState, useCallback, useEffect, useRef } from 'react';

export const PORTABLE_UPDATE_URL = 'https://github.com/Chunyu33/viap/releases/latest';
// 便携版不接入自动更新，额外提供固定网盘入口作为手动下载渠道。
export const PORTABLE_CLOUD_UPDATE_URL = 'https://pan.quark.cn/s/4761ee4ba698';

export type UpdateStatus =
  | 'idle'
  | 'checking'
  | 'available'
  | 'downloading'
  | 'installing'
  | 'up-to-date'
  | 'error';

export interface UpdateInfo {
  version: string;
  notes: string;
  pubDate: string;
}

export function useUpdater() {
  const [status, setStatus] = useState<UpdateStatus>('idle');
  const [updateInfo, setUpdateInfo] = useState<UpdateInfo | null>(null);
  const [downloadProgress, setDownloadProgress] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [isPortable, setIsPortable] = useState<boolean | null>(null);
  // 保存 Update 对象引用，避免 downloadAndInstall 闭包过期
  const updateRef = useRef<Update | null>(null);
  // 取消标志：用户 dismiss 后阻止后台下载完成后的 relaunch
  const cancelRef = useRef(false);

  useEffect(() => {
    // 发行模式由 Rust feature 决定，前端不依赖文件名判断，避免便携版误触发在线更新。
    invoke<boolean>('is_portable_build')
      .then(setIsPortable)
      .catch(() => setIsPortable(false));
  }, []);

  const checkForUpdate = useCallback(async (): Promise<Update | null> => {
    if (isPortable !== false) {
      setStatus('idle');
      setError(null);
      return null;
    }

    cancelRef.current = false; // 新一次检测/下载流程开始，重置取消标志
    setStatus('checking');
    setError(null);
    try {
      const update = await check();
      if (update?.available) {
        updateRef.current = update;
        setUpdateInfo({
          version: update.version,
          notes: update.body ?? '',
          pubDate: update.date ?? '',
        });
        setStatus('available');
        return update;
      } else {
        setStatus('up-to-date');
        return null;
      }
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      setError(msg);
      setStatus('error');
      return null;
    }
  }, [isPortable]);

  const downloadAndInstall = useCallback(async (update?: Update) => {
    if (isPortable !== false) return;

    const target = update ?? updateRef.current;
    if (!target) return;

    setStatus('downloading');
    setDownloadProgress(0);
    setError(null);
    try {
      let downloaded = 0;
      // 记录总大小，用于计算百分比；可能为 null（服务器不返回 Content-Length）
      let contentLength: number | null = null;

      await target.downloadAndInstall((event) => {
        switch (event.event) {
          case 'Started':
            contentLength = event.data.contentLength ?? null;
            break;
          case 'Progress':
            // 用户取消后不再更新进度，避免界面状态残留
            if (cancelRef.current) return;
            downloaded += event.data.chunkLength;
            if (contentLength && contentLength > 0) {
              setDownloadProgress(Math.round((downloaded / contentLength) * 100));
            }
            // 无 contentLength 时不更新进度值，保持默认的 0%（进度条显示 5% 初始态）
            break;
          case 'Finished':
            // 用户已取消，不触发安装和重启
            if (cancelRef.current) return;
            setDownloadProgress(100);
            setStatus('installing');
            break;
        }
      });

      // 用户取消后不再重启应用
      if (cancelRef.current) return;
      await relaunch();
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      setError(msg);
      setStatus('error');
    }
  }, [isPortable]);

  const dismiss = useCallback(() => {
    cancelRef.current = true; // 通知后台下载回调终止，阻止 relaunch
    setStatus('idle');
    setUpdateInfo(null);
    setError(null);
    setDownloadProgress(0);
  }, []);

  return {
    status,
    updateInfo,
    downloadProgress,
    error,
    isPortable,
    updateRef,
    checkForUpdate,
    downloadAndInstall,
    dismiss,
  };
}
