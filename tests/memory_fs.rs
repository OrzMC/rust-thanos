//! End-to-end optimizer smoke test running entirely on an in-memory
//! filesystem (MemoryFS + MemoryMcaIOFactory). No disk world is touched.
//!
//! The engine pipeline (dimension discovery → count → region processing →
//! report) must behave identically on the memory tree as on the real
//! filesystem, which is exactly what the full IO abstraction is for.

use rust_thanos::mca::reader::McaReader;
use rust_thanos::world::{
    DefaultOptimizer, FileSystem, MemoryFS, MemoryMcaIOFactory, OptimizerEngine, OptimizerRequest,
    OptimizerRequestBuilder, ProgressMode,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The real fixture region (676 chunks, InhabitedTime spread over 0..=40).
/// At a strict-`>` threshold of 0 ticks exactly 58 chunks are kept — the same
/// expectation the disk-backed `threshold.rs` asserts.
const FIXTURE_REGION: &str = "tests/Fixtures/world/region/r.0.0.mca";
const KEPT_AT_ZERO: u64 = 58;
const TOTAL_CHUNKS: u64 = 676;

/// Seeds a MemoryFS with just the fixture region under `/world/region/`
/// (no `data/chunks.dat`, so force-load can never influence the counts).
fn memory_world() -> Arc<MemoryFS> {
    let bytes = std::fs::read(FIXTURE_REGION).expect("fixture region readable");
    let mem = Arc::new(MemoryFS::new());
    mem.write(Path::new("/world/region/r.0.0.mca"), &bytes)
        .expect("seed memory world");
    mem
}

#[test]
fn memory_fs_end_to_end_threshold_zero() {
    let mem = memory_world();
    let request = OptimizerRequestBuilder::new(PathBuf::from("/world"))
        .output(Some(PathBuf::from("/out")))
        .inhabited_threshold_ticks(Some(0))
        .progress_mode(ProgressMode::Off)
        .fs(mem.clone())
        .io_factory(Arc::new(MemoryMcaIOFactory))
        .build();
    let report = DefaultOptimizer
        .run(&request)
        .expect("optimize on memory FS");

    assert_eq!(
        report.processed_chunks, TOTAL_CHUNKS,
        "all fixture chunks processed"
    );
    assert_eq!(
        report.removed_chunks,
        TOTAL_CHUNKS - KEPT_AT_ZERO,
        "strict >0 removes every uninhabited chunk"
    );
    assert!(report.errors.is_empty(), "no errors on a clean region");

    // The output region must exist on the memory tree with exactly 58 chunks.
    let out_bytes = mem
        .read(Path::new("/out/region/r.0.0.mca"))
        .expect("output region present")
        .expect("output region bytes");
    let mut reader = McaReader::open_from_bytes("/out/region/r.0.0.mca", out_bytes)
        .expect("parse output region");
    assert_eq!(reader.count().unwrap(), KEPT_AT_ZERO as usize);
}

#[test]
fn memory_fs_end_to_end_keeps_everything() {
    let mem = memory_world();
    // The fixture's InhabitedTime values only span 0..=40, so no positive
    // threshold keeps everything. A negative threshold (-1, strict `>`) keeps
    // every chunk including the uninhabited ones.
    let request = OptimizerRequestBuilder::new(PathBuf::from("/world"))
        .output(Some(PathBuf::from("/out2")))
        .inhabited_threshold_ticks(Some(-1))
        .progress_mode(ProgressMode::Off)
        .fs(mem.clone())
        .io_factory(Arc::new(MemoryMcaIOFactory))
        .build();
    let report = DefaultOptimizer
        .run(&request)
        .expect("optimize on memory FS");

    assert_eq!(
        report.removed_chunks, 0,
        "negative threshold keeps everything"
    );
    let out_bytes = mem
        .read(Path::new("/out2/region/r.0.0.mca"))
        .expect("output region present")
        .expect("output region bytes");
    let mut reader = McaReader::open_from_bytes("/out2/region/r.0.0.mca", out_bytes)
        .expect("parse output region");
    assert_eq!(reader.count().unwrap(), TOTAL_CHUNKS as usize);
}

#[test]
fn memory_fs_request_construction_is_public_api() {
    // The request/builder/engine types are library API usable by external
    // callers — this test exercises construction AND the seconds→ticks path.
    let mem = memory_world();
    let req: OptimizerRequest = OptimizerRequestBuilder::new(PathBuf::from("/world"))
        .output(Some(PathBuf::from("/out3")))
        .inhabited_threshold_seconds(300) // 6000 ticks
        .parallelism(0)
        .strict(true)
        .sync_on_finalize(true)
        .fs(mem.clone())
        .io_factory(Arc::new(MemoryMcaIOFactory))
        .build();
    // The fixture's InhabitedTime is spread over 0..=40, so >6000 keeps none:
    // the whole region is removed and the lazy writer produces no output file.
    let report = DefaultOptimizer.run(&req).expect("run completes");
    assert_eq!(report.removed_chunks, TOTAL_CHUNKS);
    assert_eq!(report.kept_chunks(), 0);
    assert!(
        !mem.exists(Path::new("/out3/region/r.0.0.mca")),
        "fully-removed region must not produce an output file"
    );
    assert!(report.duration > 0);
}
