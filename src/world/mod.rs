//! World optimization engine (library entry point).
//!
//! The optimizer pipeline is built from the pieces in this module:
//! [`fs`] (filesystem abstraction), [`io_factory`] (MCA reader/writer factory),
//! [`forceload`] (force-loaded chunk parsing), [`output`] (overlap guard),
//! [`misc`] (non-MCA copy), [`zip`] (timestamped archive), [`progress`]
//! (event model + sinks), [`report`] (report model + sinks), and
//! [`optimizer`] (the request model + engine). The legacy [`run`] function
//! remains as a thin convenience wrapper that builds a default
//! [`OptimizerRequest`] against the real filesystem.

mod forceload;
mod fs;
mod io_factory;
mod merger;
mod misc;
mod optimizer;
mod output;
mod progress;
mod report;
mod zip;

pub use crate::patterns::range::RangePattern;
pub use forceload::parse_force_loaded;
pub use fs::{FileSystem, MemoryFS, RealFileSystem};
pub use io_factory::{
    DefaultMcaIOFactory, McaIOFactory, McaReaderLike, McaWriterLike, MemoryMcaIOFactory,
};
pub use merger::{
    merge, DefaultMerger, MergeEngine, MergeReport, MergeReportIO, MergeRequest,
    MergeRequestBuilder,
};
pub use optimizer::{
    optimize, DefaultOptimizer, FilterOptions, Hooks, IoOptions, OptimizerEngine, OptimizerRequest,
    OptimizerRequestBuilder, OutputOptions, ProgressMode, ProgressOptions, RuntimeOptions,
};
pub use progress::{
    NoopProgressSink, ProgressEvent, ProgressSink, ProgressStage, ProgressTracker, TermProgressSink,
};
pub use report::{
    ErrorHook, ErrorKind, FileReportSink, MetricsSink, NoopMetricsSink, OptimizeError,
    OptimizeReport, ReportFormat, ReportIO, ReportSink,
};

use anyhow::Result;
use std::path::PathBuf;

/// Convenience wrapper over [`optimize`] preserving the legacy tick-based API.
///
/// `inhabited_threshold` is in **ticks**, not seconds (the tests pass 26/27
/// directly to pin the strict-`>` boundary). It is forwarded via
/// [`OptimizerRequestBuilder::inhabited_threshold_ticks`] so the ×20
/// seconds→ticks conversion never applies. `output == None` means in-place.
pub fn run(
    input: PathBuf,
    output: Option<PathBuf>,
    inhabited_threshold: i64,
    remove_unknown: bool,
    progress_mode: ProgressMode,
) -> Result<OptimizeReport> {
    let in_place = output.is_none();
    let request = OptimizerRequestBuilder::new(input)
        .output(output)
        .inhabited_threshold_ticks(Some(inhabited_threshold))
        .remove_unknown(remove_unknown)
        .progress_mode(progress_mode)
        .in_place(in_place)
        .build();
    optimize(&request)
}
