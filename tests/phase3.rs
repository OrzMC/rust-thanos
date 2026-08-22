//! Phase 3 e2e: scripting + parallelism.
//!
//! Verifies the report model/serializers (`--report` / `--report-file` /
//! `--report-format`), the `ErrorKind` taxonomy on recorded errors, region-level
//! parallelism determinism (`--parallelism` byte-identical to serial), and the
//! `McaReader::get_chunk_at` public API.

use rust_thanos::mca::reader::McaReader;
use rust_thanos::world::{
    optimize, DefaultOptimizer, ErrorKind, OptimizeReport, OptimizerEngine,
    OptimizerRequestBuilder, ProgressMode, ReportIO,
};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

/// Copies the real fixture world to a fresh temp dir so tests can mutate it.
fn fixture_world() -> PathBuf {
    let base = std::env::temp_dir().join(format!("rt-p3-{}", uuid::Uuid::new_v4()));
    let world = base.join("world");
    fs::create_dir_all(&world).unwrap();
    let mut opts = fs_extra::dir::CopyOptions::new();
    opts.content_only = true;
    fs_extra::dir::copy("tests/Fixtures/world", &world, &opts).unwrap();
    world
}

/// Snapshot every file (relpath → bytes) under `root` for byte-level comparison.
fn snapshot(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut map = BTreeMap::new();
    for e in walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if e.file_type().is_file() {
            let rel = e
                .path()
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .to_string();
            map.insert(rel, fs::read(e.path()).unwrap());
        }
    }
    map
}

fn run_req(input: PathBuf, out: PathBuf, parallelism: usize) -> OptimizeReport {
    let request = OptimizerRequestBuilder::new(input)
        .output(Some(out))
        .inhabited_threshold_ticks(Some(0))
        .progress_mode(ProgressMode::Off)
        .parallelism(parallelism)
        .build();
    DefaultOptimizer.run(&request).expect("optimize run")
}

#[test]
fn report_io_json_csv_text() {
    let report = OptimizeReport {
        processed_chunks: 100,
        removed_chunks: 40,
        kept_chunks: 60,
        before_size: 10_000,
        after_size: 6_000,
        duration: 1234,
        errors: vec![
            rust_thanos::world::OptimizeError {
                path: "region/r.0.0.mca".into(),
                kind: ErrorKind::Mca,
                message: "MCA file is corrupted".into(),
            },
            rust_thanos::world::OptimizeError {
                path: "data/x.dat".into(),
                kind: ErrorKind::CopyMisc,
                message: "Failed to copy \"file\"\nline2".into(),
            },
        ],
    };

    let json = ReportIO::to_json(&report);
    assert!(json.contains("\"processedChunks\":100"));
    assert!(json.contains("\"removedChunks\":40"));
    assert!(json.contains("\"keptChunks\":60"));
    assert!(json.contains("\"kind\":\"Mca\""));
    assert!(json.contains("\"kind\":\"CopyMisc\""));
    // Escaping: double quotes become \", newlines become \n.
    assert!(json.contains("Failed to copy \\\"file\\\"\\nline2"));

    let csv = ReportIO::to_csv(&report);
    assert!(csv.starts_with("processedChunks,removedChunks,keptChunks"));
    assert!(csv.contains("\"region/r.0.0.mca\",\"Mca\""));
    assert!(csv.contains("\"data/x.dat\",\"CopyMisc\""));

    let text = ReportIO::to_text(&report);
    assert!(text.contains("Statistics: processed=100 removed=40 kept=60"));
    assert!(text.contains("[Mca] region/r.0.0.mca - MCA file is corrupted"));
    assert!(text.contains("[CopyMisc] data/x.dat"));
}

#[test]
fn get_chunk_at_resolves_world_coords() {
    let mut r = McaReader::open("tests/Fixtures/world/region/r.0.0.mca").unwrap();
    let entries = r.entries().unwrap();
    assert!(!entries.is_empty(), "fixture region must have entries");
    for e in entries.iter().take(20) {
        let got = r.get_chunk_at(e.global_x(), e.global_z()).unwrap().unwrap();
        assert_eq!(got.region_index(), e.region_index());
    }
    // Negative-coordinate region: r.-1.0.mca holds world x in [-32, 0).
    let mut rn = McaReader::open("tests/Fixtures/world/region/r.-1.0.mca").unwrap();
    let en = rn.entries().unwrap();
    assert!(!en.is_empty(), "negative region must have entries");
    for e in en.iter().take(20) {
        let got = rn
            .get_chunk_at(e.global_x(), e.global_z())
            .unwrap()
            .unwrap();
        assert_eq!(got.region_index(), e.region_index());
    }
}

