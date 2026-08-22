# rust-thanos 真实世界数据验收脚本

本目录是 [docs/benchmark.md](../docs/benchmark.md) 所述验收的**可复现执行脚本**。它们在一个 WSL 2 / Linux 环境、数据位于 WSL 原生 ext4 的假定布局下运行（避免 9p 互操作层对 I/O 的干扰）。

## 假定布局（请按需调整脚本顶部变量）

```
/bench/
├── world/                 # 真实世界全量数据（21,688 个 MCA，≈17.4 GiB）
├── world_backup/          # 同世界优化后的备份（真值基准，3,415 个 MCA）
├── rust-thanos/           # 本项目源码 + 已构建的 release 二进制
├── out/                   # 各次 backup/merge 的输出
└── reports/               # 各次运行的 report JSON / stdout / time
```

| 脚本 | 作用 |
| --- | --- |
| `setup.sh` | 安装 build-essential/curl/time 与 Rust minimal 工具链 |
| `build.sh` | `cargo build --release --locked` 并打印二进制信息 |
| `env.sh` | 快照运行环境（CPU/内存/rustc/数据体积）到 `reports/env.txt` |
| `run_one.sh <秒> <tag>` | 单次 `backup`，`/usr/bin/time -v` 计时，写 report JSON |
| `run_dry.sh <秒> <tag>` | 单次 `--dry-run`（纯分析、零写入） |
| `sweep.sh` | 依序跑 `-t {0,60,120,180,240,300}` 六档阈值并汇总 |
| `watch_sweep.sh` | 监听 sweep 进度，每完成一个阈值输出一行 |
| `summarize.py <glob>` | 聚合 report JSON 为对比表（proc/removed/kept/体积/耗时/吞吐/错误数） |
| `smoke.sh` | 在 `world_backup` 上跑 `-t 300` 冒烟 + 结构校验 |
| `verify_structure.sh <输出> <标签>` | 校验输出为可用 Minecraft 世界（必需文件/目录、零字节 MCA、维度分布） |
| `compare_outputs.sh <输出> <标签>` | 输出 MCA 文件集 vs `world_backup`（真值）与 `world`（子集）对比 |
| `merge.sh` | `merge world world_backup` 往返恢复 + 结构校验 |

## 复现步骤

```bash
# 1) 一次性：工具链 + 构建（WSL 内）
sudo bash setup.sh
bash build.sh

# 2) 准备数据（将真实世界拷入 ext4，保留字节校验）
#    world/、world_backup/ 放至 /bench/ 下，并把本仓库放至 /bench/rust-thanos

# 3) 环境快照 + 冒烟
bash env.sh
bash smoke.sh

# 4) 六档阈值扫描（t0 … t300，默认 ulimit，约 12 分钟）
bash sweep.sh

# 5) 正确性验证（任一档输出，例如 t60）
bash verify_structure.sh /bench/out/t60 t60
bash compare_outputs.sh   /bench/out/t60 t60
bash merge.sh

# 6) 汇总
python3 summarize.py /bench/reports/t*.json
```

> 说明：运行环境为 WSL 2（Ubuntu 26.04），15 GiB 内存 < 世界体积，且 WSL 不允许主动 `drop_caches`，故每次运行均包含真实磁盘读取；结果受页面缓存状态影响，属于自然（warm）缓存下的上界。
