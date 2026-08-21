//! World merger (backup restore).
//!
//! Mirrors OrzMCBackup's `WorldMerger.kt`: merges an optimized world (the
//! *patch*: region files holding only the chunks that survived optimization)
//! back onto a full backup (the *base*) at chunk-slot granularity. Every chunk
//! that survived optimization is taken from the patch (it is the newer state);
//! every pruned slot is filled from the base, so no old data is ever lost.
//! entities/poi follow the region merge decision per slot.
//!
//! Pipeline (mirrors `DefaultMerger.run`):
//! 1. OverlapGuard — base/patch/output must be three distinct, non-overlapping
//!    directories, or the run aborts with an `ErrorKind::Input` error.
//! 2. copyTree — full base → output copy, skipping base `.mca` files under
//!    region/entities/poi that the patch also has (the overlay rewrites them).
//!    Orphan patch entities/poi files (no sibling region in patch) never shadow
//!    the base copy.
//! 3. overlayPatch — merge region files at chunk-slot level (patch slot wins,
//!    else base slot, else empty) with entities/poi lockstep. Region files are
//!    independent (each writes its own output), so they merge in parallel on a
//!    rayon pool sized by [`RuntimeOptions::parallelism`] — byte-identical to
//!    serial. A corrupt patch region restores the base file byte-for-byte; a
//!    patch-only region is copied together with its entities/poi siblings.

use crate::world::fs::{FileSystem, RealFileSystem};
use crate::world::io_factory::{McaIOFactory, McaReaderLike, McaWriterLike};
use crate::world::optimizer::{Hooks, IoOptions, OutputOptions, ProgressOptions, RuntimeOptions};
use crate::world::output::ensure_no_overlap;
use crate::world::progress::{ProgressSink, ProgressStage, ProgressTracker};
use crate::world::report::{
    ErrorHook, ErrorKind, MetricsSink, OptimizeError, OptimizeReport, ReportFormat, ReportIO,
    ReportSink,
};
use anyhow::{anyhow, Result};
use log::warn;
use rayon::prelude::*;
use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Configuration for one merge run.
pub struct MergeRequest {
    /// Full world backup (older, complete).
    pub base: PathBuf,
    /// Optimized world to merge back in (newer, partial).
    pub patch: PathBuf,
    /// Destination world directory.
    pub output: PathBuf,
    pub output_options: OutputOptions,
    pub progress: ProgressOptions,
    pub runtime: RuntimeOptions,
    pub hooks: Hooks,
    pub io: IoOptions,
}

/// Builder for [`MergeRequest`] with ergonomic defaults.
pub struct MergeRequestBuilder {
    request: MergeRequest,
}

impl MergeRequestBuilder {
    pub fn new(base: PathBuf, patch: PathBuf, output: PathBuf) -> Self {
        Self {
            request: MergeRequest {
                base,
                patch,
                output,
                output_options: OutputOptions::default(),
                progress: ProgressOptions::default(),
                runtime: RuntimeOptions::default(),
                hooks: Hooks::default(),
                io: IoOptions::default(),
            },
        }
    }

    pub fn force(mut self, v: bool) -> Self {
        self.request.output_options.force = v;
        self
    }
    pub fn progress_mode(mut self, v: crate::world::optimizer::ProgressMode) -> Self {
        self.request.progress.mode = v;
        self
    }
    pub fn progress_interval(mut self, v: u64) -> Self {
        self.request.progress.interval = v;
        self
    }
    pub fn progress_interval_ms(mut self, v: u64) -> Self {
        self.request.progress.interval_ms = v;
        self
    }
    pub fn progress_sink(mut self, v: Arc<dyn ProgressSink>) -> Self {
        self.request.progress.sink = v;
        self
    }
    pub fn parallelism(mut self, v: usize) -> Self {
        self.request.runtime.parallelism = v;
        self
    }
    pub fn sync_on_finalize(mut self, v: bool) -> Self {
        self.request.io.sync_on_finalize = v;
        self
    }
    pub fn fs(mut self, v: Arc<dyn FileSystem>) -> Self {
        self.request.io.fs = v;
        self
    }
    pub fn io_factory(mut self, v: Arc<dyn McaIOFactory>) -> Self {
        self.request.io.io_factory = v;
        self
    }
    pub fn on_error(mut self, v: ErrorHook) -> Self {
        self.request.hooks.on_error = Some(v);
        self
    }
    pub fn report_sink(mut self, v: Arc<dyn ReportSink>) -> Self {
        self.request.hooks.report_sink = Some(v);
        self
    }
    pub fn metrics_sink(mut self, v: Arc<dyn MetricsSink>) -> Self {
        self.request.hooks.metrics_sink = Some(v);
        self
    }
    pub fn build(self) -> MergeRequest {
        self.request
    }
}

