# rust-thanos

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://github.com/OrzMC/rust-thanos/actions/workflows/ci.yml/badge.svg)](https://github.com/OrzMC/rust-thanos/actions/workflows/ci.yml)

一个高效的 Minecraft Java 世界优化工具，用于检测并删除未使用的区块，减少世界文件大小，提高游戏性能。

## 🌟 特性

- **智能区块检测**：通过 `InhabitedTime` 和强加载区块列表判定区块是否被使用
- **多种压缩格式支持**：兼容 RAW、ZLIB、GZIP、LZ4 Block（含校验）
- **高效处理**：快速字节定位，避免完整 NBT 解析，提高处理速度
- **并行处理**：利用多核 CPU 加速处理过程
- **灵活配置**：支持自定义 `InhabitedTime` 阈值和多种进度显示模式
- **安全可靠**：详细的错误处理和容错机制，确保数据安全
- **多种使用模式**：支持输出到新目录或原地替换
- **递归维度识别**：自动扫描输入目录下所有含 region 的维度目录（Vanilla 的 DIM-1/DIM1，Paper 的 world_nether/world_the_end 以及自定义维度）
- **便捷归档**：非原地模式下支持将输出目录压缩为 zip，并按 `YYYYMMddHHmmss.zip` 命名，压缩完成后自动删除输出目录
- **友好进度显示**：进度条自适应终端宽度，最多显示 50 个 bar

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

1. 克隆仓库并进入目录：
   ```bash
   git clone https://github.com/OrzMC/rust-thanos.git
   cd rust-thanos
   ```

2. 构建项目：
   ```bash
   cargo build --release
   ```

3. 运行测试（可选）：
   ```bash
   cargo test
   ```

4. 安装到系统（可选）：
   ```bash
   cargo install --path .
   ```

### 下载预编译二进制文件

