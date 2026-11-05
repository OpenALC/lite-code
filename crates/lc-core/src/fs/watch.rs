//! 文件变更监听骨架（docs/ARCHITECTURE.md §5.2）。
//!
//! `notify` + `notify-debouncer-full`，去抖 200ms：一批原始事件合并为一个
//! [`WatchEvent`]，携带 `(generation, paths)`。generation 单调递增，初始为 0
//! （"无变更"基线），每批 +1；前端记录已拉取的 generation，发现跳变即重拉
//! ——与 `EventEnvelope::seq` 的跳变语义一致（crates/lc-proto/src/event.rs）。
//!
//! 这是骨架：只负责"变更 → 通知"。忽略表（worktree 过滤，docs/TASKS.md §3）
//! 与向 UI 的分发由消费方在 `WatchSink` 里接线。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};

use super::paths::{PathResolver, RealPathResolver, display_path};
use super::{FsError, io_error};

/// 去抖窗口（docs/ARCHITECTURE.md §5.2：200ms）。
pub const DEBOUNCE: Duration = Duration::from_millis(200);

/// 一批去抖后的变更：涉及路径（已转为展示路径）+ 跳变后的 generation。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchEvent {
    pub generation: u64,
    pub paths: Vec<String>,
}

/// 变更接收方。Tauri 壳把它适配成契约的 `CoreEvent::FsTree` / `FsEntry`
/// （crates/lc-proto/src/event.rs），headless 前端可用来驱动 golden 回归。
pub trait WatchSink: Send + Sync {
    fn on_batch(&self, event: WatchEvent);
}

/// 递归监听一个目录。drop 即停（释放 watch 句柄并结束后台线程）。
pub struct FileWatcher {
    root: PathBuf,
    generation: Arc<AtomicU64>,
    /// 必须持有：notify 的 watch 生命周期与去抖线程都挂在它上面
    _inner: Debouncer<RecommendedWatcher, RecommendedCache>,
}

// 手写而非 derive：Debouncer 没有 Debug；日志里只需 root 与当前 generation
impl std::fmt::Debug for FileWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileWatcher")
            .field("root", &self.root)
            .field("generation", &self.generation.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl FileWatcher {
    /// 递归监听 `root`。`sink` 在 notify 回调线程上被调用，必须快速返回
    /// （只做转发；重拉目录树由前端收到 generation 跳变后自行触发）。
    pub fn new(root: &Path, sink: Arc<dyn WatchSink>) -> Result<Self, FsError> {
        // 与 fs 其余入口同一套路径语义：canonicalize 顺带校验存在性
        let resolver = RealPathResolver;
        let canonical = resolver.canonicalize(root).map_err(|e| io_error(root, e))?;
        if !canonical.is_dir() {
            return Err(FsError::Io {
                path: display_path(&canonical),
                message: "not a directory".into(),
            });
        }

        let generation = Arc::new(AtomicU64::new(0));
        let gen_counter = Arc::clone(&generation);
        let mut debouncer = new_debouncer(DEBOUNCE, None, move |result: DebounceEventResult| {
            // 批内错误（watch 失效、权限告警）：骨架阶段忽略，后续里程碑再分类
            let Ok(events) = result else { return };
            // 同一路径在一批里可能由多个原始事件贡献，去重后再计数
            let paths = collect_paths(
                events
                    .iter()
                    .flat_map(|event| event.paths.iter().map(|p| p.as_path())),
            );
            if paths.is_empty() {
                return;
            }
            let generation = gen_counter.fetch_add(1, Ordering::Relaxed) + 1;
            sink.on_batch(WatchEvent { generation, paths });
        })
        .map_err(|e| FsError::Io {
            path: display_path(&canonical),
            message: e.to_string(),
        })?;
        debouncer
            .watch(&canonical, RecursiveMode::Recursive)
            .map_err(|e| FsError::Io {
                path: display_path(&canonical),
                message: e.to_string(),
            })?;

        Ok(Self {
            root: canonical,
            generation,
            _inner: debouncer,
        })
    }

    /// 监听根（canonicalize 后的展示形态）。
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 当前 generation，与最近一次 [`WatchEvent::generation`] 一致。
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }
}

/// 将一批原始事件里的路径排序去重，并转为展示路径。
/// 空批返回空 Vec（调用方据此跳过 generation 自增）。
pub(super) fn collect_paths<'a>(paths: impl IntoIterator<Item = &'a Path>) -> Vec<String> {
    let mut paths: Vec<PathBuf> = paths.into_iter().map(Path::to_path_buf).collect();
    paths.sort();
    paths.dedup();
    paths.iter().map(|p| display_path(p)).collect()
}