/// Summary report for a merge run.
#[derive(Debug, Clone, Default)]
pub struct MergeReport {
    /// Region files merged at slot level (including corrupt-patch base restores).
    pub merged_regions: u64,
    /// Patch-only region/entities/poi files copied verbatim.
    pub copied_files: u64,
    /// Slots taken from the patch (newer data wins).
    pub patch_slots: u64,
    /// Slots filled from the base backup.
    pub base_slots: u64,
    /// entities entries written in lockstep.
    pub linked_entities: u64,
    /// poi entries written in lockstep.
    pub linked_poi: u64,
    /// Non-MCA patch files overlaid onto the base copy.
    pub overlay_files: u64,
    /// Non-fatal errors collected during the run.
    pub errors: Vec<OptimizeError>,
}

/// Engine interface (injectable/mockable).
pub trait MergeEngine {
    fn run(&self, request: &MergeRequest) -> Result<MergeReport>;
}

/// Convenience entry point.
pub fn merge(request: &MergeRequest) -> Result<MergeReport> {
    DefaultMerger.run(request)
}

/// Default merge engine.
pub struct DefaultMerger;

impl MergeEngine for DefaultMerger {
    fn run(&self, request: &MergeRequest) -> Result<MergeReport> {
        let fs = request.io.fs.clone();
        let base = request.base.clone();
        let patch = request.patch.clone();
        let out = request.output.clone();

        let shared = MergeShared {
            fs: fs.clone(),
            factory: request.io.io_factory.clone(),
            progress: Arc::new(ProgressTracker::new(
                0,
                request.progress.interval,
                request.progress.interval_ms,
                request.progress.sink.clone(),
            )),
            errors: Arc::new(Mutex::new(Vec::new())),
            on_error: request.hooks.on_error.clone(),
            metrics: request.hooks.metrics_sink.clone(),
            counters: MergeCounters::default(),
        };

        // Guard against aliasing before any write: an overlapping base/patch/output
        // could wipe or corrupt the source worlds (e.g. output == base with --force).
        if overlaps(fs.as_ref(), &base, &out)
            || overlaps(fs.as_ref(), &patch, &out)
            || overlaps(fs.as_ref(), &base, &patch)
        {
            shared.record(
                &base,
                ErrorKind::Input,
                "base, patch and output must be three distinct, non-overlapping directories",
            );
            return Ok(MergeReport {
                errors: shared.errors.lock().unwrap().clone(),
                ..Default::default()
            });
        }
        if !fs.is_directory(&base) || !fs.is_directory(&patch) {
            shared.record(
                &base,
                ErrorKind::Input,
                "base/patch must be existing directories",
            );
            return Ok(MergeReport {
                errors: shared.errors.lock().unwrap().clone(),
                ..Default::default()
            });
        }
        let Some(out_dir) = resolve_output_dir(&shared, request) else {
            return Ok(MergeReport {
                errors: shared.errors.lock().unwrap().clone(),
                ..Default::default()
            });
        };
        shared.progress.emit(
            ProgressStage::Init,
            Some(0),
            Some(0),
            Some(&base),
            Some("starting merge"),
        );

        // 1. Copy the full backup to the output, so every chunk from the base is
        //    preserved. Skip base .mca files under region/entities/poi that the
        //    patch also has: the overlay rewrites them anyway.
        let (patch_mca_rels, _patch_region_rels) = collect_patch_mca(fs.as_ref(), &patch);
        copy_tree(fs.as_ref(), &base, &out_dir, &patch_mca_rels, &|p, k, m| {
            shared.record(p, k, m)
        });

        // 2. Overlay the optimized backup: merge region/entities/poi at chunk-slot
        //    level, replace every other file with the (newer) patch copy.
        overlay_patch(
            &shared,
            &base,
            &patch,
            &out_dir,
            request.runtime.parallelism,
            request.io.sync_on_finalize,
        );

        let lock = out_dir.join("session.lock");
        if fs.exists(&lock) {
            let _ = fs.delete_if_exists(&lock);
        }
        shared.progress.emit(
            ProgressStage::Done,
            Some(shared.counters.merged_regions.load(Ordering::Relaxed)),
            Some(0),
            Some(&out_dir),
            Some("merge complete"),
        );

        let report = MergeReport {
            merged_regions: shared.counters.merged_regions.load(Ordering::Relaxed),
            copied_files: shared.counters.copied_files.load(Ordering::Relaxed),
            patch_slots: shared.counters.patch_slots.load(Ordering::Relaxed),
            base_slots: shared.counters.base_slots.load(Ordering::Relaxed),
            linked_entities: shared.counters.linked_entities.load(Ordering::Relaxed),
            linked_poi: shared.counters.linked_poi.load(Ordering::Relaxed),
            overlay_files: shared.counters.overlay_files.load(Ordering::Relaxed),
            errors: shared.errors.lock().unwrap().clone(),
        };

        if let Some(sink) = &request.hooks.report_sink {
            if let Err(e) = sink.write(&MergeReportIO::to_optimize_report(&report)) {
                eprintln!("Failed to write report: {}", e);
            }
        }
        if let Some(m) = &shared.metrics {
            m.inc_processed(report.patch_slots);
            m.inc_removed(report.base_slots);
        }
        Ok(report)
    }
}