#[test]
fn parallelism_is_deterministic() {
    // Two independent copies of the same world, one serial one parallel — the
    // output must be byte-identical file-for-file.
    let w1 = fixture_world();
    let w2 = fixture_world();
    let out1 = w1.parent().unwrap().join("out1");
    let out2 = w2.parent().unwrap().join("out2");

    let r1 = run_req(w1, out1.clone(), 1);
    let r2 = run_req(w2, out2.clone(), 4);

    assert_eq!(r1.processed_chunks, r2.processed_chunks, "same scan");
    assert_eq!(r1.removed_chunks, r2.removed_chunks, "same removal");
    assert_eq!(
        snapshot(&out1),
        snapshot(&out2),
        "parallel and serial outputs must be byte-identical"
    );
}

#[test]
fn corrupt_region_records_error_kind_mca() {
    let world = fixture_world();
    // Truncate one region file below the 8192-byte header minimum.
    fs::write(world.join("region").join("r.0.0.mca"), vec![0u8; 100]).unwrap();
    let out = world.parent().unwrap().join("out");
    let report = run_req(world, out.clone(), 2);

    let kinds: Vec<ErrorKind> = report.errors.iter().map(|e| e.kind).collect();
    assert!(
        kinds.contains(&ErrorKind::Mca),
        "corrupt region must be recorded with kind Mca, got {:?}",
        kinds
    );
    // Non-strict run completes and still writes the valid sibling region, and
    // the corrupt region is byte-preserved in the output rather than dropped.
    assert!(out.join("region").join("r.-1.0.mca").exists());
    let preserved = fs::read(out.join("region").join("r.0.0.mca")).unwrap();
    assert_eq!(preserved, vec![0u8; 100], "corrupt region preserved as-is");
    // Its entities/poi counterparts are preserved too.
    assert!(
        out.join("entities").join("r.0.0.mca").exists(),
        "corrupt region's entities preserved"
    );
    assert!(
        out.join("poi").join("r.0.0.mca").exists(),
        "corrupt region's poi preserved"
    );
}

#[test]
fn cli_report_file_and_strict_exit_code() {
    let world = fixture_world();
    fs::write(world.join("region").join("r.0.0.mca"), vec![0u8; 100]).unwrap();
    let base = world.parent().unwrap();
    let out = base.join("out");
    let rpt = base.join("report.json");

    let bin = env!("CARGO_BIN_EXE_rust-thanos");
    let status = Command::new(bin)
        .args([
            "backup",
            world.to_str().unwrap(),
            out.to_str().unwrap(),
            "-t",
            "0",
            "--report",
            "--report-file",
            rpt.to_str().unwrap(),
            "--report-format",
            "json",
            "--strict",
        ])
        .output()
        .unwrap();

    // strict + recorded error → exit 1.
    assert!(
        !status.status.success(),
        "strict run with errors must exit non-zero, stdout: {}",
        String::from_utf8_lossy(&status.stdout)
    );
    let rpt_txt = fs::read_to_string(&rpt).expect("report file written");
    assert!(
        rpt_txt.contains("\"kind\":\"Mca\""),
        "report json has Mca error: {rpt_txt}"
    );
    assert!(
        rpt_txt.contains("\"processedChunks\""),
        "report json has stats"
    );
    // The plain-text report was printed to stdout.
    let stdout = String::from_utf8_lossy(&status.stdout);
    assert!(
        stdout.contains("Statistics:"),
        "text report printed to stdout"
    );
}

#[test]
fn keep_range_pattern_keeps_rectangle() {
    // Chunk (2, 2) inside r.0.0.mca is always kept when the rectangle covers it,
    // even with -t 0 (which removes every not-otherwise-kept chunk).
    let world = fixture_world();
    let out = world.parent().unwrap().join("out");
    let request = OptimizerRequestBuilder::new(world)
        .output(Some(out.clone()))
        .inhabited_threshold_ticks(Some(0))
        .progress_mode(ProgressMode::Off)
        .pattern(Box::new(rust_thanos::world::RangePattern::new(2, 2, 2, 2)))
        .build();
    let report = optimize(&request).unwrap();

    assert!(report.kept_chunks > 0, "range pattern keeps the rectangle");
    let mut r = McaReader::open(out.join("region").join("r.0.0.mca").to_str().unwrap()).unwrap();
    let kept = r.get_chunk_at(2, 2).unwrap();
    assert!(kept.is_some(), "chunk (2,2) kept by range pattern");
    // index = x + z*32 = 2 + 2*32 = 66
    assert_eq!(kept.unwrap().region_index(), 66);
}
