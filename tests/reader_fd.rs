//! Regression: the MCA reader must not open one file descriptor per chunk entry.
//!
//! `McaReader::entries()` clones its access handle once per populated chunk
//! slot. When that clone used to `File::try_clone()` — one new fd each — a
//! large world (21k regions x up to ~700 chunks) exhausted the process ulimit
//! and silently dropped ~30% of regions with "Too many open files". `FileAccess`
//! now shares a single `Arc<Mutex<File>>` across clones, so this test asserts
//! creating all 676 entries of the fixture region costs at most a handful of
//! fds. Linux only: `/proc/self/fd` does not exist on macOS/Windows.

#[cfg(target_os = "linux")]
use rust_thanos::mca::reader::McaReader;

#[cfg(target_os = "linux")]
#[test]
fn chunk_entries_share_region_fd() {
    fn open_fds() -> usize {
        std::fs::read_dir("/proc/self/fd")
            .map(|d| d.count())
            .unwrap_or(0)
    }

    let baseline = open_fds();
    let mut reader = McaReader::open("tests/Fixtures/world/region/r.0.0.mca").unwrap();
    let entries = reader.entries().unwrap();
    assert_eq!(entries.len(), 676, "fixture region has 676 chunk slots");
    let after = open_fds();
    assert!(
        after <= baseline + 32,
        "676 chunk entries must share one region fd (baseline={baseline} after={after})"
    );
    drop(entries);
}