/// State shared across parallel region-merge tasks. Sync: it holds only `Arc`s,
/// atomics and mutexes, so rayon tasks can share it; the per-task readers and
/// writers (`Send` but not `Sync`) stay local to each task.
struct MergeShared {
    fs: Arc<dyn FileSystem>,
    factory: Arc<dyn McaIOFactory>,
    progress: Arc<ProgressTracker>,
    errors: Arc<Mutex<Vec<OptimizeError>>>,
    on_error: Option<ErrorHook>,
    metrics: Option<Arc<dyn MetricsSink>>,
    counters: MergeCounters,
}

impl MergeShared {
    fn record(&self, path: &Path, kind: ErrorKind, msg: &str) {
        let e = OptimizeError {
            path: path.to_string_lossy().to_string(),
            kind,
            message: msg.to_string(),
        };
        if let Some(f) = &self.on_error {
            f(&e);
        }
        if let Some(m) = &self.metrics {
            m.record_error(&e);
        }
        warn!("[{}] {}: {}", kind, path.display(), msg);
        self.errors.lock().unwrap().push(e);
    }
}

#[derive(Default)]
struct MergeCounters {
    merged_regions: AtomicU64,
    copied_files: AtomicU64,
    patch_slots: AtomicU64,
    base_slots: AtomicU64,
    linked_entities: AtomicU64,
    linked_poi: AtomicU64,
    overlay_files: AtomicU64,
}

/// True when two paths alias each other (same dir, or one nested in the other).
fn overlaps(fs: &dyn FileSystem, a: &Path, b: &Path) -> bool {
    if fs.as_any().downcast_ref::<RealFileSystem>().is_some() {
        ensure_no_overlap(a, Some(b)).is_err()
    } else {
        // Memory/other filesystems have no canonicalize; compare raw paths.
        a == b || a.starts_with(b) || b.starts_with(a)
    }
}

/// Prepare the output directory: clear it when `--force` and non-empty, reject
/// a non-empty output otherwise, then create it.
fn resolve_output_dir(shared: &MergeShared, request: &MergeRequest) -> Option<PathBuf> {
    let fs = shared.fs.clone();
    let out = &request.output;
    if fs.exists(out) {
        let non_empty = fs.list(out).map(|v| !v.is_empty()).unwrap_or(false);
        if non_empty {
            if !request.output_options.force {
                shared.record(
                    out,
                    ErrorKind::Output,
                    "Output directory already exists and is not empty; use --force to overwrite",
                );
                return None;
            }
            if let Err(e) = fs.delete_tree(out) {
                shared.record(
                    out,
                    ErrorKind::Output,
                    &format!("Failed to clear output directory: {}", e),
                );
                return None;
            }
        }
    }
    match fs.create_directories(out) {
        Ok(()) => Some(out.clone()),
        Err(e) => {
            shared.record(
                out,
                ErrorKind::Output,
                &format!("Output directory is not writable: {}", e),
            );
            None
        }
    }
}

