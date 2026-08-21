//! End-to-end force-loaded chunk tests against REAL fixtures copied from
//! OrzMCBackup: the legacy `data/chunks.dat` (Forge Forced LongArray) and the
//! 26.1+ `data/minecraft/chunk_tickets.dat` (minecraft:forced tickets).

use rust_thanos::mca::reader::McaReader;
use rust_thanos::world::ProgressMode;
use std::fs;
use std::path::Path;

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

/// Maps global chunk coords inside region r.0.0 (x,z ∈ 0..32) to their region
/// index (z*32+x), sorted.
fn indices_for_coords(coords: &[(i32, i32)]) -> Vec<u32> {
    let mut v: Vec<u32> = coords
        .iter()
        .filter(|(x, z)| (0..32).contains(x) && (0..32).contains(z))
        .map(|(x, z)| (z * 32 + x) as u32)
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// Legacy Forge `data/chunks.dat` keeps the force-loaded chunk even at a very
/// high threshold. The real fixture marks global chunk (1,1) → region index 33.
#[test]
fn legacy_chunks_dat_keeps_forced_chunk() {
    let base = std::env::temp_dir().join(format!("rt-forced-legacy-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&base).unwrap();
    let world = base.join("world");
    fs::create_dir_all(world.join("region")).unwrap();
    fs::create_dir_all(world.join("data")).unwrap();
    fs::copy(
        "tests/Fixtures/world/region/r.0.0.mca",
        world.join("region").join("r.0.0.mca"),
    )
    .unwrap();
    fs::copy(
        "tests/Fixtures/world/data/chunks.dat",
        world.join("data").join("chunks.dat"),
    )
    .unwrap();

    let forced = rust_thanos::world::parse_force_loaded(&world);
    assert_eq!(forced, vec![(1, 1)]);

    let out = base.join("out");
    rust_thanos::world::run(
        world.clone(),
        Some(out.clone()),
        1_000_000,
        false,
        ProgressMode::Off,
    )
    .unwrap();
    assert_eq!(output_indices(&out), vec![33]); // global (1,1)
}

/// Modern `data/minecraft/chunk_tickets.dat` (26.1+) keeps the force-loaded
/// chunks. The real fixture marks a 2x2 block (0,0),(0,1),(1,0),(1,1).
#[test]
fn chunk_tickets_dat_keeps_forced_chunks() {
    let base = std::env::temp_dir().join(format!("rt-forced-tickets-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&base).unwrap();
    // Copy the whole 26.1 overworld dimension: region + data (incl. both the
    // modern chunk_tickets.dat and the legacy chunks.dat).
    let dim = base.join("dim");
    fs::create_dir_all(&dim).unwrap();
    let mut opts = fs_extra::dir::CopyOptions::new();
    opts.content_only = true;
    fs_extra::dir::copy(
        "tests/Fixtures/world-26-1/dimensions/minecraft/overworld",
        &dim,
        &opts,
    )
    .unwrap();

    // First file in priority order wins: chunk_tickets.dat (not chunks.dat).
    let forced = rust_thanos::world::parse_force_loaded(&dim);
    assert_eq!(forced, vec![(0, 0), (0, 1), (1, 0), (1, 1)]);
    let expected = indices_for_coords(&forced);

    let out = base.join("out");
    rust_thanos::world::run(
        dim.clone(),
        Some(out.clone()),
        1_000_000,
        false,
        ProgressMode::Off,
    )
    .unwrap();
    assert_eq!(output_indices(&out), expected);
}
