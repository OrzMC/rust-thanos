//! Optimizer engine.
//!
//! The library entry point: a [`OptimizerRequest`] config model (mirroring
//! OrzMCBackup's `OptimizerRequest`) is run by [`DefaultOptimizer`], which
//! orchestrates dimension discovery → counting → per-dimension region
//! processing → in-place replacement, all through the [`FileSystem`] and
//! [`McaIOFactory`] abstractions so the same pipeline runs on real files or in
//! memory.
//!
//! Parallelism is single-level (mirroring OrzMCBackup A3): dimensions are
//! processed serially, and the region files within each dimension run in
//! parallel on a rayon pool sized by [`RuntimeOptions::parallelism`]. Every
//! region writes its own independent output files, so the result is
//! byte-identical regardless of pool size or scheduling order.

use crate::patterns::inhabited::InhabitedTimePattern;
use crate::patterns::list::ListPattern;
use crate::patterns::ChunkPattern;
use crate::world::forceload::parse_force_loaded_fs;
use crate::world::fs::{FileSystem, RealFileSystem};
use crate::world::io_factory::{DefaultMcaIOFactory, McaIOFactory, McaReaderLike, McaWriterLike};
use crate::world::misc::{copy_misc_files, discover_misc_parents};
use crate::world::output::ensure_no_overlap;
use crate::world::progress::{ProgressSink, ProgressStage, ProgressTracker};
use crate::world::report::{
    ErrorHook, ErrorKind, MetricsSink, OptimizeError, OptimizeReport, ReportSink,
};
use crate::world::zip::compress_to_timestamp_zip;
use anyhow::{anyhow, Result};
use clap::ValueEnum;
use log::{info, warn};
use rayon::prelude::*;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// How much per-chunk progress detail to render.
#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ProgressMode {
    Off,
    Global,
    Region,
}

/// Chunk-selection options.
#[derive(Clone)]
pub struct FilterOptions {
    /// InhabitedTime threshold in **seconds** (1s = 20 ticks). The engine
    /// multiplies by 20 unless [`inhabited_threshold_ticks`](Self::inhabited_threshold_ticks)
    /// overrides it.
    pub inhabited_threshold_seconds: i64,
    /// Tick-exact override for the InhabitedTime threshold. Library/legacy
    /// callers that already hold a tick value use this to avoid the ×20
    /// conversion losing precision (e.g. a 27-tick boundary).
    pub inhabited_threshold_ticks: Option<i64>,
    /// Treat unknown/external-compressed chunks as removable.
    pub remove_unknown: bool,
    /// Fail hard (exit non-zero) when errors are recorded.
    pub strict: bool,
    /// Extra keep-patterns appended after the built-in force-load + inhabited
    /// patterns (library injection point; e.g. `--keep-range`).
    pub patterns: Vec<Box<dyn ChunkPattern>>,
}

impl Default for FilterOptions {
    fn default() -> Self {
        Self {
            inhabited_threshold_seconds: 300,
            inhabited_threshold_ticks: None,
            remove_unknown: false,
            strict: false,
            patterns: Vec::new(),
        }
    }
}

/// Output-related options.
#[derive(Clone)]
pub struct OutputOptions {
    pub in_place: bool,
    pub zip_output: bool,
    pub force: bool,
    pub copy_misc: bool,
    pub dry_run: bool,
}

impl Default for OutputOptions {
    fn default() -> Self {
        Self {
            in_place: false,
            zip_output: false,
            force: false,
            copy_misc: true,
            dry_run: false,
        }
    }
}

/// Progress display options.
#[derive(Clone)]
pub struct ProgressOptions {
    pub mode: ProgressMode,
    /// Emit an update every N chunks (0 disables count-based emission).
    pub interval: u64,
    /// Emit an update every N milliseconds (0 disables time-based emission).
    pub interval_ms: u64,
    /// The event sink. Library default is [`NoopProgressSink`]; the CLI
    /// injects a terminal renderer.
    pub sink: Arc<dyn ProgressSink>,
}

