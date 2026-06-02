// 目录大小持久化缓存模块
//
// 策略：Stale-While-Revalidate (SWR)
//   Phase 1: 查询缓存 → 命中则立即 emit 缓存值（即使已过期），前端秒显
//   Phase 2: 无论缓存是否命中，都执行 get_dir_size_safe() 重算真实大小
//            new_size != cached → cache.set() + emit 新值刷新 UI
//            new_size == cached → 仅刷新 updated_at，避免缓存永不过期
//
// 缓存文件：{data_dir}/cache/size_cache.json
// TTL：7 天，仅用于判断是否需要后台刷新，不阻止缓存显示和重算

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// 缓存条目
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheEntry {
    /// 目录大小（字节）
    size: u64,
    /// 最后验证时间戳（Unix 秒）—— 每次 set 都刷新，即使 size 未变
    updated_at: u64,
}

/// 磁盘持久化结构
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheFile {
    entries: HashMap<String, CacheEntry>,
}

/// 目录大小缓存（内存层）
pub struct SizeCache {
    entries: HashMap<String, CacheEntry>,
    dirty: bool,
    /// 是否已从磁盘加载（防止空文件反复读取和 JSON 解析）
    loaded: bool,
}

/// 7 天 TTL（秒）
pub const SIZE_CACHE_TTL_SECS: u64 = 7 * 24 * 3600;

lazy_static::lazy_static! {
    /// 全局大小缓存单例
    pub static ref SIZE_CACHE: Mutex<SizeCache> = Mutex::new(SizeCache::new());
}

impl SizeCache {
    pub fn new() -> Self {
        Self { entries: HashMap::new(), dirty: false, loaded: false }
    }

    /// 缓存文件路径
    fn cache_file_path() -> PathBuf {
        crate::storage::data_dir::get_data_dir()
            .join("cache")
            .join("size_cache.json")
    }

    /// 延迟加载：首次调用时从磁盘读取一次
    /// loaded 标志确保即使文件为空也不重复读取和 JSON 解析
    fn ensure_loaded(&mut self) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        let path = Self::cache_file_path();
        if !path.exists() {
            return;
        }
        if let Ok(json) = std::fs::read_to_string(&path) {
            match serde_json::from_str::<CacheFile>(&json) {
                Ok(cache) => {
                    self.entries = cache.entries;
                    println!("[size_cache] loaded {} entries", self.entries.len());
                }
                Err(_) => {
                    println!("[size_cache] file corrupt, starting fresh");
                }
            }
        }
    }

    /// 获取缓存大小（字节），返回 None 表示无记录
    /// SWR：即使 TTL 已过期也返回缓存值（由调用方在 Phase 2 重算）
    pub fn get(&mut self, path: &str) -> Option<u64> {
        self.ensure_loaded();
        let key = path.to_lowercase();
        let entry = self.entries.get(&key)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let age = now.saturating_sub(entry.updated_at);
        if age > SIZE_CACHE_TTL_SECS {
            println!("[size_cache] stale hit {}", path);
        }
        Some(entry.size)
    }

    /// 写入/更新缓存条目（仅写内存，批量 flush 到磁盘）
    /// 每次调用都刷新 updated_at —— 即使 size 未变也延长 TTL，防止活跃目录缓存过期
    pub fn set(&mut self, path: &str, size_bytes: u64) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let key = path.to_lowercase();
        self.entries.insert(key, CacheEntry { size: size_bytes, updated_at: now });
        self.dirty = true;
    }

    /// 持久化到磁盘
    /// 原子写入：先写 .json.tmp，成功后再 rename 覆盖目标
    /// 调用时机：sizes_done 时一次性 flush，不在批量期间频繁 IO
    pub fn flush(&mut self) {
        if !self.dirty {
            return;
        }
        let path = Self::cache_file_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let cache = CacheFile { entries: self.entries.clone() };
        let json = match serde_json::to_string(&cache) {
            Ok(j) => j,
            Err(_) => return,
        };
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, &json).is_err() {
            return;
        }
        let _ = std::fs::rename(&tmp, &path);
        self.dirty = false;
        println!("[size_cache] flushed {} entries", self.entries.len());
    }
}
