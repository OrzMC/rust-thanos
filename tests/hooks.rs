//! Phase 5: the library as a library.
//!
//! Proves the documented hook surface — `on_error` / `report_sink` /
//! `metrics_sink` — and the builder config model work end-to-end on the real
//! filesystem for both the optimizer and the merger. This is the same API the
//! README shows as the "使用为库" example.

use rust_thanos::world::{
    merge, optimize, ErrorKind, MergeRequestBuilder, MetricsSink, OptimizeError, OptimizeReport,
    OptimizerRequestBuilder, ReportSink,
};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct CountingMetrics {
    processed: AtomicU64,
    removed: AtomicU64,
    errors: AtomicU64,
}

impl MetricsSink for CountingMetrics {
    fn record_error(&self, _e: &OptimizeError) {
        self.errors.fetch_add(1, Ordering::Relaxed);
    }
    fn inc_processed(&self, n: u64) {
        self.processed.fetch_add(n, Ordering::Relaxed);
    }
    fn inc_removed(&self, n: u64) {
        self.removed.fetch_add(n, Ordering::Relaxed);
    }
}

/// A [`ReportSink`] that captures the delivered report for inspection.
#[derive(Default)]
struct CaptureReport(Mutex<Option<OptimizeReport>>);

impl ReportSink for CaptureReport {
    fn write(&self, report: &OptimizeReport) -> anyhow::Result<()> {
        *self.0.lock().unwrap() = Some(report.clone());
        Ok(())
    }
}

fn copy_fixture(src: &str, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    let mut opts = fs_extra::dir::CopyOptions::new();
    opts.content_only = true;
    fs_extra::dir::copy(src, dst, &opts).unwrap();
}

fn temp_root(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("rt-h5-{tag}-{}", uuid::Uuid::new_v4()))
}

#[test]
fn optimizer_hooks_end_to_end() {
    let root = temp_root("opt");
    let input = root.join("world");
    copy_fixture("tests/Fixtures/world", &input);
    // Corrupt one region so the engine records an error through the hooks.
    std::fs::write(input.join("region/r.0.0.mca"), vec![0u8; 100]).unwrap();
    let out = root.join("out");

    let seen: Arc<Mutex<Vec<OptimizeError>>> = Arc::new(Mutex::new(Vec::new()));
    let seen_hook = seen.clone();
    let report_sink = Arc::new(CaptureReport::default());
    let metrics = Arc::new(CountingMetrics::default());

    let request = OptimizerRequestBuilder::new(input)
        .output(Some(out))
        .inhabited_threshold_seconds(300)
        .on_error(Arc::new(move |e: &OptimizeError| {
            seen_hook.lock().unwrap().push(e.clone());
        }))
        .report_sink(report_sink.clone())
        .metrics_sink(metrics.clone())
        .build();
    let report = optimize(&request).unwrap();

    // Error recorded in the report and forwarded to both error hooks.
    assert!(!report.errors.is_empty(), "corrupt region recorded");
    let kinds: Vec<ErrorKind> = report.errors.iter().map(|e| e.kind).collect();
    assert!(kinds.contains(&ErrorKind::Mca), "got {:?}", kinds);
    assert_eq!(
        seen.lock().unwrap().len(),
        report.errors.len(),
        "on_error saw every recorded error"
    );
    assert_eq!(
        metrics.errors.load(Ordering::Relaxed),
        report.errors.len() as u64,
        "metrics_sink recorded every error"
    );

    // Report + counters delivered.
    assert!(
        report_sink.0.lock().unwrap().is_some(),
        "report delivered via report_sink"
    );
    assert_eq!(
        metrics.processed.load(Ordering::Relaxed),
        report.processed_chunks
    );
    assert_eq!(
        metrics.removed.load(Ordering::Relaxed),
        report.removed_chunks
    );
    assert!(
        report.processed_chunks > 0,
        "the valid r.-1.0.mca region still processed chunks"
    );
}

#[test]
fn merger_hooks_end_to_end() {
    let root = temp_root("merge");
    let base = root.join("base");
    let patch = root.join("patch");
    copy_fixture("tests/Fixtures/merge/base", &base);
    copy_fixture("tests/Fixtures/merge/patch", &patch);
    // Garbage (>= 8192 B) patch region → read fails → error recorded.
    std::fs::write(
        patch.join("dimensions/minecraft/overworld/region/r.0.0.mca"),
        vec![0xFFu8; 8192],
    )
    .unwrap();
    let out = root.join("out");

    let seen: Arc<Mutex<Vec<OptimizeError>>> = Arc::new(Mutex::new(Vec::new()));
    let seen_hook = seen.clone();
    let report_sink = Arc::new(CaptureReport::default());
    let metrics = Arc::new(CountingMetrics::default());

    let request = MergeRequestBuilder::new(base, patch, out)
        .on_error(Arc::new(move |e: &OptimizeError| {
            seen_hook.lock().unwrap().push(e.clone());
        }))
        .report_sink(report_sink.clone())
        .metrics_sink(metrics.clone())
        .build();
    let report = merge(&request).unwrap();

    assert!(!report.errors.is_empty(), "garbage patch region recorded");
    assert_eq!(
        seen.lock().unwrap().len(),
        report.errors.len(),
        "on_error saw every recorded error"
    );
    assert_eq!(
        metrics.errors.load(Ordering::Relaxed),
        report.errors.len() as u64
    );

    // report_sink receives the lossy-mapped optimizer report.
    let delivered = report_sink
        .0
        .lock()
        .unwrap()
        .clone()
        .expect("report delivered via report_sink");
    assert_eq!(delivered.processed_chunks, report.patch_slots);
    assert_eq!(delivered.removed_chunks, report.base_slots);
    assert_eq!(delivered.errors.len(), report.errors.len());

    assert_eq!(
        metrics.processed.load(Ordering::Relaxed),
        report.patch_slots
    );
    assert_eq!(metrics.removed.load(Ordering::Relaxed), report.base_slots);
}