impl Default for ProgressOptions {
    fn default() -> Self {
        Self {
            mode: ProgressMode::Region,
            interval: 1000,
            interval_ms: 0,
            sink: Arc::new(crate::world::progress::NoopProgressSink),
        }
    }
}

/// Runtime options.
#[derive(Clone, Copy, Default)]
pub struct RuntimeOptions {
    /// Region-parallelism. 0 means auto (`available_parallelism`).
    pub parallelism: usize,
}

/// Callback hooks.
#[derive(Default)]
pub struct Hooks {
    /// Invoked for every recorded (non-fatal) error.
    pub on_error: Option<ErrorHook>,
    /// Receives the final report (e.g. `FileReportSink` for `--report-file`).
    pub report_sink: Option<Arc<dyn ReportSink>>,
    /// Receives aggregated counters and error notifications.
    pub metrics_sink: Option<Arc<dyn MetricsSink>>,
}

/// IO plumbing: filesystem + MCA factory.
pub struct IoOptions {
    pub fs: Arc<dyn FileSystem>,
    pub io_factory: Arc<dyn McaIOFactory>,
    /// Skip the per-region `sync_all()` flush after finalize (`--no-fsync`).
    pub sync_on_finalize: bool,
}

impl Default for IoOptions {
    fn default() -> Self {
        Self {
            fs: Arc::new(RealFileSystem),
            io_factory: Arc::new(DefaultMcaIOFactory),
            sync_on_finalize: true,
        }
    }
}

/// Configuration model for one optimization run.
pub struct OptimizerRequest {
    pub input: PathBuf,
    pub output: Option<PathBuf>,
    pub filter: FilterOptions,
    pub output_options: OutputOptions,
    pub progress: ProgressOptions,
    pub runtime: RuntimeOptions,
    pub hooks: Hooks,
    pub io: IoOptions,
}

/// Builder for [`OptimizerRequest`] with ergonomic defaults.
pub struct OptimizerRequestBuilder {
    request: OptimizerRequest,
}

impl OptimizerRequestBuilder {
    pub fn new(input: PathBuf) -> Self {
        Self {
            request: OptimizerRequest {
                input,
                output: None,
                filter: FilterOptions::default(),
                output_options: OutputOptions::default(),
                progress: ProgressOptions::default(),
                runtime: RuntimeOptions::default(),
                hooks: Hooks::default(),
                io: IoOptions::default(),
            },
        }
    }