/// Relative `.mca` paths in the patch under region/entities/poi. Region paths go
/// into the second set so orphan entities/poi (region absent in patch) can be
/// detected: those must NOT skip the base copy, or base data would be lost.
fn collect_patch_mca(fs: &dyn FileSystem, patch: &Path) -> (HashSet<PathBuf>, HashSet<PathBuf>) {
    let mut patch_mca_rels = HashSet::new();
    let mut patch_region_rels = HashSet::new();
    let Ok(walked) = fs.walk(patch, false) else {
        return (patch_mca_rels, patch_region_rels);
    };
    for p in walked {
        if p == patch || !fs.is_regular_file(&p) {
            continue;
        }
        if p.file_name().and_then(|s| s.to_str()) == Some("session.lock") {
            continue;
        }
        let rel = p.strip_prefix(patch).unwrap_or(&p);
        let kind = rel
            .parent()
            .and_then(|x| x.file_name())
            .and_then(|s| s.to_str())
            .map(str::to_string);
        let is_mca = rel
            .file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.ends_with(".mca"));
        if matches!(kind.as_deref(), Some("region" | "entities" | "poi")) && is_mca {
            if kind.as_deref() == Some("region") {
                patch_region_rels.insert(rel.to_path_buf());
            }
            patch_mca_rels.insert(rel.to_path_buf());
        }
    }
    // Entities/poi are rewritten by mergeRegion only when the sibling region exists
    // in patch. Drop the skip for an orphan patch entities/poi file so the base
    // copy of that file is preserved.
    patch_mca_rels.retain(|rel| {
        let Some(kind) = rel
            .parent()
            .and_then(|x| x.file_name())
            .and_then(|s| s.to_str())
        else {
            return true;
        };
        if kind == "region" {
            return true;
        }
        let name = rel.file_name().unwrap();
        let region_sibling = rel
            .parent()
            .and_then(|p| p.parent())
            .map(|dim| dim.join("region").join(name));
        region_sibling.is_some_and(|s| patch_region_rels.contains(&s))
    });
    (patch_mca_rels, patch_region_rels)
}

/// Copy the base tree to `dst`, skipping `skip_rels` (base-relative paths the
/// overlay rewrites) and `session.lock`. Per-file failures are recorded, not
/// thrown. Mirrors OrzMCBackup's `copyTree`.
fn copy_tree(
    fs: &dyn FileSystem,
    src: &Path,
    dst: &Path,
    skip_rels: &HashSet<PathBuf>,
    record: &dyn Fn(&Path, ErrorKind, &str),
) {
    if let Err(e) = fs.create_directories(dst) {
        record(
            src,
            ErrorKind::Copy,
            &format!("Failed to create output directory: {}", e),
        );
    }
    let Ok(walked) = fs.walk(src, false) else {
        return;
    };
    for p in walked {
        if p == src {
            continue;
        }
        let rel = p.strip_prefix(src).unwrap_or(&p);
        let target = dst.join(rel);
        if fs.is_directory(&p) {
            if let Err(e) = fs.create_directories(&target) {
                record(
                    &p,
                    ErrorKind::Copy,
                    &format!("Failed to create directory: {}", e),
                );
            }
        } else if p.file_name().and_then(|s| s.to_str()) == Some("session.lock") {
            // Never copied (a stale lock must not leak into a fresh output).
        } else if skip_rels.contains(rel) {
            // Rewritten by the overlay; skip the wasted intermediate copy.
        } else {
            if let Some(parent) = target.parent() {
                if let Err(e) = fs.create_directories(parent) {
                    record(
                        &p,
                        ErrorKind::Copy,
                        &format!("Failed to create directory: {}", e),
                    );
                }
            }
            if let Err(e) = fs.copy(&p, &target, true) {
                record(&p, ErrorKind::Copy, &format!("Failed to copy file: {}", e));
            }
        }
    }
}

/// Overlay the patch onto the base copy in `out`: merge region/entities/poi at
/// slot level (parallel when `parallelism > 1`), then overlay every other patch
/// file (misc) onto the base copy. Mirrors OrzMCBackup's `overlayPatch`.
fn overlay_patch(
    shared: &MergeShared,
    base: &Path,
    patch: &Path,
    out: &Path,
    parallelism: usize,
    sync_on_finalize: bool,
) {
    let fs = shared.fs.clone();
    let Ok(walked) = fs.walk(patch, false) else {
        return;
    };
    let files: Vec<PathBuf> = walked
        .into_iter()
        .filter(|p| {
            p != patch
                && fs.is_regular_file(p)
                && p.file_name().and_then(|s| s.to_str()) != Some("session.lock")
        })
        .collect();
    let total = files.len() as u64;
    shared.progress.emit(
        ProgressStage::CopyMisc,
        Some(0),
        Some(total),
        Some(patch),
        Some("overlaying patch"),
    );

    let region_files: Vec<PathBuf> = files
        .iter()
        .filter(|p| {
            let rel = p.strip_prefix(patch).unwrap_or(p);
            rel.parent()
                .and_then(|x| x.file_name())
                .and_then(|s| s.to_str())
                == Some("region")
                && rel
                    .file_name()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s.ends_with(".mca"))
        })
        .cloned()
        .collect();
    let region_set: HashSet<PathBuf> = region_files.iter().cloned().collect();
    let misc_files: Vec<PathBuf> = files
        .iter()
        .filter(|p| !region_set.contains(*p))
        .cloned()
        .collect();
    // CopyMiscProgress reports only the misc overlay, so its total is the misc
    // count — otherwise the final percentage never reaches 100%.
    let misc_total = misc_files.len() as u64;

    let region_count = region_files.len();
    if parallelism > 1 && region_count > 1 {
        let par_parallelism = parallelism;
        match rayon::ThreadPoolBuilder::new()
            .num_threads(par_parallelism)
            .build()
        {
            Ok(pool) => {
                pool.install(|| {
                    region_files.par_iter().for_each(|p| {
                        process_region(shared, base, patch, out, p, sync_on_finalize);
                    });
                });
            }
            Err(e) => {
                shared.record(
                    patch,
                    ErrorKind::Parallel,
                    &format!(
                        "Failed to build thread pool, falling back to serial merge: {}",
                        e
                    ),
                );
                for p in &region_files {
                    process_region(shared, base, patch, out, p, sync_on_finalize);
                }
            }
        }
    } else {
        for p in &region_files {
            process_region(shared, base, patch, out, p, sync_on_finalize);
        }
    }

    let mut done = 0u64;
    for p in &misc_files {
        let rel = p.strip_prefix(patch).unwrap_or(p);
        let kind = rel
            .parent()
            .and_then(|x| x.file_name())
            .and_then(|s| s.to_str());
        let is_mca_under_reserved = matches!(kind, Some("region" | "entities" | "poi"))
            && rel
                .file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.ends_with(".mca"));
        if !is_mca_under_reserved {
            copy_file(shared, p, &out.join(rel), false);
        }
        done += 1;
        if done.is_multiple_of(1000) {
            shared.progress.emit(
                ProgressStage::CopyMiscProgress,
                Some(done),
                Some(misc_total),
                Some(p),
                None,
            );
        }
    }
    shared.progress.emit(
        ProgressStage::CopyMiscProgress,
        Some(done),
        Some(misc_total),
        Some(patch),
        None,
    );
}

