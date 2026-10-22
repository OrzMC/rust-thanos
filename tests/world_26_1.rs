//! 26.1+ 整世界端到端（Paper `dimensions/` 布局）：优化后输出仍是可用世界。
//!
//! 对应 issue #1「支持 26.1 及后续版本的世界备份和优化」。fixture 是从 OrzMCBackup 的
//! 真实 26.1 世界复制的，结构与旧版不同：
//!
//! * 维度在 `dimensions/minecraft/<dim>/{region,entities,poi,data}` 下，**不是**顶层 `region/`；
//! * 强加载文件是**每维度一份** `data/minecraft/chunk_tickets.dat`（旧版是全局 `data/chunks.dat`）；
//! * 世界级文件（`level.dat`、`players/`、`data/`、`generated/`）仍在世界根目录。
//!
//! 这里守三条容易回归的语义：维度发现、逐维度强加载在内层目录也能生效、非 MCA 文件
//! 按「世界级 + 维度级」一起带走（而 `session.lock` 不带）。

use rust_thanos::mca::reader::McaReader;
use rust_thanos::world::{
    parse_force_loaded, DefaultOptimizer, OptimizerEngine, OptimizerRequestBuilder, ProgressMode,
};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

const FIXTURE: &str = "tests/Fixtures/world-26-1";
const DIMENSIONS: [&str; 3] = ["overworld", "the_nether", "the_end"];

/// 把 fixture 整份复制到临时目录（测试会写入输出目录，不动仓库里的 fixture）。
fn fixture_world() -> PathBuf {
    let base = std::env::temp_dir().join(format!("rt-26-1-{}", uuid::Uuid::new_v4()));
    let world = base.join("world");
    fs::create_dir_all(&world).unwrap();
    let mut opts = fs_extra::dir::CopyOptions::new();
    opts.content_only = true;
    fs_extra::dir::copy(FIXTURE, &world, &opts).unwrap();
    world
}

/// 某个 region 文件里实际存在的区块槽位（按 region 内索引）。
fn region_indices(path: &Path) -> BTreeSet<u32> {
    let mut reader = McaReader::open(path.to_string_lossy().as_ref()).unwrap();
    reader
        .entries()
        .unwrap()
        .iter_mut()
        .map(|entry| entry.region_index())
        .collect()
}

/// 区块全局坐标 → (region 文件名, region 内索引)。26.1 fixture 的强加载点都在 r.0.0，
/// 但按公式算更稳（将来换 fixture 也不用改断言）。
fn region_slot(x: i32, z: i32) -> (String, u32) {
    let (rx, rz) = (x >> 5, z >> 5);
    let index = ((z - rz * 32) * 32 + (x - rx * 32)) as u32;
    (format!("r.{rx}.{rz}.mca"), index)
}

#[test]
fn optimizes_the_whole_26_1_world_and_keeps_it_usable() {
    let world = fixture_world();
    let out = world.parent().unwrap().join("out");

    let request = OptimizerRequestBuilder::new(world.clone())
        .output(Some(out.clone()))
        // 阈值给到极高：除了「强加载」没有任何区块该留下来。
        .inhabited_threshold_ticks(Some(20_000_000))
        .progress_mode(ProgressMode::Off)
        .build();
    let report = DefaultOptimizer.run(&request).expect("optimize run");

    assert!(report.kept_chunks > 0, "强加载区块应该被保住: {report:?}");
    assert!(
        report.removed_chunks > 0,
        "高于阈值的区块应该被删: {report:?}"
    );

    for dim in DIMENSIONS {
        let dim_in = world.join("dimensions/minecraft").join(dim);
        let dim_out = out.join("dimensions/minecraft").join(dim);

        // 1) 维度被发现并写出（老逻辑只看顶层 region/，这里必须是 dimensions/<dim>/）。
        assert!(
            dim_out.join("region").is_dir(),
            "26.1 维度目录没被处理: {}",
            dim_out.display()
        );

        // 2) 逐维度强加载：该维度 chunk_tickets.dat 里点名的区块必须一个不少地留下。
        let forced = parse_force_loaded(&dim_in);
        assert!(!forced.is_empty(), "fixture 的 {dim} 应有强加载区块");
        let mut expected: std::collections::BTreeMap<String, BTreeSet<u32>> = Default::default();
        for (x, z) in forced {
            let (file, index) = region_slot(x, z);
            expected.entry(file).or_default().insert(index);
        }
        for (file, indices) in expected {
            let kept = region_indices(&dim_out.join("region").join(&file));
            assert!(
                indices.is_subset(&kept),
                "{dim}/{file} 丢了强加载区块: 期望包含 {indices:?}, 实际 {kept:?}"
            );
        }

        // 3) 维度级非 MCA 文件（game_rules/world_border/paper 数据/强加载文件）跟着走。
        for rel in [
            "data/minecraft/chunk_tickets.dat",
            "data/minecraft/game_rules.dat",
            "data/paper/metadata.dat",
        ] {
            assert!(dim_out.join(rel).is_file(), "{dim}/{rel} 没被复制");
        }
    }

    // 4) 世界级文件（旧版与 26.1 相同的位置）也必须在输出里，否则世界起不来。
    for rel in [
        "level.dat",
        "data/minecraft/scoreboard.dat",
        "generated/minecraft/structure",
    ] {
        assert!(out.join(rel).exists(), "世界级 {rel} 没被复制");
    }
    assert!(
        out.join("players/data").is_dir(),
        "players/ 没被复制（玩家数据会全丢）"
    );

    // 5) session.lock 是运行时锁，复制过去会让服务端认为世界被占用。
    assert!(
        !out.join("session.lock").exists(),
        "session.lock 不该被复制"
    );
}
