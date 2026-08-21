//! Phase 4 e2e: the world merger.
//!
//! Merges a real committed Anvil-format fixture pair (`tests/Fixtures/merge/`),
//! proving the slot-level merge (patch wins, else base, else empty; entities/poi
//! lockstep) works on genuine on-disk region files through the production
//! `RealFileSystem` + `DefaultMcaIOFactory` path, that parallelism is
//! byte-identical to serial, and that a corrupt patch restores the base copy
//! byte-for-byte.

use rust_thanos::mca::reader::McaReader;
use rust_thanos::world::{
    merge, ErrorKind, FileSystem, MemoryFS, MemoryMcaIOFactory, MergeReport, MergeReportIO,
    MergeRequestBuilder,
};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

const FIXTURES: &str = "tests/Fixtures/merge";
const DIM: &str = "dimensions/minecraft/overworld";

/// Copies `src` fixture subtree into `dst` (new temp dir).
fn copy_fixture(src: &str, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    let mut opts = fs_extra::dir::CopyOptions::new();
    opts.content_only = true;
    fs_extra::dir::copy(src, dst, &opts).unwrap();
}

/// A fresh base/patch fixture pair in a temp dir.
fn fixture_pair() -> (PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!("rt-m4-{}", uuid::Uuid::new_v4()));
    let base = root.join("base");
    let patch = root.join("patch");
    copy_fixture(&format!("{FIXTURES}/base"), &base);
    copy_fixture(&format!("{FIXTURES}/patch"), &patch);
    (base, patch)
}

fn run_merge(base: PathBuf, patch: PathBuf, out: PathBuf, parallelism: usize) -> MergeReport {
    let request = MergeRequestBuilder::new(base, patch, out)
        .parallelism(parallelism)
        .build();
    merge(&request).unwrap()
}

/// Count used chunk slots by inspecting the 4096-byte location table.
fn slot_count_bytes(bytes: &[u8]) -> usize {
    bytes[0..4096]
        .chunks(4)
        .filter(|c| c.iter().any(|b| *b != 0))
        .count()
}

fn slot_count(path: &str) -> usize {
    slot_count_bytes(&fs::read(path).unwrap())
}

/// The fixture NBT ends with the 8-byte InhabitedTime long followed by TAG_End,
/// so the long is the 9 bytes from the end.
fn read_inhabited_bytes(bytes: &[u8], slot: usize) -> i64 {
    let mut r = McaReader::open_from_bytes("r.0.0.mca", bytes.to_vec()).unwrap();
    let mut e = r.get(slot).unwrap().expect("slot present");
    let payload = e.all_data_uncompressed().unwrap();
    let start = payload.len() - 9;
    i64::from_be_bytes(payload[start..start + 8].try_into().unwrap())
}