    pub fn output(mut self, output: Option<PathBuf>) -> Self {
        self.request.output = output;
        self
    }
    pub fn inhabited_threshold_seconds(mut self, v: i64) -> Self {
        self.request.filter.inhabited_threshold_seconds = v;
        self
    }
    pub fn inhabited_threshold_ticks(mut self, v: Option<i64>) -> Self {
        self.request.filter.inhabited_threshold_ticks = v;
        self
    }
    pub fn remove_unknown(mut self, v: bool) -> Self {
        self.request.filter.remove_unknown = v;
        self
    }
    pub fn strict(mut self, v: bool) -> Self {
        self.request.filter.strict = v;
        self
    }
    pub fn pattern(mut self, p: Box<dyn ChunkPattern>) -> Self {
        self.request.filter.patterns.push(p);
        self
    }
    pub fn in_place(mut self, v: bool) -> Self {
        self.request.output_options.in_place = v;
        self
    }
    pub fn zip_output(mut self, v: bool) -> Self {
        self.request.output_options.zip_output = v;
        self
    }
    pub fn force(mut self, v: bool) -> Self {
        self.request.output_options.force = v;
        self
    }
    pub fn copy_misc(mut self, v: bool) -> Self {
        self.request.output_options.copy_misc = v;
        self
    }
    pub fn dry_run(mut self, v: bool) -> Self {
        self.request.output_options.dry_run = v;
        self
    }
    pub fn progress_mode(mut self, v: ProgressMode) -> Self {
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
    pub fn build(self) -> OptimizerRequest {
        self.request
    }
}

/// Engine interface (injectable/mockable).
pub trait OptimizerEngine {
    fn run(&self, request: &OptimizerRequest) -> Result<OptimizeReport>;
}

/// Convenience entry point.
pub fn optimize(request: &OptimizerRequest) -> Result<OptimizeReport> {
    DefaultOptimizer.run(request)
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

pub struct DefaultOptimizer;

impl OptimizerEngine for DefaultOptimizer {
    fn run(&self, request: &OptimizerRequest) -> Result<OptimizeReport> {
        let fs = request.io.fs.clone();
        let factory = request.io.io_factory.clone();
        let input = request.input.clone();
        let start_time = std::time::Instant::now();

        if !fs.is_directory(&input) {
            return Err(anyhow!("input must be directory"));
        }
        // Guard against aliasing before any write: an output that is the input,
        // an ancestor/descendant of it, or aliases it through a symlink would be
        // wiped by --force before processing. In-place and dry-run use a fresh
        // temp directory that can never overlap.
        if !request.output_options.in_place && !request.output_options.dry_run {
            if let Some(ref output) = request.output {
                guard_overlap(fs.as_ref(), &input, output)?;
            }
        }
        let before_size = dir_size(fs.as_ref(), &input);
        let out = resolve_output_dir(request)?;
        let ticks = request.filter.inhabited_threshold_ticks.unwrap_or(
            request
                .filter
                .inhabited_threshold_seconds
                .checked_mul(20)
                .ok_or_else(|| anyhow!("inhabited threshold seconds overflow"))?,
        );
        let tasks = discover_dimensions(fs.as_ref(), &input);
        let dim_set = tasks.clone();
        // Misc sources = every discovered dimension + the non-dimension
        // ancestors holding world-level files in a 26.1+ Paper layout.
        let misc_parents = discover_misc_parents(fs.as_ref(), &input, &tasks);
        let misc_sources: Vec<PathBuf> = tasks.iter().chain(misc_parents.iter()).cloned().collect();
        let total_chunks = count_total_chunks(&fs, &factory, &tasks);

        let progress = Arc::new(ProgressTracker::new(
            total_chunks,
            request.progress.interval,
            request.progress.interval_ms,
            request.progress.sink.clone(),
        ));
        progress.emit(
            ProgressStage::Init,
            Some(0),
            Some(0),
            Some(&input),
            Some("starting"),
        );
        progress.emit(
            ProgressStage::Discover,
            None,
            None,
            Some(&input),
            Some("scanning dimensions"),
        );
        progress.emit(
            ProgressStage::Discover,
            Some(0),
            Some(total_chunks),
            Some(&input),
            Some("counting chunks"),
        );

        let shared = Shared {
            fs: fs.clone(),
            factory,
            input: input.clone(),
            out: out.clone(),
            ticks,
            remove_unknown: request.filter.remove_unknown,
            sync_on_finalize: request.io.sync_on_finalize,
            dry_run: request.output_options.dry_run,
            extra_patterns: request.filter.patterns.clone(),
            progress: progress.clone(),
            removed_total: Arc::new(AtomicU64::new(0)),
            skipped_files: Arc::new(Mutex::new(HashSet::new())),
            errors: Arc::new(Mutex::new(Vec::new())),
            on_error: request.hooks.on_error.clone(),
            metrics: request.hooks.metrics_sink.clone(),
        };

        let parallelism = if request.runtime.parallelism == 0 {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1)
        } else {
            request.runtime.parallelism
        };
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(parallelism.max(1))
            .build()?;
        // Dimensions are serial; region files within a dimension run in
        // parallel (single-level parallelism, mirroring OrzMCBackup A3).
        pool.install(|| -> Result<()> {
            for dim in &tasks {
                process_dimension(&shared, dim)?;
            }
            Ok(())
        })?;

        // Post-processing writes are all skipped in dry-run (zero bytes hit the
        // output); the scan statistics still reflect the analysis.
        if !request.output_options.dry_run {
            if request.output_options.in_place {
                handle_in_place(&shared, &tasks)?;
            } else {
                if request.output_options.copy_misc {
                    info!("copying misc files");
                    copy_misc_files(
                        fs.as_ref(),
                        &input,
                        &out,
                        &misc_sources,
                        &dim_set,
                        &|p, k, m| shared.record(p, k, m),
                    );
                }
                if request.output_options.zip_output {
                    match compress_to_timestamp_zip(fs.as_ref(), &out) {
                        Ok(zip_path) => {
                            println!("zip: {}", zip_path.display());
                            if let Err(e) = fs.delete_tree(&out) {
                                shared.record(
                                    &out,
                                    ErrorKind::Cleanup,
                                    &format!("Failed to delete output directory: {}", e),
                                );
                            }
                        }
                        Err(e) => shared.record(
                            &out,
                            ErrorKind::Compress,
                            &format!("Failed to compress output directory: {}", e),
                        ),
                    }
                }
            }
        } else {
            // Dry-run temp output is discarded entirely.
            let _ = fs.delete_tree(&out);
        }

        let after_size = if request.output_options.dry_run {
            before_size // dry-run changes nothing
        } else if request.output_options.in_place {
            dir_size(fs.as_ref(), &input)
        } else {
            dir_size(fs.as_ref(), &out)
        };

        let processed = shared.progress.processed.load(Ordering::Relaxed);
        let removed = shared.removed_total.load(Ordering::Relaxed);
        let duration = start_time.elapsed().as_millis() as u64;
        let errors = shared.errors.lock().unwrap().clone();
        let kept = processed.saturating_sub(removed);

        progress.emit(
            ProgressStage::Done,
            Some(processed),
            Some(total_chunks),
            Some(&input),
            None,
        );

        println!(
            "保留区块总数: {} · 删除区块总数: {} · 总耗时: {:.2}s",
            kept,
            removed,
            duration as f64 / 1000.0
        );
        let pct = if before_size > 0 {
            (1.0 - (after_size as f64 / before_size as f64)) * 100.0
        } else {
            0.0
        };
        println!(
            "处理前: {} | 处理后: {} | 缩减: {} ({:.2}%)",
            fmt_bytes(before_size),
            fmt_bytes(after_size),
            fmt_bytes(before_size.saturating_sub(after_size)),
            pct
        );

        let report = OptimizeReport {
            processed_chunks: processed,
            removed_chunks: removed,
            kept_chunks: kept,
            before_size,
            after_size,
            duration,
            errors,
        };

        if let Some(sink) = &request.hooks.report_sink {
            if let Err(e) = sink.write(&report) {
                eprintln!("Failed to write report: {}", e);
            }
        }
        if let Some(m) = &request.hooks.metrics_sink {
            m.inc_processed(processed);
            m.inc_removed(removed);
        }

        Ok(report)
    }
}

/// State shared across parallel region tasks (within one dimension at a time).
struct Shared {
    fs: Arc<dyn FileSystem>,
    factory: Arc<dyn McaIOFactory>,
    input: PathBuf,
    out: PathBuf,
    ticks: i64,
    remove_unknown: bool,
    sync_on_finalize: bool,
    dry_run: bool,
    extra_patterns: Vec<Box<dyn ChunkPattern>>,
    progress: Arc<ProgressTracker>,
    removed_total: Arc<AtomicU64>,
    skipped_files: Arc<Mutex<HashSet<PathBuf>>>,
    errors: Arc<Mutex<Vec<OptimizeError>>>,
    on_error: Option<ErrorHook>,
    metrics: Option<Arc<dyn MetricsSink>>,
}

impl Shared {
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

