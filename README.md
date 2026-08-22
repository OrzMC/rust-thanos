# rust-thanos

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://github.com/OrzMC/rust-thanos/actions/workflows/ci.yml/badge.svg)](https://github.com/OrzMC/rust-thanos/actions/workflows/ci.yml)

> 🌐 宣传网页：[https://orzmc.github.io/rust-thanos/](https://orzmc.github.io/rust-thanos/)

高效的 Minecraft Java 世界优化工具。检测并删除未使用的区块，减少世界文件大小，提高游戏性能；并提供 **merge 合并恢复** 命令，把优化后的世界安全合并回完整备份。

rust-thanos 是 [OrzMCBackup](https://github.com/OrzMC/OrzMCBackup)（Java/Kotlin 版，即 thanos PHP 原版的超集）的 **Rust 版 CLI + 库**，算法与命令行行为对齐 OrzMCBackup。

## 🌟 特性

- **智能区块检测**：`InhabitedTime`（严格大于阈值）、强加载区块列表与 `--keep-range` 多通道取并集判定，未知压缩格式默认安全保留
- **多种压缩格式支持**：兼容 RAW、ZLIB、GZIP、LZ4 Block（含校验）
- **数据安全**：损坏/无法解析的区块**安全保留**而非删除；损坏区块记录到报告；原地模式跳过打开失败/过小的输入文件，绝不丢数据；解压有 64MB 上限防解压炸弹
- **输出即可用世界**：自动复制 `level.dat`、`players/`、`data/`、`.bak` 等非 MCA 文件，输出目录可直接当世界用
- **merge 合并恢复**：把优化后的世界（patch）按区块槽级合并回完整备份（base），合并结果 = 备份的全量 + 优化后仍存活的最新区块，entities/poi 与区块锁步，无法打开的 patch region 逐字节回退 base
- **并行处理**：region 级并行（`--parallelism`，默认按 CPU 核数），任意并行度下输出逐字节一致
- **脚本友好**：JSON/CSV/Text 报告、`--strict` 退出码、`--dry-run` 零写入演练、Region/Global 进度显示
- **灵活配置**：可自定义 `InhabitedTime` 阈值、`--keep-range` 保留矩形、`--no-fsync` 加速落盘
- **递归维度识别**：自动扫描输入目录下所有含 region 的维度目录（Vanilla DIM-1/DIM1、Paper `world_nether`/`world_the_end`、自定义维度）
- **便捷归档**：非原地模式支持把输出压缩为 `YYYYMMddHHmmss.zip`
- **库 + CLI 双形态**：完整配置模型、IO 抽象（`FileSystem` / `McaIOFactory`）、hooks（`on_error` / `report_sink` / `metrics_sink`）、内存文件系统，可作库嵌入

## 📦 安装

### 通过 Homebrew 安装

```bash
# 添加私有源
brew tap orzgeeker/core
# 安装
brew install rust-thanos
# 卸载
brew uninstall rust-thanos
# 移除私有源
brew untap orzgeeker/core
```

### 从源代码构建

```bash
git clone https://github.com/OrzMC/rust-thanos.git
cd rust-thanos
cargo build --release          # 构建
cargo test                     # 运行测试（可选）
cargo install --path .         # 安装到系统（可选）
```

### 下载预编译二进制文件

预编译二进制可在 [Releases](https://github.com/OrzMC/rust-thanos/releases) 页面下载，开箱即用（无需安装运行时）：

| 平台 | 架构 | 产物格式 |
| --- | --- | --- |
| macOS | arm64 / x86_64 | `.tar.gz` |
| Linux | arm64 / x86_64 | `.tar.gz` |
| Windows | arm64 / x86_64 | `.zip` |

## 🚀 使用

`rust-thanos` 有两个子命令：

```
rust-thanos backup <WORLD_DIR> [OUTPUT_DIR] [选项]   # 优化世界（默认工作流）
rust-thanos merge  <BASE_DIR> <PATCH_DIR> <OUTPUT>   [选项]   # 合并恢复
```

### backup — 优化世界

#### 基本语法

```bash
rust-thanos backup WORLD_DIR [OUTPUT_DIR] \
  [-t|--inhabited-time-seconds <seconds>] [--remove-unknown] \
  [--progress-mode <mode>] [--in-place] [--zip-output] [-f|--force] \
  [--parallelism <n>] [--no-fsync] [--strict] [--dry-run] \
  [--copy-misc|--no-copy-misc] \
  [--keep-range <x1> <z1> <x2> <z2>] \
  [--progress-interval <n>] [--progress-interval-ms <ms>] \
  [--report] [--report-file <file>] [--report-format <json|csv|text>]
```

#### 参数说明

| 参数 | 描述 |
|------|------|
| `WORLD_DIR` | Minecraft 世界根目录（必填） |
| `OUTPUT_DIR` | 输出目录，必须为空；已存在且非空时提示是否覆盖，`-f` 跳过提示；省略时必须配合 `--in-place`（临时目录后回写输入目录）或 `--dry-run`（临时目录、丢弃不写），否则报错 |
| `-t, --inhabited-time-seconds` | `InhabitedTime` 阈值（秒，1s=20 ticks），**严格大于**该值才保留，默认 `300` |
| `--remove-unknown` | 删除外部压缩、空数据或缺失 InhabitedTime 标签的区块，默认关闭（安全保留）；真正未知的压缩格式仍安全保留 |
| `--progress-mode` | 进度显示：`off` / `global` / `region`，默认 `region` |
| `--in-place` | 原地处理：忽略 `OUTPUT_DIR`，直接替换原世界目录 |
| `--zip-output` | 非原地模式下把输出压缩为 `YYYYMMddHHmmss.zip` 并删除输出目录 |
| `-f, --force` | 输出目录已存在且非空时强制覆盖，不做交互提示 |
| `--parallelism` | region 处理并行度，`0` = 自动（按 CPU 核数），默认 `0` |
| `--no-fsync` | 跳过 region 落盘 `fsync`（更快，掉电时更易损坏） |
| `--strict` | 存在任何记录的错误时以退出码 `1` 结束，否则 `0` |
| `--dry-run` | 只分析不写入：零字节落盘，统计照常打印 |
| `--copy-misc` / `--no-copy-misc` | 是否复制非 MCA 文件到输出，默认开启；始终跳过 `session.lock` 与 region/entities/poi 顶层 `.mca` |
| `--keep-range <x1> <z1> <x2> <z2>` | 额外保留矩形 `[x1,z1]×[x2,z2]`（含边界）内区块，与 InhabitedTime 规则取并集 |
| `--progress-interval <n>` | 每处理 `n` 个区块输出一次进度（计数节流），默认 `1000` |
| `--progress-interval-ms <ms>` | 每 `ms` 毫秒输出一次进度（时间节流，`0` 关闭，优先于计数节流） |
| `--report` | 结束后在 stdout 打印纯文本报告 |
| `--report-file <file>` | 把报告写入文件（格式由 `--report-format` 指定，默认 `json`） |
| `--report-format <json\|csv\|text>` | 报告文件格式 |

#### 示例

```bash
# 基本使用：输出到新目录
rust-thanos backup /path/to/world /path/to/output

# 自定义阈值（10 分钟）
rust-thanos backup /path/to/world /path/to/output --inhabited-time-seconds 600

# 原地替换
rust-thanos backup /path/to/world --in-place

# 全局进度 + 报告 + strict 退出码
rust-thanos backup /path/to/world /path/to/output --progress-mode global \
  --report --report-file report.json --report-format json --strict

# 只分析不写入（演练）
rust-thanos backup /path/to/world --dry-run

# 压缩输出并删除输出目录
rust-thanos backup /path/to/world /path/to/output --zip-output

# 强制覆盖 + 高并行
rust-thanos backup /path/to/world /path/to/output -f --parallelism 8
```

### merge — 合并恢复

把优化后的世界合并回完整备份。优化过程会把区块裁剪到「在用」子集，merge 负责把这份裁剪后的世界与完整备份按**区块槽级**合并：

- 每个区块槽：**patch 有该槽 → 用 patch（它是最新状态）**；否则 **base 有该槽 → 用 base**；否则空槽。
- 每槽的区块决定同样驱动 `entities/` 与 `poi/` 的对应槽（锁步）。
- base 独有的 region 原样保留；patch 独有的 region 连同样的兄弟 entities/poi 一起拷入。
- 无法打开的 patch region（缺失 / 过小 / 损坏头）→ 整个 region 连 entities/poi **逐字节回退 base**；已打开但读取中途出错则记录错误（`ErrorKind::Mca`）。
- 输出 = base 的全量 + patch 中仍存活的最新区块，即「优化备份 → 恢复完整世界」。

#### 基本语法

```bash
rust-thanos merge BASE_DIR PATCH_DIR OUTPUT_DIR \
  [-f|--force] [--progress-mode <mode>] [--parallelism <n>] \
  [--progress-interval <n>] [--no-fsync] \
  [--report] [--report-file <file>] [--report-format <json|csv|text>]
```

#### 参数说明

| 参数 | 描述 |
|------|------|
| `BASE_DIR` | 完整备份世界（优化前的原世界） |
| `PATCH_DIR` | 优化后的世界（`rust-thanos backup` 的输出） |
| `OUTPUT_DIR` | 合并结果写入目录；BASE/PATCH/OUTPUT 三者必须互不重叠 |
| `-f, --force` | 输出目录已存在且非空时强制覆盖 |
| `--progress-mode` | 进度显示：`off` / `global` / `region`，默认 `off` |
| `--parallelism` | region 并行度，默认 `0`；`0`/`1` 串行，`> 1` 才并行（与 backup 不同，不自动按核数）；任意值输出逐字节一致 |
| `--no-fsync` | 跳过 region 落盘 `fsync` |
| `--progress-interval <n>` | 每处理 `n` 个文件输出一次进度（merge 按文件节流），默认 `1000` |
| `--report` / `--report-file` / `--report-format` | 同 backup：报告显示 / 写文件 / 格式 |

报告统计：`mergedRegions`（合并的 region 数）、`patchSlots`（来自 patch 的槽数）、`baseSlots`（来自 base 的槽数）、`linkedEntities` / `linkedPoi`（锁步的 entities/poi 槽数）、`copiedFiles`（patch 独有复制文件数）、`overlayFiles`（覆盖的非 MCA 文件数）、`errors`。有错误时退出码为 `1`。（JSON 报告键为 camelCase，如 `mergedRegions`；Rust 库字段为 snake_case，如 `report.merged_regions`。）

#### 示例

```bash
# 基本合并
rust-thanos merge /path/to/original /path/to/optimized /path/to/restored

# 高并行 + 报告
rust-thanos merge /path/to/original /path/to/optimized /path/to/restored \
  --parallelism 8 --report --report-file merge.json --report-format json
```

### 端到端工作流：优化备份 → 合并恢复

```bash
# 1. 优化世界：把完整世界裁剪为只剩在用区块的备份
rust-thanos backup /path/to/world /path/to/backup -t 600 --report

# 2. 合并恢复：把优化后的备份合回完整世界，得到最新且完整的世界
rust-thanos merge /path/to/world /path/to/backup /path/to/restored --report
```

`/path/to/restored` 与优化前的世界等价（全部区块都在），且所有优化后仍存活的区块取最新状态。

> 命令行帮助文本为英文，`rust-thanos --help` 可查看完整说明与示例。

## 📚 作为库使用

rust-thanos 同时是一个 Rust 库，公开 `world` 模块。优化与合并都通过「配置模型 + 引擎」调用，可用 hooks 接入错误/报告/指标。注意：`--strict` 是 CLI 行为（影响退出码），库引擎不读取该字段。

### 优化

```rust
use rust_thanos::world::{
    optimize, ErrorKind, FileReportSink, MetricsSink, OptimizeError, OptimizeReport,
    OptimizerRequestBuilder, ProgressMode, ReportFormat, ReportSink,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

// 一个 MetricsSink：收集聚合计数
struct Counter(AtomicU64, AtomicU64);
impl MetricsSink for Counter {
    fn record_error(&self, _e: &OptimizeError) {}
    fn inc_processed(&self, n: u64) { self.0.fetch_add(n, Ordering::Relaxed); }
    fn inc_removed(&self, n: u64) { self.1.fetch_add(n, Ordering::Relaxed); }
}

let request = OptimizerRequestBuilder::new("/path/to/world".into())
    .output(Some("/path/to/out".into()))
    .inhabited_threshold_seconds(600)          // 秒级阈值，内部 ×20 转 tick
    .progress_mode(ProgressMode::Region)
    // hooks：错误回调 / 报告写出 / 指标
    .on_error(Arc::new(|e: &OptimizeError| {
        eprintln!("[{}] {}", e.kind, e.path);
    }))
    .report_sink(Arc::new(FileReportSink::new(
        "/path/to/report.json".into(),
        ReportFormat::Json,
    )))
    .metrics_sink(Arc::new(Counter(AtomicU64::new(0), AtomicU64::new(0))))
    .build();

let report: OptimizeReport = optimize(&request)?;
println!("kept {} removed {}", report.kept_chunks, report.removed_chunks);
for e in &report.errors {
    match e.kind { ErrorKind::Mca => { /* … */ } _ => {} }
}
```

### 内存世界（测试 / 无盘场景）

`MemoryFS` + `MemoryMcaIOFactory` 让同一套管线跑在内存树里，无需真实磁盘：

```rust
use rust_thanos::world::{optimize, FileSystem, MemoryFS, MemoryMcaIOFactory, OptimizerRequestBuilder};
use std::sync::Arc;

let mem = Arc::new(MemoryFS::new());
mem.write(&"/world/region/r.0.0.mca".into(), region_bytes)?;
let request = OptimizerRequestBuilder::new("/world".into())
    .output(Some("/world-out".into()))
    .fs(mem.clone())
    .io_factory(Arc::new(MemoryMcaIOFactory))
    .build();
let report = optimize(&request)?;
```

### 合并

```rust
use rust_thanos::world::{merge, MergeReport, MergeRequestBuilder};

let request = MergeRequestBuilder::new(
    "/path/to/original".into(),   // base
    "/path/to/optimized".into(),  // patch
    "/path/to/restored".into(),   // output
)
.parallelism(8)
.build();

let report: MergeReport = merge(&request)?;
println!(
    "patch {} base {} regions {}",
    report.patch_slots, report.base_slots, report.merged_regions
);
```

## 🔧 工作原理

**优化（backup）**
1. **世界扫描**：递归识别所有维度目录（含 region 子目录即维度）。
2. **强加载解析**：探测 `data/minecraft/chunk_tickets.dat`（1.21.6+/26.1+）与 `data/chunks.dat`（旧版），**两个文件只取先存在者**，文件内的 Forced / Tickets 两种结构取并集。
3. **区块分析**：对每个 `.mca` 的区块条目，判定 `InhabitedTime > 阈值` 或位于强加载/keep-range 内；无法解析的区块**安全保留**并记录错误。
4. **region 级并行**：维度串行、region 并行（每 region 写独立输出，结果与并行度无关、逐字节一致）。
5. **锁步三文件**：保留的区块槽同步写入 `region/`、`entities/`、`poi/` 同名文件；整区全删不生成空文件（惰性写入）。
6. **输出可用化**：复制非 MCA 文件（跳过 `session.lock`）；支持原地替换、zip 归档、dry-run。
7. **报告**：统计 + 错误列表，JSON/CSV/Text 输出，`--strict` 控制退出码。

**合并（merge）**
1. **OverlapGuard**：BASE/PATCH/OUTPUT 互不重叠，否则直接报 `Input` 错误。
2. **copyTree**：base 全量复制到输出，跳过 patch 也有的 region/entities/poi `.mca`（稍后 overlay 重写）。
3. **overlayPatch**：region 级并行合并——每槽 `patch 有 → patch；否则 base 有 → base；否则空`，entities/poi 锁步；无法打开的 patch region 逐字节回退 base；patch 独有 region 连兄弟 entities/poi 一并拷入；空 entities/poi 结果删除文件。

## 📊 性能

真实世界数据验收（17.4 GiB，3,162,906 区块，WSL2/12 逻辑核，见 [docs/benchmark.md](docs/benchmark.md)）：

- **吞吐**：22k–33k chunks/s；`-t 300` 在约 1:46 内完成全量处理（含 17.4 GiB 读取与输出写入），缩减率 88.3%。
- **内存**：字节级定位 + 惰性写入，不整体加载 region；实测 RSS 仅 **23–25 MiB**，与 17.4 GiB 输入规模无关。
- **merge 往返**：18 GiB 世界 + 优化备份合并恢复 2:41 完成，内存 16 MiB，region 数据 100% 保留。
- **并行**：region 级并行，自动按 CPU 核数，任意并行度输出确定（重复运行逐字节一致）。

## 🛡️ 安全注意事项

1. **先备份**：处理重要世界前建议先做完整备份。
2. **`--in-place` 风险**：确保有足够磁盘空间（临时目录 + 回写）。
3. **`--remove-unknown` 慎用**：会删除外部压缩、空数据或缺失 InhabitedTime 标签的区块（真正未知的压缩格式仍安全保留）。
4. **merge 三方目录**：BASE/PATCH/OUTPUT 必须是三个互不重叠的目录，程序会校验并在违规时拒绝执行。

## 🧪 测试

内置真实 Anvil 格式 fixture（`tests/Fixtures/`），覆盖优化、merge、内存文件系统、hooks 等：

```bash
cargo test
```

## 🖥️ 平台支持

提供独立二进制，覆盖 macOS / Linux / Windows，且同时支持 x86_64 与 ARM64：

- **macOS**：`.tar.gz`（arm64 / x86_64）
- **Linux**：`.tar.gz`（arm64 / x86_64）
- **Windows**：`.zip`（arm64 / x86_64），原生 MSVC 构建，无需额外运行库

> 提示：Windows 下若需要最佳并发性能，仍可使用 WSL 运行 Linux 版本。

Homebrew 公式支持 macOS 与 Linux；发布流程自动为 macOS（arm64/x64）、Linux（x64/arm64）与 Windows（arm64/x64）生成压缩包与校验。

## 📄 许可证

MIT — 查看 [LICENSE](LICENSE)。

## 📞 相关链接

- 项目：[https://github.com/OrzMC/rust-thanos](https://github.com/OrzMC/rust-thanos)
- 报告问题：[Issues](https://github.com/OrzMC/rust-thanos/issues)
- [Minecraft 区域文件格式](https://minecraft.fandom.com/wiki/Region_file_format) · [NBT 格式](https://minecraft.fandom.com/wiki/NBT_format) · [Rust](https://www.rust-lang.org/)