fn read_inhabited(path: &str, slot: usize) -> i64 {
    read_inhabited_bytes(&fs::read(path).unwrap(), slot)
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

#[test]
fn merge_real_anvil_slot_by_slot() {
    let (base, patch) = fixture_pair();
    let out = base.parent().unwrap().join("out");
    let report = run_merge(base, patch, out.clone(), 4);

    assert!(report.errors.is_empty(), "no errors: {:?}", report.errors);
    assert_eq!(report.merged_regions, 1);
    assert_eq!(report.patch_slots, 40);
    assert_eq!(report.base_slots, 60);
    assert_eq!(report.linked_entities, 100, "entities merge lockstep");
    assert_eq!(report.linked_poi, 100, "poi merge lockstep");
    assert_eq!(
        report.copied_files, 0,
        "no patch-only regions in this fixture"
    );
    assert_eq!(report.overlay_files, 1, "patch level.dat overlaid");

    let dim = out.join(DIM);
    let region = dim.join("region/r.0.0.mca");
    assert_eq!(
        slot_count(region.to_str().unwrap()),
        100,
        "merged region keeps every base and patch slot"
    );
    assert_eq!(
        read_inhabited(region.to_str().unwrap(), 0),
        2000,
        "patch chunk wins"
    );
    assert_eq!(
        read_inhabited(region.to_str().unwrap(), 39),
        2039,
        "patch chunk wins"
    );
    assert_eq!(
        read_inhabited(region.to_str().unwrap(), 40),
        1040,
        "pruned slot filled from base"
    );
    assert_eq!(
        read_inhabited(region.to_str().unwrap(), 99),
        1099,
        "pruned slot filled from base"
    );

    assert_eq!(
        slot_count(dim.join("entities/r.0.0.mca").to_str().unwrap()),
        100
    );
    assert_eq!(slot_count(dim.join("poi/r.0.0.mca").to_str().unwrap()), 100);

    // Base-only region preserved; non-MCA files come from the newer patch.
    assert_eq!(
        slot_count(dim.join("region/r.1.1.mca").to_str().unwrap()),
        10
    );
    assert_eq!(fs::read(out.join("level.dat")).unwrap(), b"patch-level-dat");
}

#[test]
fn merge_parallelism_is_deterministic() {
    let (b1, p1) = fixture_pair();
    let (b2, p2) = fixture_pair();
    let out1 = b1.parent().unwrap().join("out1");
    let out2 = b2.parent().unwrap().join("out2");

    let r1 = run_merge(b1, p1, out1.clone(), 1);
    let r2 = run_merge(b2, p2, out2.clone(), 4);

    assert_eq!(r1.patch_slots, r2.patch_slots, "same patch slots");
    assert_eq!(r1.base_slots, r2.base_slots, "same base slots");
    assert_eq!(r1.merged_regions, r2.merged_regions);
    assert_eq!(
        snapshot(&out1),
        snapshot(&out2),
        "parallel and serial merges must be byte-identical"
    );
}

#[test]
fn merge_corrupt_patch_restores_base() {
    let (base, patch) = fixture_pair();
    // Too small to be a valid MCA (< 8192 bytes) → the base copy is restored
    // byte-for-byte so nothing is dropped.
    fs::write(
        patch.join(format!("{DIM}/region/r.0.0.mca")),
        vec![0u8; 100],
    )
    .unwrap();
    let out = base.parent().unwrap().join("out");
    let report = run_merge(base.clone(), patch, out.clone(), 2);

    assert_eq!(
        report.merged_regions, 1,
        "corrupt patch region still counts as merged"
    );
    assert_eq!(report.patch_slots, 0, "no patch slots survived");
    // Byte-copy restore path (mergeBaseIfPresent) doesn't slot-count, matching
    // OrzMCBackup: the whole region comes back verbatim, not slot-by-slot.
    assert_eq!(report.base_slots, 0, "byte-copy restore doesn't slot-count");

    let dim = out.join(DIM);
    let read = |rel: &str| fs::read(dim.join(rel)).unwrap();
    let base_read = |rel: &str| fs::read(base.join(format!("{DIM}/{rel}"))).unwrap();
    assert_eq!(
        read("region/r.0.0.mca"),
        base_read("region/r.0.0.mca"),
        "region restored"
    );
    assert_eq!(
        read("entities/r.0.0.mca"),
        base_read("entities/r.0.0.mca"),
        "entities restored"
    );
    assert_eq!(
        read("poi/r.0.0.mca"),
        base_read("poi/r.0.0.mca"),
        "poi restored"
    );
}

#[test]
fn merge_corrupt_patch_garbage_records_error() {
    let (base, patch) = fixture_pair();
    // 8192+ bytes of garbage: passes the size check, fails on read → recorded.
    fs::write(
        patch.join(format!("{DIM}/region/r.0.0.mca")),
        vec![0xFFu8; 8192],
    )
    .unwrap();
    let out = base.parent().unwrap().join("out");
    let report = run_merge(base, patch, out, 1);
    let kinds: Vec<ErrorKind> = report.errors.iter().map(|e| e.kind).collect();
    assert!(
        kinds.contains(&ErrorKind::Mca),
        "garbage patch region must record Mca, got {:?}",
        kinds
    );
}

#[test]
fn merge_report_io_json_csv_text() {
    let report = MergeReport {
        merged_regions: 1,
        copied_files: 2,
        patch_slots: 40,
        base_slots: 60,
        linked_entities: 100,
        linked_poi: 99,
        overlay_files: 3,
        errors: vec![rust_thanos::world::OptimizeError {
            path: format!("{DIM}/region/r.0.0.mca"),
            kind: ErrorKind::Mca,
            message: "Failed to merge region file: read failed".into(),
        }],
    };

    let json = MergeReportIO::to_json(&report);
    assert!(json.contains("\"mergedRegions\":1"));
    assert!(json.contains("\"patchSlots\":40"));
    assert!(json.contains("\"baseSlots\":60"));
    assert!(json.contains("\"linkedEntities\":100"));
    assert!(json.contains("\"kind\":\"Mca\""));

    let csv = MergeReportIO::to_csv(&report);
    assert!(csv.starts_with("mergedRegions,copiedFiles,patchSlots,baseSlots"));
    assert!(csv.contains(&format!("\"{DIM}/region/r.0.0.mca\",\"Mca\"")));

    let text = MergeReportIO::to_text(&report);
    assert!(text.contains("Statistics: mergedRegions=1 patchSlots=40 baseSlots=60"));
    assert!(text.contains("[Mca] dimensions/minecraft/overworld/region/r.0.0.mca"));

    // Lossy mapping into the optimizer report model for ReportSink consumers.
    let opt = MergeReportIO::to_optimize_report(&report);
    assert_eq!(opt.processed_chunks, 40);
    assert_eq!(opt.removed_chunks, 60);
    assert_eq!(opt.errors.len(), 1);
}

#[test]
fn merge_overlap_guard_rejects() {
    let (base, patch) = fixture_pair();
    // output == base: the guard must abort before any write.
    let report = run_merge(base.clone(), patch, base.clone(), 1);
    let kinds: Vec<ErrorKind> = report.errors.iter().map(|e| e.kind).collect();
    assert!(
        kinds.contains(&ErrorKind::Input),
        "overlapping output must record Input, got {:?}",
        kinds
    );
    assert_eq!(report.merged_regions, 0, "no merge performed");
}

#[test]
fn merge_memory_fs_end_to_end() {
    let mem = Arc::new(MemoryFS::new());
    let rels = [
        "base/dimensions/minecraft/overworld/region/r.0.0.mca",
        "base/dimensions/minecraft/overworld/entities/r.0.0.mca",
        "base/dimensions/minecraft/overworld/poi/r.0.0.mca",
        "base/dimensions/minecraft/overworld/region/r.1.1.mca",
        "patch/dimensions/minecraft/overworld/region/r.0.0.mca",
        "patch/dimensions/minecraft/overworld/entities/r.0.0.mca",
        "patch/dimensions/minecraft/overworld/poi/r.0.0.mca",
    ];
    for rel in rels {
        let bytes = fs::read(format!("{FIXTURES}/{rel}")).unwrap();
        mem.write(&Path::new("/world").join(rel), &bytes).unwrap();
    }
    let request = MergeRequestBuilder::new(
        PathBuf::from("/world/base"),
        PathBuf::from("/world/patch"),
        PathBuf::from("/world/out"),
    )
    .fs(mem.clone())
    .io_factory(Arc::new(MemoryMcaIOFactory))
    .build();
    let report = merge(&request).unwrap();
    assert!(
        report.errors.is_empty(),
        "memory merge errors: {:?}",
        report.errors
    );
    assert_eq!(report.patch_slots, 40);
    assert_eq!(report.base_slots, 60);
    let out_bytes = mem
        .read(Path::new(
            "/world/out/dimensions/minecraft/overworld/region/r.0.0.mca",
        ))
        .unwrap()
        .expect("merged region present");
    assert_eq!(slot_count_bytes(&out_bytes), 100);
    assert_eq!(
        read_inhabited_bytes(&out_bytes, 0),
        2000,
        "patch chunk wins"
    );
    assert_eq!(
        read_inhabited_bytes(&out_bytes, 40),
        1040,
        "base fills pruned slot"
    );
}

#[test]
fn merge_cli_report_and_exit_code() {
    let (base, patch) = fixture_pair();
    let root = base.parent().unwrap();
    let out = root.join("out");
    let rpt = root.join("report.json");

    let bin = env!("CARGO_BIN_EXE_rust-thanos");
    let status = Command::new(bin)
        .args([
            "merge",
            base.to_str().unwrap(),
            patch.to_str().unwrap(),
            out.to_str().unwrap(),
            "--report",
            "--report-file",
            rpt.to_str().unwrap(),
            "--report-format",
            "json",
            "--parallelism",
            "4",
        ])
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "clean merge exits 0, stdout: {}",
        String::from_utf8_lossy(&status.stdout)
    );
    let rpt_txt = fs::read_to_string(&rpt).unwrap();
    assert!(
        rpt_txt.contains("\"patchSlots\":40"),
        "report json: {rpt_txt}"
    );
    assert!(rpt_txt.contains("\"baseSlots\":60"));
    let stdout = String::from_utf8_lossy(&status.stdout);
    assert!(
        stdout.contains("Statistics: mergedRegions=1"),
        "text report printed"
    );

    // Corrupt patch region (garbage ≥ 8192 B) → error recorded → exit 1.
    let (base2, patch2) = fixture_pair();
    fs::write(
        patch2.join(format!("{DIM}/region/r.0.0.mca")),
        vec![0xFFu8; 8192],
    )
    .unwrap();
    let out2 = base2.parent().unwrap().join("out2");
    let rpt2 = base2.parent().unwrap().join("report2.json");
    let status2 = Command::new(bin)
        .args([
            "merge",
            base2.to_str().unwrap(),
            patch2.to_str().unwrap(),
            out2.to_str().unwrap(),
            "--report-file",
            rpt2.to_str().unwrap(),
            "--report-format",
            "json",
        ])
        .output()
        .unwrap();
    assert!(
        !status2.status.success(),
        "merge with errors exits 1, stdout: {}",
        String::from_utf8_lossy(&status2.stdout)
    );
    let rpt2_txt = fs::read_to_string(&rpt2).unwrap();
    assert!(
        rpt2_txt.contains("\"kind\":\"Mca\""),
        "report has Mca error: {rpt2_txt}"
    );
}
