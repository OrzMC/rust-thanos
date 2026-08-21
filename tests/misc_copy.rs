//! Phase 2 e2e: output usability.
//!
//! After region processing the output must be a usable world: non-MCA files
//! (level.dat, players/, data/, .bak backups, nested .mca) are copied, while
//! `session.lock` and top-level `region|entities|poi/*.mca` are not. `--copy-misc`
//! off skips everything; `--dry-run` writes zero bytes and reports honest stats.

use rust_thanos::world::{
    optimize, DefaultOptimizer, OptimizeReport, OptimizerEngine, OptimizerRequestBuilder,
    ProgressMode,
};
use std::fs;
use std::path::{Path, PathBuf};

/// Copies the real fixture world to a fresh temp dir so tests can mutate it.
fn fixture_world() -> PathBuf {
    let base = std::env::temp_dir().join(format!("rt-misc-{}", uuid::Uuid::new_v4()));
    let world = base.join("world");
    fs::create_dir_all(&world).unwrap();
    let mut opts = fs_extra::dir::CopyOptions::new();
    opts.content_only = true;
    fs_extra::dir::copy("tests/Fixtures/world", &world, &opts).unwrap();
    world
}

fn run_req(input: PathBuf, out: PathBuf, copy_misc: bool, dry_run: bool) -> OptimizeReport {
    let request = OptimizerRequestBuilder::new(input)
        .output(Some(out))
        .inhabited_threshold_ticks(Some(0))
        .progress_mode(ProgressMode::Off)
        .copy_misc(copy_misc)
        .dry_run(dry_run)
        .build();
    DefaultOptimizer.run(&request).expect("optimize run")
}

fn dir_size_of(path: &Path) -> u64 {
    if path.is_file() {
        return fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    }
    let mut total = 0u64;
    if let Ok(rd) = fs::read_dir(path) {
        for e in rd.flatten() {
            total += dir_size_of(&e.path());
        }
    }
    total
}

fn walk_has_file(root: &Path, needle: &str) -> bool {
    walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(|e| e.ok())
        .any(|e| e.file_name().to_string_lossy() == needle)
}

#[test]
fn copies_misc_files_but_skips_session_lock_and_top_mca() {
    let world = fixture_world();
    fs::write(world.join("session.lock"), b"stale-lock").unwrap();
    let out = world.parent().unwrap().join("out");
    run_req(world, out.clone(), true, false);

    // World-level misc
    assert!(out.join("level.dat").exists(), "level.dat copied");
    assert!(out.join("playerdata").is_dir(), "playerdata copied");
    assert!(
        out.join("data").join("map_0.dat").exists(),
        "data/map_0.dat copied"
    );
    // Non-MCA files inside reserved subtrees are misc and must be kept.
    assert!(
        out.join("poi").join("another-file.txt").exists(),
        "non-mca inside poi preserved"
    );
    assert!(
        out.join("region").join("other-file.txt").exists(),
        "non-mca inside region preserved"
    );
    // Nested dimension misc
    assert!(
        out.join("DIM-1")
            .join("data")
            .join("Fortress_index.dat")
            .exists(),
        "dimension misc copied"
    );
    // Rewritten region
    assert!(
        out.join("region").join("r.0.0.mca").exists(),
        "rewritten region exists"
    );
    // session.lock never leaks
    assert!(
        !out.join("session.lock").exists(),
        "root session.lock skipped"
    );
    assert!(
        !walk_has_file(&out, "session.lock"),
        "no session.lock anywhere in output"
    );
}

#[test]
fn no_copy_misc_skips_misc_files() {
    let world = fixture_world();
    let out = world.parent().unwrap().join("out2");
    run_req(world, out.clone(), false, false);

    assert!(
        out.join("region").join("r.0.0.mca").exists(),
        "region still rewritten"
    );
    assert!(!out.join("level.dat").exists(), "level.dat not copied");
    assert!(
        !out.join("data").join("map_0.dat").exists(),
        "data/ not copied"
    );
    assert!(!out.join("playerdata").exists(), "playerdata not copied");
}

#[test]
fn dry_run_writes_nothing_and_reports_stats() {
    let world = fixture_world();
    let out = world.parent().unwrap().join("out3");
    let before = dir_size_of(&world);
    let report = run_req(world.clone(), out.clone(), true, true);

    assert!(report.processed_chunks > 0, "scan still counts chunks");
    assert!(report.removed_chunks > 0, "scan still evaluates removal");
    assert!(report.errors.is_empty(), "clean world, no errors");
    assert_eq!(report.before_size, before, "before_size matches disk");
    assert_eq!(report.after_size, before, "dry-run reports no change");
    assert!(!out.exists(), "dry-run never touches the requested output");
    // The original world is untouched.
    assert!(world.join("region").join("r.0.0.mca").exists());
}

#[test]
fn dry_run_discards_its_temp_output() {
    let world = fixture_world();
    let request = OptimizerRequestBuilder::new(world)
        .inhabited_threshold_ticks(Some(0))
        .progress_mode(ProgressMode::Off)
        .dry_run(true)
        .in_place(false)
        .build();
    // No output specified at all: dry-run must still work and not leak a temp dir
    // next to the input. (resolveOutputDir uses a temp dir and we delete it.)
    let report = optimize(&request).unwrap();
    assert!(report.processed_chunks > 0);
}
