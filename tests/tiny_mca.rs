use std::fs;
// removed unused import
use rust_thanos::world::ProgressMode;

#[test]
fn tiny_mca_is_skipped_without_error() {
    let base = std::env::temp_dir().join(format!("rt-tiny-{}", uuid::Uuid::new_v4()));
    let world = base.join("world");
    fs::create_dir_all(world.join("region")).unwrap();
    // create a tiny file (<8192 bytes)
    fs::write(world.join("region").join("r.0.0.mca"), vec![0u8; 100]).unwrap();
    let out = base.join("out");
    rust_thanos::world::run(
        world.clone(),
        Some(out.clone()),
        0,
        false,
        ProgressMode::Off,
    )
    .unwrap();
    // Should complete and create output structure
    assert!(out.join("region").exists());
    // The unparseable region is recorded as skipped but byte-preserved in the
    // output (non-in-place runs never silently drop a region they can't parse).
    let preserved = fs::read(out.join("region").join("r.0.0.mca")).unwrap();
    assert_eq!(preserved, vec![0u8; 100], "tiny region preserved as-is");
}