    fn mark_skipped(&self, rf: &Path, efile: &Path, pfile: &Path) {
        let mut s = self.skipped_files.lock().unwrap();
        s.insert(rf.to_path_buf());
        s.insert(efile.to_path_buf());
        s.insert(pfile.to_path_buf());
    }

    fn emit_region_end(&self, rf: &Path, kept: u64, removed: u64) {
        self.progress.emit(
            ProgressStage::RegionEnd,
            Some(self.progress.processed.load(Ordering::Relaxed)),
            Some(self.progress.total),
            Some(rf),
            Some(&format!("保留 {} 删除 {}", kept, removed)),
        );
    }
}

fn is_dimension_dir(fs: &dyn FileSystem, path: &Path) -> bool {
    fs.is_directory(&path.join("region"))
}

fn discover_dimensions(fs: &dyn FileSystem, root: &Path) -> Vec<PathBuf> {
    let mut tasks: Vec<PathBuf> = Vec::new();
    if is_dimension_dir(fs, root) {
        tasks.push(root.to_path_buf());
    }
    if let Ok(walked) = fs.walk(root, false) {
        for p in walked {
            if p != root && fs.is_directory(&p) && is_dimension_dir(fs, &p) && !tasks.contains(&p) {
                tasks.push(p);
            }
        }
    }
    tasks
}

fn dir_size(fs: &dyn FileSystem, path: &Path) -> u64 {
    if fs.is_regular_file(path) {
        return fs.size(path);
    }
    let mut total = 0u64;
    if let Ok(rd) = fs.list(path) {
        for e in rd {
            total = total.saturating_add(dir_size(fs, &e));
        }
    }
    total
}

fn fmt_bytes(mut n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut i = 0;
    let mut val = n as f64;
    while n >= 1024 && i < UNITS.len() - 1 {
        val /= 1024.0;
        n /= 1024;
        i += 1;
    }
    format!("{:.2} {}", val, UNITS[i])
}

/// True when `path` is a readable region MCA at least 8192 bytes (two header
/// sectors) long. Silent — callers decide whether an invalid file is an error
/// or merely an absent entities/poi counterpart.
fn is_valid_mca(fs: &dyn FileSystem, path: &Path) -> bool {
    fs.is_regular_file(path) && fs.size(path) >= 8192
}

fn count_total_chunks(
    fs: &Arc<dyn FileSystem>,
    factory: &Arc<dyn McaIOFactory>,
    dims: &[PathBuf],
) -> u64 {
    let mut total = 0u64;
    for dim in dims {
        let region_dir = dim.join("region");
        if !fs.is_directory(&region_dir) {
            continue;
        }
        if let Ok(rd) = fs.list(&region_dir) {
            for p in rd {
                if p.extension().and_then(|s| s.to_str()) == Some("mca")
                    && is_valid_mca(fs.as_ref(), &p)
                {
                    if let Ok(mut r) = factory.open_reader(fs.clone(), &p) {
                        if let Ok(c) = r.count() {
                            total += c as u64;
                        }
                    }
                }
            }
        }
    }
    total
}

fn guard_overlap(fs: &dyn FileSystem, input: &Path, output: &Path) -> Result<()> {
    if fs.as_any().downcast_ref::<RealFileSystem>().is_some() {
        ensure_no_overlap(input, Some(output))
    } else {
        // Memory/other filesystems have no canonicalize; compare raw paths.
        if input == output || input.starts_with(output) || output.starts_with(input) {
            Err(anyhow!(
                "output directory {} overlaps input world {}",
                output.display(),
                input.display()
            ))
        } else {
            Ok(())
        }
    }
}

fn resolve_output_dir(request: &OptimizerRequest) -> Result<PathBuf> {
    let fs = &request.io.fs;
    if request.output_options.dry_run {
        return Ok(fs.create_temp_directory("thanos-dry-")?);
    }
    if request.output_options.in_place {
        return Ok(fs.create_temp_directory("thanos-")?);
    }
    let output = request
        .output
        .as_ref()
        .ok_or_else(|| anyhow!("output directory required when not in in-place mode"))?;
    if fs.exists(output) {
        let non_empty = !fs.list(output)?.is_empty();
        if non_empty {
            if request.output_options.force {
                fs.delete_tree(output)?;
                fs.create_directories(output)?;
            } else {
                return Err(anyhow!(
                    "output directory already exists and is not empty; use --force to overwrite"
                ));
            }
        }
    } else {
        fs.create_directories(output)?;
    }
    Ok(output.clone())
}

/// Per-dimension context for parallel region processing.
struct DimCtx<'a> {
    shared: &'a Shared,
    target_dim: PathBuf,
    entities_dir: PathBuf,
    poi_dir: PathBuf,
    patterns: Vec<Box<dyn ChunkPattern>>,
}