/// One patch region: merge it slot-by-slot against base when base has it,
/// otherwise copy the region plus its entities/poi siblings verbatim.
fn process_region(
    shared: &MergeShared,
    base: &Path,
    patch: &Path,
    out: &Path,
    p: &Path,
    sync_on_finalize: bool,
) {
    let fs = shared.fs.clone();
    let rel = p.strip_prefix(patch).unwrap_or(p);
    let base_file = base.join(rel);
    let out_file = out.join(rel);
    if fs.is_regular_file(&base_file) {
        if let Err(e) = merge_region(shared, &base_file, p, &out_file, sync_on_finalize) {
            shared.record(
                p,
                ErrorKind::Mca,
                &format!("Failed to merge region file: {}", e),
            );
        }
    } else {
        // Patch-only region: copy the region plus its entities/poi siblings.
        // `rel` is region/<name> in the flat layout or dimensions/.../region/<name>
        // in the nested layout; the parent of "region" is the dimension dir.
        copy_file(shared, p, &out_file, true);
        if let (Some(name), Some(dim_dir)) =
            (rel.file_name(), rel.parent().and_then(|x| x.parent()))
        {
            let ent_rel = dim_dir.join("entities").join(name);
            let poi_rel = dim_dir.join("poi").join(name);
            if fs.is_regular_file(&patch.join(&ent_rel)) {
                copy_file(shared, &patch.join(&ent_rel), &out.join(&ent_rel), true);
            }
            if fs.is_regular_file(&patch.join(&poi_rel)) {
                copy_file(shared, &patch.join(&poi_rel), &out.join(&poi_rel), true);
            }
        }
    }
    shared.progress.emit(
        ProgressStage::RegionEnd,
        Some(shared.counters.merged_regions.load(Ordering::Relaxed)),
        None,
        Some(p),
        Some("合并完成"),
    );
}

