//! Report model + serializers.
//!
//! Mirrors OrzMCBackup's `Report.kt` and `OptimizerConfig.kt` hooks: an
//! [`OptimizeReport`] summarizes a run's chunk statistics and non-fatal
//! [`OptimizeError`]s, [`ReportIO`] serializes it to JSON/CSV/text, and
//! [`ReportSink`]/[`FileReportSink`] deliver it (`--report-file`).
//! [`MetricsSink`] is the companion hook for aggregated counters.

use clap::ValueEnum;
use std::fmt;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

/// Error category for a recorded [`OptimizeError`]. Aligned with OrzMCBackup's
/// string kinds (`ERR_MCA`, `ERR_WRITE`, …) so report consumers see the same
/// taxonomy across the Rust and Java implementations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::upper_case_acronyms)]
pub enum ErrorKind {
    Mca,
    Entities,
    Poi,
    Entries,
    Pattern,
    Write,
    WriteEntities,
    WritePoi,
    Finalize,
    FinalizeEntities,
    FinalizePoi,
    Input,
    Output,
    CopyMisc,
    Compress,
    Cleanup,
    InPlace,
    Copy,
    Parallel,
    ForceLoad,
}

impl ErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mca => "Mca",
            Self::Entities => "Entities",
            Self::Poi => "Poi",
            Self::Entries => "Entries",
            Self::Pattern => "Pattern",
            Self::Write => "Write",
            Self::WriteEntities => "WriteEntities",
            Self::WritePoi => "WritePoi",
            Self::Finalize => "Finalize",
            Self::FinalizeEntities => "FinalizeEntities",
            Self::FinalizePoi => "FinalizePoi",
            Self::Input => "Input",
            Self::Output => "Output",
            Self::CopyMisc => "CopyMisc",
            Self::Compress => "Compress",
            Self::Cleanup => "Cleanup",
            Self::InPlace => "InPlace",
            Self::Copy => "Copy",
            Self::Parallel => "Parallel",
            Self::ForceLoad => "ForceLoad",
        }
    }
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A single recorded (non-fatal) error during an optimize run.
#[derive(Debug, Clone)]
pub struct OptimizeError {
    pub path: String,
    pub kind: ErrorKind,
    pub message: String,
}

/// The `on_error` hook type shared by the optimizer and merger request models.
pub type ErrorHook = Arc<dyn Fn(&OptimizeError) + Send + Sync>;

/// Summary report for an optimization run.
#[derive(Debug, Clone)]
pub struct OptimizeReport {
    /// Total chunks scanned (kept + removed).
    pub processed_chunks: u64,
    /// Chunks removed by the filter.
    pub removed_chunks: u64,
    /// Chunks kept (scanned minus removed).
    pub kept_chunks: u64,
    /// Input directory size before processing.
    pub before_size: u64,
    /// Output (or, in-place, post-replacement) directory size.
    pub after_size: u64,
    /// Wall-clock duration in milliseconds.
    pub duration: u64,
    /// Non-fatal errors collected during the run.
    pub errors: Vec<OptimizeError>,
}

impl OptimizeReport {
    pub fn kept_chunks(&self) -> u64 {
        self.kept_chunks
    }
}

/// Report output format for [`ReportIO`] / [`FileReportSink`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ReportFormat {
    Json,
    Csv,
    Text,
}

impl ReportFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Csv => "csv",
            Self::Text => "text",
        }
    }
}

impl fmt::Display for ReportFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Serializes [`OptimizeReport`] to JSON, CSV, or plain text, mirroring
/// OrzMCBackup's `ReportIO`.
pub struct ReportIO;

impl ReportIO {
    /// JSON string escaping, shared with `MergeReportIO`.
    pub(crate) fn esc(s: &str) -> String {
        let mut out = String::with_capacity(s.len() + 16);
        for c in s.chars() {
            match c {
                '\\' => out.push_str("\\\\"),
                '"' => out.push_str("\\\""),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                c if (c as u32) < 0x20 => {
                    out.push_str(&format!("\\u{:04x}", c as u32));
                }
                c => out.push(c),
            }
        }
        out
    }

    pub fn to_json(r: &OptimizeReport) -> String {
        let mut sb = String::new();
        sb.push_str(&format!(
            "{{\"processedChunks\":{},\"removedChunks\":{},\"keptChunks\":{},\"beforeSize\":{},\"afterSize\":{},\"durationMs\":{},\"errors\":[",
            r.processed_chunks,
            r.removed_chunks,
            r.kept_chunks,
            r.before_size,
            r.after_size,
            r.duration,
        ));
        for (i, e) in r.errors.iter().enumerate() {
            if i > 0 {
                sb.push(',');
            }
            sb.push_str(&format!(
                "{{\"path\":\"{}\",\"kind\":\"{}\",\"message\":\"{}\"}}",
                Self::esc(&e.path),
                Self::esc(e.kind.as_str()),
                Self::esc(&e.message),
            ));
        }
        sb.push_str("]}");
        sb
    }

    pub fn to_csv(r: &OptimizeReport) -> String {
        let mut sb = String::new();
        sb.push_str("processedChunks,removedChunks,keptChunks,beforeSize,afterSize,durationMs,errorsCount\n");
        sb.push_str(&format!(
            "{},{},{},{},{},{},{}\n",
            r.processed_chunks,
            r.removed_chunks,
            r.kept_chunks,
            r.before_size,
            r.after_size,
            r.duration,
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

    pub fn to_text(r: &OptimizeReport) -> String {
        let mut sb = String::new();
        sb.push_str(&format!(
            "Statistics: processed={} removed={} kept={} beforeSize={} afterSize={} durationMs={} errors={}\n",
            r.processed_chunks,
            r.removed_chunks,
            r.kept_chunks,
            r.before_size,
            r.after_size,
            r.duration,
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
    pub fn write(r: &OptimizeReport, path: &Path, format: ReportFormat) -> anyhow::Result<()> {
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

/// Receives the final [`OptimizeReport`] of a run (e.g. to write it to a file
/// or forward it to an external system). Mirrors OrzMCBackup `ReportSink`.
pub trait ReportSink: Send + Sync {
    fn write(&self, report: &OptimizeReport) -> anyhow::Result<()>;
}

/// A [`ReportSink`] that writes the report to a file via [`ReportIO`].
pub struct FileReportSink {
    pub path: std::path::PathBuf,
    pub format: ReportFormat,
}

impl FileReportSink {
    pub fn new(path: std::path::PathBuf, format: ReportFormat) -> Self {
        Self { path, format }
    }
}

impl ReportSink for FileReportSink {
    fn write(&self, report: &OptimizeReport) -> anyhow::Result<()> {
        ReportIO::write(report, &self.path, self.format)
    }
}

/// Receives aggregated counters and error notifications during a run. Mirrors
/// OrzMCBackup `MetricsSink`.
pub trait MetricsSink: Send + Sync {
    fn record_error(&self, error: &OptimizeError);
    fn inc_processed(&self, n: u64);
    fn inc_removed(&self, n: u64);
}

/// A [`MetricsSink`] that discards everything.
pub struct NoopMetricsSink;

impl MetricsSink for NoopMetricsSink {
    fn record_error(&self, _error: &OptimizeError) {}
    fn inc_processed(&self, _n: u64) {}
    fn inc_removed(&self, _n: u64) {}
}