fn process_dimension(shared: &Shared, dim: &Path) -> Result<()> {
    let fs = shared.fs.clone();
    let rel = dim.strip_prefix(&shared.input).unwrap_or(dim);
    let target_dim = shared.out.join(rel);
    fs.create_directories(&target_dim)?;

    let forced = parse_force_loaded_fs(fs.as_ref(), dim);
    let mut patterns: Vec<Box<dyn ChunkPattern>> = Vec::new();
    patterns.push(Box::new(ListPattern::new(forced)));
    patterns.push(Box::new(InhabitedTimePattern::new(
        shared.ticks,
        shared.remove_unknown,
    )));
    for p in &shared.extra_patterns {
        patterns.push(p.box_clone());
    }

    let region_dir = dim.join("region");
    let entities_dir = dim.join("entities");
    let poi_dir = dim.join("poi");
    fs.create_directories(&target_dim.join("region"))?;
    if fs.is_directory(&entities_dir) {
        fs.create_directories(&target_dim.join("entities"))?;
    }
    if fs.is_directory(&poi_dir) {
        fs.create_directories(&target_dim.join("poi"))?;
    }

    shared
        .progress
        .emit(ProgressStage::DimensionStart, None, None, Some(dim), None);

    let regions: Vec<PathBuf> = fs
        .list(&region_dir)?
        .into_iter()
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("mca"))
        .collect();

    let ctx = DimCtx {
        shared,
        target_dim,
        entities_dir,
        poi_dir,
        patterns,
    };
    // Region files within a dimension are independent: each writes its own
    // output, so parallel scheduling never affects the result.
    regions.par_iter().for_each(|rf| process_region(&ctx, rf));

    shared
        .progress
        .emit(ProgressStage::DimensionEnd, None, None, Some(dim), None);
    Ok(())
}