/// Merge one region at chunk-slot granularity: patch slot wins, else base slot,
/// else empty; entities/poi follow the region decision per slot (lockstep).
///
/// - A corrupt patch region restores the base copy byte-for-byte so nothing is
///   dropped (the base copy was skipped by copyTree for common files).
/// - A patch-only entities/poi result drops the stale base copy left by copyTree.
fn merge_region(
    shared: &MergeShared,
    base_region: &Path,
    patch_region: &Path,
    out_region: &Path,
    sync_on_finalize: bool,
) -> Result<()> {
    let fs = shared.fs.clone();
    let name = out_region
        .file_name()
        .ok_or_else(|| anyhow!("invalid output region path"))?;
    let out_dim = out_region.parent().and_then(|p| p.parent());
    let base_dim = base_region.parent().and_then(|p| p.parent());
    let patch_dim = patch_region.parent().and_then(|p| p.parent());
    let (Some(out_dim), Some(base_dim), Some(patch_dim)) = (out_dim, base_dim, patch_dim) else {
        return Ok(());
    };
    let out_entities = out_dim.join("entities").join(name);
    let out_poi = out_dim.join("poi").join(name);

    // base may lack an entities/poi directory entirely; create the output
    // parents so the lazy writers can place siblings there.
    for d in [out_region.parent(), out_entities.parent(), out_poi.parent()]
        .into_iter()
        .flatten()
    {
        if let Err(e) = fs.create_directories(d) {
            shared.record(
                out_region,
                ErrorKind::Write,
                &format!("Failed to create output directories: {}", e),
            );
            return Ok(());
        }
    }

    let mut crb = open_reader(shared, base_region, ErrorKind::Mca);
    let crp = open_reader(shared, patch_region, ErrorKind::Mca);
    // Patch region must be readable. If it is corrupt, restore the base copy
    // (skipped by copyTree for common files) byte-for-byte so nothing is dropped.
    if crp.is_none() {
        copy_base_if_present(shared, base_region, out_region, ErrorKind::Mca);
        copy_base_if_present(
            shared,
            &base_dim.join("entities").join(name),
            &out_entities,
            ErrorKind::Entities,
        );
        copy_base_if_present(
            shared,
            &base_dim.join("poi").join(name),
            &out_poi,
            ErrorKind::Poi,
        );
        shared
            .counters
            .merged_regions
            .fetch_add(1, Ordering::Relaxed);
        return Ok(());
    }
    let mut crp = crp.unwrap();
    let mut erb = if crb.is_some() {
        open_reader(
            shared,
            &base_dim.join("entities").join(name),
            ErrorKind::Entities,
        )
    } else {
        None
    };
    let mut erp = open_reader(
        shared,
        &patch_dim.join("entities").join(name),
        ErrorKind::Entities,
    );
    let mut prb = if crb.is_some() {
        open_reader(shared, &base_dim.join("poi").join(name), ErrorKind::Poi)
    } else {
        None
    };
    let mut prp = open_reader(shared, &patch_dim.join("poi").join(name), ErrorKind::Poi);

    // Writers are created lazily on first write; a fully-empty entities/poi
    // lockstep result never creates a file (and the stale base copy is removed).
    let mut cw: Option<Box<dyn McaWriterLike>> = None;
    let mut ew: Option<Box<dyn McaWriterLike>> = None;
    let mut pw: Option<Box<dyn McaWriterLike>> = None;

    for i in 0..1024 {
        let pe = crp
            .get(i)
            .map_err(|e| anyhow!("patch get({}) failed: {}", i, e))?;
        let be = match crb.as_mut() {
            Some(r) => r
                .get(i)
                .map_err(|e| anyhow!("base get({}) failed: {}", i, e))?,
            None => None,
        };
        if let Some(mut pe) = pe {
            {
                let w = lazy_writer(shared, &mut cw, out_region, sync_on_finalize)?;
                w.write_entry(&mut pe)
                    .map_err(|e| anyhow!("Failed to write chunk entry: {}", e))?;
            }
            shared.counters.patch_slots.fetch_add(1, Ordering::Relaxed);
            if let Some(erp) = erp.as_mut() {
                match erp.get(i) {
                    Ok(Some(mut e)) => {
                        let w = lazy_writer(shared, &mut ew, &out_entities, sync_on_finalize)?;
                        w.write_entry(&mut e)
                            .map_err(|e| anyhow!("Failed to write entities entry: {}", e))?;
                        shared
                            .counters
                            .linked_entities
                            .fetch_add(1, Ordering::Relaxed);
                    }
                    Ok(None) => {}
                    Err(e) => {
                        shared.record(
                            &out_entities,
                            ErrorKind::Entities,
                            &format!("Failed to read entities entry {}: {}", i, e),
                        );
                    }
                }
            }
            if let Some(prp) = prp.as_mut() {
                match prp.get(i) {
                    Ok(Some(mut e)) => {
                        let w = lazy_writer(shared, &mut pw, &out_poi, sync_on_finalize)?;
                        w.write_entry(&mut e)
                            .map_err(|e| anyhow!("Failed to write poi entry: {}", e))?;
                        shared.counters.linked_poi.fetch_add(1, Ordering::Relaxed);
                    }
                    Ok(None) => {}
                    Err(e) => {
                        shared.record(
                            &out_poi,
                            ErrorKind::Poi,
                            &format!("Failed to read poi entry {}: {}", i, e),
                        );
                    }
                }
            }
        } else if let Some(mut be) = be {
            {
                let w = lazy_writer(shared, &mut cw, out_region, sync_on_finalize)?;
                w.write_entry(&mut be)
                    .map_err(|e| anyhow!("Failed to write chunk entry: {}", e))?;
            }
            shared.counters.base_slots.fetch_add(1, Ordering::Relaxed);
            if let Some(erb) = erb.as_mut() {
                match erb.get(i) {
                    Ok(Some(mut e)) => {
                        let w = lazy_writer(shared, &mut ew, &out_entities, sync_on_finalize)?;
                        w.write_entry(&mut e)
                            .map_err(|e| anyhow!("Failed to write entities entry: {}", e))?;
                        shared
                            .counters
                            .linked_entities
                            .fetch_add(1, Ordering::Relaxed);
                    }
                    Ok(None) => {}
                    Err(e) => {
                        shared.record(
                            &out_entities,
                            ErrorKind::Entities,
                            &format!("Failed to read entities entry {}: {}", i, e),
                        );
                    }
                }
            }
            if let Some(prb) = prb.as_mut() {
                match prb.get(i) {
                    Ok(Some(mut e)) => {
                        let w = lazy_writer(shared, &mut pw, &out_poi, sync_on_finalize)?;
                        w.write_entry(&mut e)
                            .map_err(|e| anyhow!("Failed to write poi entry: {}", e))?;
                        shared.counters.linked_poi.fetch_add(1, Ordering::Relaxed);
                    }
                    Ok(None) => {}
                    Err(e) => {
                        shared.record(
                            &out_poi,
                            ErrorKind::Poi,
                            &format!("Failed to read poi entry {}: {}", i, e),
                        );
                    }
                }
            }
        }
    }

    let finalize = |slot: &mut Option<Box<dyn McaWriterLike>>, path: &Path, kind: ErrorKind| {
        if let Some(w) = slot.as_mut() {
            if let Err(e) = w.finalize_file() {
                shared.record(path, kind, &format!("Failed to finalize write: {}", e));
            }
        }
    };
    finalize(&mut cw, out_region, ErrorKind::Write);
    finalize(&mut ew, &out_entities, ErrorKind::Write);
    finalize(&mut pw, &out_poi, ErrorKind::Write);
    // If the lockstep produced no entities/poi, drop the stale base copy left by
    // copyTree.
    if ew.is_none() {
        let _ = fs.delete_if_exists(&out_entities);
    }
    if pw.is_none() {
        let _ = fs.delete_if_exists(&out_poi);
    }
    shared
        .counters
        .merged_regions
        .fetch_add(1, Ordering::Relaxed);
    Ok(())
}

