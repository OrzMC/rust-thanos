//! Progress events + sinks.
//!
//! Mirrors OrzMCBackup's `Progress.kt` / `ProgressSink.kt`: the engine emits
//! [`ProgressEvent`]s through a [`ProgressSink`], and [`ProgressTracker`] owns
//! the processed counter while throttling chunk-level events by `interval`
//! (every N chunks) or `interval_ms` (every N milliseconds). [`TermProgressSink`]
//! is the CLI's default renderer: Global mode shows a percentage bar (or percent
//! lines off a TTY), Region mode prints one line per region with kept/removed
//! counts.

use crate::world::optimizer::ProgressMode;
use console::Term;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::path::Path;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// Phases of the optimization pipeline, emitted as [`ProgressEvent`] values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressStage {
    /// Initialization.
    Init,
    /// Discovering dimensions / counting chunks.
    Discover,
    /// Processing started for one dimension.
    DimensionStart,
    /// Processing started for one region file.
    RegionStart,
    /// Chunk-level progress update.
    ChunkProgress,
    /// Processing finished for one region file (region name + kept/removed).
    RegionEnd,
    /// Processing finished for one dimension.
    DimensionEnd,
    /// Finalizing writes.
    Finalize,
    /// Copying miscellaneous (non-MCA) files.
    CopyMisc,
    /// Progress update during misc file copy.
    CopyMiscProgress,
    /// Compressing output to zip archive.
    Compress,
    /// Cleaning up temporary files.
    Cleanup,
    /// Processing complete.
    Done,
}

/// A single event emitted during optimization progress.
#[derive(Debug, Clone)]
pub struct ProgressEvent {
    pub stage: ProgressStage,
    /// Number of items processed so far (for count-based stages).
    pub current: Option<u64>,
    /// Total items expected.
    pub total: Option<u64>,
    /// Related file or directory path.
    pub path: Option<String>,
    /// Human-readable description.
    pub message: Option<String>,
}

/// Receives [`ProgressEvent`]s during optimization.
pub trait ProgressSink: Send + Sync {
    fn emit(&self, event: &ProgressEvent);
}

/// A [`ProgressSink`] that discards all events.
pub struct NoopProgressSink;

impl ProgressSink for NoopProgressSink {
    fn emit(&self, _event: &ProgressEvent) {}
}

/// Aggregates the progress-reporting lifecycle of a single optimization run:
/// the configured sink and emit cadence, the total work estimate, and the
/// thread-safe count of processed items. Thread-safe for use from parallel
/// region worker threads.
pub struct ProgressTracker {
    pub total: u64,
    interval: u64,
    interval_ms: u64,
    sink: Arc<dyn ProgressSink>,
    pub processed: AtomicU64,
    last_emit_time_ms: AtomicU64,
}

impl ProgressTracker {
    pub fn new(total: u64, interval: u64, interval_ms: u64, sink: Arc<dyn ProgressSink>) -> Self {
        Self {
            total,
            interval,
            interval_ms,
            sink,
            processed: AtomicU64::new(0),
            last_emit_time_ms: AtomicU64::new(0),
        }
    }

    /// Forward one progress event to the configured sink.
    pub fn emit(
        &self,
        stage: ProgressStage,
        current: Option<u64>,
        total: Option<u64>,
        path: Option<&Path>,
        message: Option<&str>,
    ) {
        self.sink.emit(&ProgressEvent {
            stage,
            current,
            total: total.or(Some(self.total)),
            path: path.map(|p| p.to_string_lossy().to_string()),
            message: message.map(str::to_string),
        });
    }

    /// Increment the processed counter and, when the throttle allows, emit a
    /// `ChunkProgress` event. Mirrors OrzMCBackup's per-chunk emit: when
    /// `interval_ms > 0` a time-based window is used, otherwise every
    /// `interval` chunks. Returns the new processed count.
    pub fn chunk_progress(&self, path: Option<&Path>) -> u64 {
        let cur = self.processed.fetch_add(1, Ordering::Relaxed) + 1;
        if self.interval_ms > 0 {
            let now = ms_since_epoch();
            let last = self.last_emit_time_ms.load(Ordering::Relaxed);
            if now.saturating_sub(last) >= self.interval_ms
                && self
                    .last_emit_time_ms
                    .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
                    .is_ok()
            {
                self.emit(
                    ProgressStage::ChunkProgress,
                    Some(cur),
                    Some(self.total),
                    path,
                    None,
                );
            }
        } else if self.interval > 0 && cur.is_multiple_of(self.interval) {
            self.emit(
                ProgressStage::ChunkProgress,
                Some(cur),
                Some(self.total),
                path,
                None,
            );
        }
        cur
    }
}

fn ms_since_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Terminal renderer used by the CLI. Global mode renders a percentage (an
/// indicatif bar on a TTY, percent lines otherwise); Region mode prints one
/// line per finished region with its kept/removed counts.
pub struct TermProgressSink {
    mode: ProgressMode,
    tty: bool,
    mp: MultiProgress,
    bar: Mutex<Option<ProgressBar>>,
    last_pct: AtomicUsize,
}

impl TermProgressSink {
    pub fn new(mode: ProgressMode) -> Self {
        Self {
            mode,
            tty: Term::stdout().is_term(),
            mp: MultiProgress::new(),
            bar: Mutex::new(None),
            last_pct: AtomicUsize::new(0),
        }
    }

    /// Lazily build the global bar on first use, once a total is known.
    fn bar_for(&self, total: u64) -> Option<ProgressBar> {
        let mut guard = self.bar.lock().unwrap();
        if guard.is_none() {
            let (_, cols) = Term::stdout().size();
            let reserve = 40u16;
            let mut bar_width = if cols > reserve {
                (cols - reserve) as usize
            } else {
                20usize
            };
            bar_width = bar_width.min(50);
            let pb = self.mp.add(ProgressBar::new(total.max(1)));
            let style = ProgressStyle::with_template(&format!(
                "{{spinner:.green}} {{bar:{width}.cyan/blue}} {{pos}}/{{len}} 区块 {{percent}}% {{msg}}",
                width = bar_width
            ))
            .unwrap()
            .progress_chars("=>-");
            pb.set_style(style);
            *guard = Some(pb);
        }
        guard.clone()
    }
}

impl ProgressSink for TermProgressSink {
    fn emit(&self, e: &ProgressEvent) {
        match self.mode {
            ProgressMode::Off => {}
            ProgressMode::Global => match e.stage {
                ProgressStage::ChunkProgress | ProgressStage::CopyMiscProgress => {
                    let (Some(cur), Some(total)) = (e.current, e.total) else {
                        return;
                    };
                    if self.tty {
                        if let Some(pb) = self.bar_for(total) {
                            pb.set_position(cur);
                        }
                    } else {
                        let pct = (cur * 100 / total.max(1)) as usize;
                        let last = self.last_pct.load(Ordering::Relaxed);
                        if pct > last {
                            self.last_pct.store(pct, Ordering::Relaxed);
                            println!("进度: {}% ({}/{})", pct, cur, total);
                        }
                    }
                }
                ProgressStage::Done => {
                    if let Ok(mut g) = self.bar.try_lock() {
                        if let Some(pb) = g.take() {
                            pb.finish_with_message("已完成");
                        }
                    }
                }
                _ => {}
            },
            ProgressMode::Region => {
                if let (ProgressStage::RegionEnd, Some(path), Some(msg)) =
                    (&e.stage, &e.path, &e.message)
                {
                    println!("{}: {}", path, msg);
                }
            }
        }
    }
}