fn process_region(ctx: &DimCtx, rf: &Path) {
    let shared = ctx.shared;
    let fs = shared.fs.clone();
    let factory = shared.factory.clone();
    let name = rf.file_name().unwrap().to_string_lossy().to_string();
    let efile = ctx.entities_dir.join(&name);
    let pfile = ctx.poi_dir.join(&name);
    let out_region = ctx.target_dim.join("region").join(&name);
    let out_entities = ctx.target_dim.join("entities").join(&name);
    let out_poi = ctx.target_dim.join("poi").join(&name);

    shared
        .progress
        .emit(ProgressStage::RegionStart, None, None, Some(rf), None);

    if !is_valid_mca(fs.as_ref(), rf) {
        shared.record(
            rf,
            ErrorKind::Mca,
            &format!(
                "MCA file is corrupted or incomplete ({} bytes)",
                fs.size(rf)
            ),
        );
        shared.mark_skipped(rf, &efile, &pfile);
        shared.emit_region_end(rf, 0, 0);
        return;
    }

    let mut cr = match factory.open_reader(fs.clone(), rf) {
        Ok(r) => r,
        Err(e) => {
            shared.record(
                rf,
                ErrorKind::Mca,
                &format!("Failed to read MCA file: {}", e),
            );
            shared.mark_skipped(rf, &efile, &pfile);
            shared.emit_region_end(rf, 0, 0);
            return;
        }
    };

    // Entities/poi readers are optional: a missing or invalid counterpart is
    // simply absent, not an error.
    let mut er: Option<Box<dyn McaReaderLike>> = None;
    if is_valid_mca(fs.as_ref(), &efile) {
        er = match factory.open_reader(fs.clone(), &efile) {
            Ok(r) => Some(r),
            Err(e) => {
                shared.record(
                    &efile,
                    ErrorKind::Entities,
                    &format!("Failed to read entities MCA: {}", e),
                );
                None
            }
        };
    }
    let mut pr: Option<Box<dyn McaReaderLike>> = None;
    if is_valid_mca(fs.as_ref(), &pfile) {
        pr = match factory.open_reader(fs.clone(), &pfile) {
            Ok(r) => Some(r),
            Err(e) => {
                shared.record(
                    &pfile,
                    ErrorKind::Poi,
                    &format!("Failed to read POI MCA: {}", e),
                );
                None
            }
        };
    }

    // Writers are created up front but lazily open their file on first write,
    // so a fully-removed region never touches disk (mirrors OrzMCBackup's
    // `if (cw == null && !dryRun)` guard — dry-run creates no writers at all).
    let mut cw: Option<Box<dyn McaWriterLike>> = None;
    let mut ew: Option<Box<dyn McaWriterLike>> = None;
    let mut pw: Option<Box<dyn McaWriterLike>> = None;
    if !shared.dry_run {
        cw = match factory.create_writer(fs.clone(), &out_region, shared.sync_on_finalize) {
            Ok(w) => Some(w),
            Err(e) => {
                shared.record(
                    rf,
                    ErrorKind::Output,
                    &format!("Failed to create output region MCA {}: {}", name, e),
                );
                None
            }
        };
        if er.is_some() {
            ew = match factory.create_writer(fs.clone(), &out_entities, shared.sync_on_finalize) {
                Ok(w) => Some(w),
                Err(e) => {
                    shared.record(
                        &efile,
                        ErrorKind::Output,
                        &format!("Failed to create output entities MCA {}: {}", name, e),
                    );
                    None
                }
            };
        }
        if pr.is_some() {
            pw = match factory.create_writer(fs.clone(), &out_poi, shared.sync_on_finalize) {
                Ok(w) => Some(w),
                Err(e) => {
                    shared.record(
                        &pfile,
                        ErrorKind::Output,
                        &format!("Failed to create output poi MCA {}: {}", name, e),
                    );
                    None
                }
            };
        }
    }

    let mut region_entries = match cr.entries() {
        Ok(v) => v,
        Err(e) => {
            shared.record(
                rf,
                ErrorKind::Entries,
                &format!("Failed to read chunk entries: {}", e),
            );
            shared.mark_skipped(rf, &efile, &pfile);
            shared.emit_region_end(rf, 0, 0);
            return;
        }
    };

    let mut removed = 0u64;
    let mut kept = 0u64;

    for entry in region_entries.iter_mut() {
        let mut keep = false;
        for p in ctx.patterns.iter() {
            match p.matches(entry) {
                Ok(true) => {
                    keep = true;
                    break;
                }
                Ok(false) => {}
                Err(e) => {
                    // A chunk we cannot evaluate is kept (safe-keep), never
                    // silently deleted. Mirrors OrzMCBackup's shouldKeep.
                    let msg = format!(
                        "Pattern evaluation failed on chunk {} in {}: {}（已保留原始区块）",
                        entry.region_index(),
                        name,
                        e
                    );
                    shared.record(rf, ErrorKind::Pattern, &msg);
                    keep = true;
                    break;
                }
            }
        }
        if keep {
            kept += 1;
            if let Some(ref mut cw) = cw {
                if let Err(e) = cw.write_entry(entry) {
                    let msg = format!(
                        "Failed to write chunk entry {} in {}: {}",
                        entry.region_index(),
                        name,
                        e
                    );
                    shared.record(rf, ErrorKind::Write, &msg);
                }
            }
            if let Some(ref mut erdr) = er {
                match erdr.get(entry.region_index() as usize) {
                    Ok(Some(mut eentry)) => {
                        if let Some(ref mut w) = ew {
                            if let Err(e) = w.write_entry(&mut eentry) {
                                let msg = format!(
                                    "Failed to write entities entry {} in {}: {}",
                                    entry.region_index(),
                                    name,
                                    e
                                );
                                shared.record(&efile, ErrorKind::WriteEntities, &msg);
                            }
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        let msg = format!(
                            "Failed to read entities entry {} in {}: {}",
                            entry.region_index(),
                            name,
                            e
                        );
                        shared.record(&efile, ErrorKind::Entities, &msg);
                    }
                }
            }
            if let Some(ref mut prdr) = pr {
                match prdr.get(entry.region_index() as usize) {
                    Ok(Some(mut pentry)) => {
                        if let Some(ref mut w) = pw {
                            if let Err(e) = w.write_entry(&mut pentry) {
                                let msg = format!(
                                    "Failed to write poi entry {} in {}: {}",
                                    entry.region_index(),
                                    name,
                                    e
                                );
                                shared.record(&pfile, ErrorKind::WritePoi, &msg);
                            }
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        let msg = format!(
                            "Failed to read poi entry {} in {}: {}",
                            entry.region_index(),
                            name,
                            e
                        );
                        shared.record(&pfile, ErrorKind::Poi, &msg);
                    }
                }
            }
        } else {
            removed += 1;
            shared.removed_total.fetch_add(1, Ordering::Relaxed);
        }
        shared.progress.chunk_progress(Some(rf));
    }

    if let Some(ref mut cw) = cw {
        if cw.is_empty() {
            info!("Region {} fully removed, no output file written", name);
        }
        if let Err(e) = cw.finalize_file() {
            shared.record(
                rf,
                ErrorKind::Finalize,
                &format!("Failed to finalize region MCA: {}", e),
            );
        }
    }
    if let Some(ref mut w) = ew {
        if let Err(e) = w.finalize_file() {
            shared.record(
                &efile,
                ErrorKind::FinalizeEntities,
                &format!("Failed to finalize entities MCA: {}", e),
            );
        }
    }
    if let Some(ref mut w) = pw {
        if let Err(e) = w.finalize_file() {
            shared.record(
                &pfile,
                ErrorKind::FinalizePoi,
                &format!("Failed to finalize POI MCA: {}", e),
            );
        }
    }
    info!(
        "Region {} processed, kept {} removed {} chunks",
        name, kept, removed
    );
    shared.emit_region_end(rf, kept, removed);
}

fn handle_in_place(shared: &Shared, tasks: &[PathBuf]) -> Result<()> {
    let fs = shared.fs.clone();
    for dim in tasks {
        let rel = dim.strip_prefix(&shared.input).unwrap_or(dim);
        let out_dim = shared.out.join(rel);
        let in_dim = shared.input.join(rel);
        for name in ["region", "entities", "poi"] {
            let src = out_dim.join(name);
            if !fs.is_directory(&src) {
                continue;
            }
            let dst = in_dim.join(name);
            fs.create_directories(&dst)?;
            let mut keep: HashSet<String> = HashSet::new();
            for p in fs.list(&src)? {
                if p.extension().and_then(|s| s.to_str()) == Some("mca") {
                    keep.insert(p.file_name().unwrap().to_string_lossy().to_string());
                }
            }
            if fs.is_directory(&dst) {
                let skipped = shared.skipped_files.lock().unwrap();
                for p in fs.list(&dst)? {
                    if p.extension().and_then(|s| s.to_str()) == Some("mca") {
                        let fname = p.file_name().unwrap().to_string_lossy().to_string();
                        // Never delete an input MCA that was skipped (too small,
                        // unreadable, or unparseable) — it was preserved as-is.
                        if !keep.contains(&fname) && !skipped.contains(&p) {
                            let _ = fs.delete_if_exists(&p);
                        }
                    }
                }
            }
            for p in fs.list(&src)? {
                if p.extension().and_then(|s| s.to_str()) == Some("mca") {
                    let target = dst.join(p.file_name().unwrap());
                    if let Err(e) = fs.copy(&p, &target, true) {
                        shared.record(&p, ErrorKind::InPlace, &format!("Failed to copy: {}", e));
                    }
                }
            }
        }
    }
    if let Err(e) = fs.delete_tree(&shared.out) {
        shared.record(
            &shared.out,
            ErrorKind::InPlace,
            &format!("Failed to clean temp dir: {}", e),
        );
    }
    Ok(())
}