/// Get the lazy writer for `slot`, creating it on first use.
fn lazy_writer<'a>(
    shared: &MergeShared,
    slot: &'a mut Option<Box<dyn McaWriterLike>>,
    path: &Path,
    sync_on_finalize: bool,
) -> Result<&'a mut Box<dyn McaWriterLike>> {
    if slot.is_none() {
        let w = shared
            .factory
            .create_writer(shared.fs.clone(), path, sync_on_finalize)
            .map_err(|e| anyhow!("Failed to create writer {}: {}", path.display(), e))?;
        *slot = Some(w);
    }
    Ok(slot.as_mut().unwrap())
}

/// Open a reader for `path` when it is a valid (≥ 8 KiB) MCA file; `None`
/// otherwise (an absent or invalid counterpart is not an error).
fn open_reader(
    shared: &MergeShared,
    path: &Path,
    kind: ErrorKind,
) -> Option<Box<dyn McaReaderLike>> {
    let fs = shared.fs.clone();
    let factory = shared.factory.clone();
    if !is_valid_mca(fs.as_ref(), path) {
        return None;
    }
    match factory.open_reader(fs, path) {
        Ok(r) => Some(r),
        Err(e) => {
            shared.record(path, kind, &format!("Failed to read MCA file: {}", e));
            None
        }
    }
}