预编译的二进制文件可在 [Releases](https://github.com/OrzMC/rust-thanos/releases) 页面下载。

## 🚀 使用

### 基本语法

```bash
rust-thanos WORLD_DIR [OUTPUT_DIR] \
  [-t|--inhabited-time-seconds <seconds>] [--remove-unknown] \
  [--progress-mode <mode>] [--in-place] [--zip-output] [-f|--force]
```

### 参数说明

| 参数 | 描述 |
|------|------|
| `WORLD_DIR` | Minecraft 世界根目录（必填） |
| `OUTPUT_DIR` | 输出目录，必须为空；当目录已存在且非空将提示是否覆盖；使用 `-f` 可跳过提示；若未提供将使用临时目录并最终替换输入目录 |
| `-t, --inhabited-time-seconds` | 以秒为单位设置 `InhabitedTime` 阈值（1s=20 ticks），默认 `300` |
| `--remove-unknown` | 对未知或外部压缩区块视为可删除，默认关闭 |
| `--progress-mode` | 进度显示模式：`off`（关闭）、`global`（全局进度）、`region`（区域进度），默认 `region` |
| `--in-place` | 原地处理：忽略 `OUTPUT_DIR`，直接替换原世界目录 |
| `--zip-output` | 非原地模式下将 `OUTPUT_DIR` 压缩为 zip（`YYYYMMddHHmmss.zip`），并在压缩完成后删除 `OUTPUT_DIR` |
| `-f, --force` | 当 `OUTPUT_DIR` 已存在且非空时强制覆盖，不进行交互提示；默认会提示选择是否覆盖 |

### 实用示例

#### 1. 基本使用 - 输出到新目录
```bash
rust-thanos /path/to/world /path/to/output
```

#### 2. 自定义 `InhabitedTime` 阈值（10 分钟）
```bash
rust-thanos /path/to/world /path/to/output --inhabited-time-seconds 600
```

#### 3. 原地替换世界文件
```bash
rust-thanos /path/to/world --in-place
```

#### 4. 移除未知压缩格式的区块
```bash
rust-thanos /path/to/world /path/to/output --remove-unknown
```

#### 5. 使用全局进度显示
```bash
rust-thanos /path/to/world /path/to/output --progress-mode global
```

#### 6. 压缩输出目录（并删除输出目录）
```bash
rust-thanos /path/to/world /path/to/output --zip-output
# 输出：
# 开始压缩: /path/to/output → /path/to/20250101123045.zip
# zip: /path/to/20250101123045.zip
```

#### 6. 使用项目内置测试世界
```bash
cargo run -- tests/Fixtures/world ./out
```

#### 7. 强制覆盖输出目录
```bash
rust-thanos /path/to/world /path/to/output -f
```

> 提示：命令行帮助文本为英文，可通过 `rust-thanos --help` 查看完整说明与示例。

## 🔧 工作原理

1. **世界扫描**：识别所有维度目录和 `.mca` 文件（递归扫描输入目录下所有含 `region` 的维度目录）
2. **强加载区块解析**：从 `data/chunks.dat` 读取强加载区块列表
3. **区块分析**：
   - 读取每个 `.mca` 文件的区块条目
   - 检查区块的 `InhabitedTime` 是否超过阈值
   - 检查区块是否在强加载列表中
4. **区块过滤**：只保留符合条件的区块
5. **输出结果**：写入保留的区块到新文件或原地替换
6. **统计报告**：显示处理结果和性能统计

## 📊 性能

- **处理速度**：取决于 CPU 核心数和世界大小，通常每秒可处理数千个区块
- **内存使用**：高效的内存管理，适合处理大型世界
- **磁盘 I/O**：优化的读写操作，减少磁盘访问

## 🛡️ 安全注意事项

1. **备份建议**：在处理重要世界之前，建议创建备份
2. **原地处理风险**：使用 `--in-place` 选项时，请确保有足够的磁盘空间
3. **未知压缩处理**：谨慎使用 `--remove-unknown` 选项，可能导致数据丢失

## 📝 技术细节

### 支持的文件类型

- 主世界文件：`region/r.*.*.mca`
- 实体文件：`entities/r.*.*.mca`
- 兴趣点文件：`poi/r.*.*.mca`

### 区块保留规则

区块会被保留如果：
1. `InhabitedTime` 大于或等于设定的阈值
2. 区块在强加载列表中
3. 区块使用未知压缩格式且未启用 `--remove-unknown`

## 🤝 贡献

欢迎提交 Issue 和 Pull Request！

### 开发流程

1. Fork 仓库
2. 创建功能分支：`git checkout -b feature/AmazingFeature`
3. 提交更改：`git commit -m 'Add some AmazingFeature'`
4. 推送到分支：`git push origin feature/AmazingFeature`
5. 开启 Pull Request

## 🧪 测试

项目包含全面的测试套件，使用内置的测试世界进行测试：

```bash
cargo test
```

## 🖥️ 平台支持

- **推荐平台**：Linux/macOS（同时支持 x86_64 与 ARM64）
- **Windows**：建议使用 WSL 获取更好的并发性能

> 通过 Homebrew Tap 安装的公式同时支持 macOS 与 Linux；发布流程会为 macOS（arm64/x64）与 Linux（x64/arm64）生成对应的压缩包与校验。

## 📄 许可证

本项目采用 MIT 许可证 - 查看 [LICENSE](LICENSE) 文件了解详情

## 📞 联系方式

- 项目链接：[https://github.com/OrzMC/rust-thanos](https://github.com/OrzMC/rust-thanos)
- 报告问题：[Issues](https://github.com/OrzMC/rust-thanos/issues)

## 📚 相关链接

- [Minecraft 维基百科 - 区域文件](https://minecraft.fandom.com/wiki/Region_file_format)
- [NBT 格式规范](https://minecraft.fandom.com/wiki/NBT_format)
- [Rust 编程语言](https://www.rust-lang.org/)

## 📈 统计

使用 rust-thanos 后，您可以期待：
- 减少 50-90% 的世界文件大小（取决于世界使用情况）
- 提高游戏加载速度
- 减少服务器内存使用

---

**rust-thanos** - 让您的 Minecraft 世界更加高效！ 🚀
