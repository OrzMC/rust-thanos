use rust_thanos::mca::reader::McaReader;
use rust_thanos::world::ProgressMode;
use std::fs;
use std::path::{Path, PathBuf};

/// The threshold tests run against the REAL fixture chunks
/// (`tests/Fixtures/world/region/r.0.0.mca`, copied from OrzMCBackup's test
/// fixtures). That region has 676 chunks whose InhabitedTime values are spread
/// over 0..=40, so exact keep/remove counts are asserted instead of
/// hand-crafted synthetic NBT.
const FIXTURE_REGION: &str = "tests/Fixtures/world/region/r.0.0.mca";

/// Copies the real region into a fresh temp world that has ONLY a `region/`
/// dir (no `data/chunks.dat`), so force-loaded chunks can never affect the
/// threshold counts.
fn region_only_world(base: &Path) -> PathBuf {
    let world = base.join("world");
    fs::create_dir_all(world.join("region")).unwrap();
    fs::copy(FIXTURE_REGION, world.join("region").join("r.0.0.mca")).unwrap();
    world
}

/// Opens the output region and returns its sorted chunk region-indices.
fn output_indices(out: &Path) -> Vec<u32> {
    let mut r = McaReader::open(
        out.join("region")
            .join("r.0.0.mca")
            .to_string_lossy()
            .as_ref(),
    )
    .unwrap();
    let mut v: Vec<u32> = r
        .entries()
        .unwrap()
        .iter_mut()
        .map(|e| e.region_index())
        .collect();
    v.sort_unstable();
    v
}

/// Exact keep-counts for the real fixture region at each threshold (ticks),
/// verified against the raw NBT of every chunk:
///   >0 → 58, >10 → 39, >20 → 15, >27 → 8, >30 → 2, >40 → 0
#[test]
fn real_fixture_threshold_counts() {
    let cases = [(0, 58), (10, 39), (20, 15), (27, 8), (30, 2), (40, 0)];
    for (threshold, expected) in cases {
        let base =
            std::env::temp_dir().join(format!("rt-thr-{}-{}", threshold, uuid::Uuid::new_v4()));
        fs::create_dir_all(&base).unwrap();
        let world = region_only_world(&base);
        let out = base.join("out");
        rust_thanos::world::run(
            world,
            Some(out.clone()),
            threshold,
            false,
            ProgressMode::Off,
        )
        .unwrap();
        if expected == 0 {
            // Strictly nothing kept: the lazy writer creates no region file.
            assert!(
                !out.join("region").join("r.0.0.mca").exists(),
                "threshold {} should produce no output region",
                threshold
            );
        } else {
            assert_eq!(
                output_indices(&out).len(),
                expected,
                "kept count at threshold {}",
                threshold
            );
        }
    }
}

/// Strict-`>` boundary on real data. Chunks with InhabitedTime == 27 exist
/// (region-indices 0, 1, 2). At threshold 27 they must be REMOVED: with the old
/// `>=` semantics they would be kept (11 kept, not 8).
#[test]
fn real_fixture_strict_greater_boundary() {
    let base = std::env::temp_dir().join(format!("rt-boundary-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&base).unwrap();
    let world = region_only_world(&base);

    let out26 = base.join("out26");
    rust_thanos::world::run(
        world.clone(),
        Some(out26.clone()),
        26,
        false,
        ProgressMode::Off,
    )
    .unwrap();
    let kept26 = output_indices(&out26);
    assert_eq!(kept26.len(), 11, ">26 keeps the three 27-value chunks");

    let out27 = base.join("out27");
    rust_thanos::world::run(
        world.clone(),
        Some(out27.clone()),
        27,
        false,
        ProgressMode::Off,
    )
    .unwrap();
    let kept27 = output_indices(&out27);
    assert_eq!(
        kept27.len(),
        8,
        "strict >27 must drop the three 27-value chunks"
    );
    for idx in [0u32, 1, 2] {
        assert!(
            !kept27.contains(&idx),
            "chunk {} (InhabitedTime == 27) must be removed at threshold 27",
            idx
        );
    }
}