/// Copy one file (byte-for-byte), recording failures rather than aborting.
fn copy_file(shared: &MergeShared, src: &Path, dst: &Path, copied: bool) {
    let fs = shared.fs.clone();
    match (|| -> Result<()> {
        if let Some(parent) = dst.parent() {
            fs.create_directories(parent)?;
        }
        fs.copy(src, dst, true)?;
        Ok(())
    })() {
        Ok(()) => {
            if copied {
                shared.counters.copied_files.fetch_add(1, Ordering::Relaxed);
            } else {
                shared
                    .counters
                    .overlay_files
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
        Err(e) => shared.record(src, ErrorKind::Copy, &format!("Failed to copy file: {}", e)),
    }
}

/// Copy `src` to `dst` when the source file exists (used to restore the base
/// copy of a corrupt patch region).
fn copy_base_if_present(shared: &MergeShared, src: &Path, dst: &Path, kind: ErrorKind) {
    let fs = shared.fs.clone();
    if !fs.is_regular_file(src) {
        return;
    }
    match (|| -> Result<()> {
        if let Some(parent) = dst.parent() {
            fs.create_directories(parent)?;
        }
        fs.copy(src, dst, true)?;
        Ok(())
    })() {
        Ok(()) => {}
        Err(e) => shared.record(src, kind, &format!("Failed to copy file: {}", e)),
    }
}

/// True when `path` is a readable region MCA at least 8192 bytes (two header
/// sectors) long. Silent — callers decide whether an invalid file is an error
/// or merely an absent counterpart.
fn is_valid_mca(fs: &dyn FileSystem, path: &Path) -> bool {
    fs.is_regular_file(path) && fs.size(path) >= 8192
}

/// Serializes [`MergeReport`] to JSON, CSV, or plain text, mirroring
/// OrzMCBackup's `MergeReportIO`.
pub struct MergeReportIO;

impl MergeReportIO {
    /// Maps merge counters into the [`OptimizeReport`] shape for consumers that
    /// expect the optimizer's report model (e.g. `ReportSink`). Lossy:
    /// `processedChunks` = patch slots, `removedChunks` = base slots.
    pub fn to_optimize_report(r: &MergeReport) -> OptimizeReport {
        OptimizeReport {
            processed_chunks: r.patch_slots,
            removed_chunks: r.base_slots,
            kept_chunks: 0,
            before_size: 0,
            after_size: 0,
            duration: 0,
            errors: r.errors.clone(),
        }
    }

    pub fn to_json(r: &MergeReport) -> String {
        let mut sb = String::new();
        sb.push_str(&format!(
            "{{\"mergedRegions\":{},\"copiedFiles\":{},\"patchSlots\":{},\"baseSlots\":{},\"linkedEntities\":{},\"linkedPoi\":{},\"overlayFiles\":{},\"errors\":[",
            r.merged_regions,
            r.copied_files,
            r.patch_slots,
            r.base_slots,
            r.linked_entities,
            r.linked_poi,
            r.overlay_files,
        ));
        for (i, e) in r.errors.iter().enumerate() {
            if i > 0 {
                sb.push(',');
            }
            sb.push_str(&format!(
                "{{\"path\":\"{}\",\"kind\":\"{}\",\"message\":\"{}\"}}",
                ReportIO::esc(&e.path),
                ReportIO::esc(e.kind.as_str()),
                ReportIO::esc(&e.message),
            ));
        }
        sb.push_str("]}");
        sb
    }

    pub fn to_csv(r: &MergeReport) -> String {
        let mut sb = String::new();
        sb.push_str("mergedRegions,copiedFiles,patchSlots,baseSlots,linkedEntities,linkedPoi,overlayFiles,errorsCount\n");
        sb.push_str(&format!(
            "{},{},{},{},{},{},{},{}\n",
            r.merged_regions,
            r.copied_files,
            r.patch_slots,
            r.base_slots,
            r.linked_entities,
            r.linked_poi,
            r.overlay_files,
            r.errors.len(),
        ));
        sb.push_str("path,kind,message\n");
        for e in &r.errors {
            let path = e.path.replace('"', "\"\"");
            let kind = e.kind.as_str().replace('"', "\"\"");
            let message = e.message.replace('"', "\"\"");
            sb.push_str(&format!("\"{}\",\"{}\",\"{}\"\n", path, kind, message));
        }
        sb
    }

    pub fn to_text(r: &MergeReport) -> String {
        let mut sb = String::new();
        sb.push_str(&format!(
            "Statistics: mergedRegions={} patchSlots={} baseSlots={} linkedEntities={} linkedPoi={} copiedFiles={} overlayFiles={} errors={}\n",
            r.merged_regions,
            r.patch_slots,
            r.base_slots,
            r.linked_entities,
            r.linked_poi,
            r.copied_files,
            r.overlay_files,
            r.errors.len(),
        ));
        if !r.errors.is_empty() {
            sb.push_str("Error list:\n");
            for e in &r.errors {
                sb.push_str(&format!("[{}] {} - {}\n", e.kind, e.path, e.message));
            }
        }
        sb.trim_end().to_string()
    }

    /// Write the report to `path` in the given format, creating parent
    /// directories as needed.
    pub fn write(r: &MergeReport, path: &Path, format: ReportFormat) -> Result<()> {
        let content = match format {
            ReportFormat::Json => Self::to_json(r),
            ReportFormat::Csv => Self::to_csv(r),
            ReportFormat::Text => Self::to_text(r),
        };
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                fs::create_dir_all(parent)?;
            }
        }
        let mut f = fs::File::create(path)?;
        f.write_all(content.as_bytes())?;
        f.write_all(b"\n")?;
        Ok(())
    }
}
