# AGENTS.md

This file provides guidance to AI coding agents (Claude Code, OpenAI Codex, Gemini CLI, Cursor, GitHub Copilot, and other agents that read `AGENTS.md`) working in this repository. It is the single source of truth for project guidance; vendor-specific files (e.g. `CLAUDE.md`) delegate here so content does not drift across agents.

## Commands

```bash
cargo build --release                                      # build release binary
cargo test                                                 # all tests
cargo test <name>                                          # single test, e.g. cargo test memory_fs
cargo test <name> -- --nocapture                           # single test with stdout
cargo clippy --all-targets --all-features -- -D warnings   # lint (CI gate)
cargo fmt --all --check                                    # format check (CI gate)
cargo check --locked                                       # verify Cargo.lock matches Cargo.toml
```

CI (`.github/workflows/ci.yml`) runs fmt → clippy → build → test on ubuntu/macos/windows. Tests run from the crate root, so `tests/Fixtures/...` relative paths resolve directly.

## Architecture

`rust-thanos` is a Minecraft Java world optimizer that mirrors OrzMCBackup: `backup` drops unused chunks (output is a usable world), `merge` merges a trimmed world back into a full backup at chunk-slot granularity. It ships as both a library and a thin CLI (`src/main.rs`, clap, subcommands `backup`/`merge`) that builds an `OptimizerRequest` / `MergeRequest` and runs the engine.

Three layers:

1. **`mca/` — Anvil region format.** `access.rs` abstracts random access (disk file or in-memory buffer); `entry.rs` models one chunk slot (header, compression RAW/ZLIB/GZIP/LZ4, external-chunk handling, lz4 checksum); `reader.rs` / `writer.rs` are the region reader and lazy writer (`sync_on_finalize`).

2. **`patterns/` — keep/remove decision.** The `ChunkPattern` trait decides per chunk over `&mut McaEntry`: `inhabited.rs` (InhabitedTime strictly `>` threshold), `list.rs` (force-loaded chunk coordinates), `range.rs` (keep rectangle). The engine unions them (OR) — a chunk is kept if any pattern matches.

3. **`world/` — the engine** (library entry point):
   - `fs.rs`: the `FileSystem` trait — the engine never touches `std::fs` directly. `RealFileSystem` (production) / `MemoryFS` (in-memory tree for tests; paths use `/`).
   - `io_factory.rs`: `McaIOFactory` produces reader/writer pairs. `DefaultMcaIOFactory` → disk; `MemoryMcaIOFactory` → `MemoryFS` (writers accumulate and flush at finalize).
   - `optimizer.rs`: `OptimizerRequest` (flat config via `OptimizerRequestBuilder`), `OptimizerEngine` trait, stateless `DefaultOptimizer`, `optimize()` free function. Pipeline: `discover_dimensions` → `count_total_chunks` → `process_dimension` (regions parallel via rayon) → `handle_in_place`. All non-fatal errors route through a shared `Shared` into `OptimizeReport.errors`.
   - `merger.rs`: `MergeRequest`/`MergeRequestBuilder`, `MergeEngine`/`DefaultMerger`, `merge()`. Overlap guard → copyTree (base full copy, skipping regions patch also has) → overlayPatch (per-slot patch-wins, entities/poi in lockstep). Note: unlike backup, merge does **not** auto-resolve `parallelism = 0` — regions run in parallel only when `parallelism > 1` (else serial).
   - Supporting: `forceload.rs` (probes `data/minecraft/chunk_tickets.dat` for 1.21.6+/26.1+, then `data/chunks.dat` — **first existing file wins, never combined**; both NBT structures inside the chosen file are unioned), `misc.rs` (copy non-MCA files), `output.rs` (overlap guard), `zip.rs` (timestamped archive), `progress.rs` (`ProgressSink` event model), `report.rs` (`OptimizeReport`, `ReportSink`, `MetricsSink`, `ErrorHook`).

Key invariants to preserve when editing the engine:
- **All I/O through traits.** Any new I/O must go through `FileSystem` + `McaIOFactory`, never `std::fs`/`File` directly — that is what keeps the pipeline testable in-memory.
- **Determinism under parallelism.** Region-level parallelism must produce byte-identical output at any `--parallelism`; each region finalizes independently. Tests pin this (merge parallelism tests in `tests/phase4.rs`).
- **Safety-first errors.** Unparseable/corrupt chunks are KEPT — `--remove-unknown` only drops external-compressed / empty-data / missing-InhabitedTime-tag chunks, never truly-unknown compression (its uncompress call errors, which the pattern propagates and the engine safe-keeps). Per-chunk failures are recorded (non-fatal) and only `--strict` changes the exit code.
- **No fd amplification.** `McaReader::entries()` clones its `RandomAccess` per chunk slot; `FileAccess::try_clone` must share one `Arc<Mutex<File>>` (never `File::try_clone`, which opens a new fd per entry — that exhausted the ulimit on a 21k-region world and silently dropped regions). `tests/reader_fd.rs` pins the fd count on Linux.
- **Skipped files reach the output.** A region (or its entities/poi counterpart) that cannot be parsed or opened is byte-preserved as-is in non-in-place output (`Shared::skip_region_preserving` / `preserve_unprocessable`), and marked skipped so in-place mode keeps the original — never silently dropped. `tests/memory_fs.rs::memory_fs_preserves_unparseable_region` and `tests/tiny_mca.rs` pin this.
- **Seconds vs ticks.** CLI threshold is seconds; `OptimizerRequestBuilder::inhabited_threshold_seconds` converts ×20 to ticks. `world::run` (legacy, used by some tests) takes ticks directly and pins the strict-`>` boundary.

## Testing

- Real Anvil fixtures under `tests/Fixtures/`: `world` (676-chunk region, InhabitedTime spread 0..=40 → keeps none at threshold ≥6000), `world-26-1` (new 26.1 format incl. `chunk_tickets.dat` and `dimensions/`), `legacy-forceload`, `merge/base` + `merge/patch`.
- Two invocation styles: `rust_thanos::world::run(...)` (tick-based convenience) or `OptimizerRequestBuilder` + `DefaultOptimizer.run()` / `optimize()`. `tests/memory_fs.rs` shows the pure in-memory path (`MemoryFS` + `MemoryMcaIOFactory`) with no disk I/O.
- Tests that mutate a copy use `fs_extra::dir::copy` into a temp dir.

## Release (version discipline)

Version scheme is `0.0.x`, and the **release tag must equal `Cargo.toml`'s `version`** — the `validate-version` job in `.github/workflows/release.yml` fails otherwise. To release:
1. Bump `Cargo.toml` `version` (e.g. `cargo set-version 0.0.4`), let Cargo sync `Cargo.lock`, then `cargo check --locked`.
2. Commit, tag exactly that version, push the tag.
3. The workflow builds 6 targets (macOS/Linux `.tar.gz`, Windows `.zip`, each arm64 + x86_64), publishes to GitHub Releases, and updates the homebrew formula (`orzgeeker/homebrew-core`).

Cross-compilation linker settings live in `.cargo/config.toml` (linux-gnu relro/znow flags; `aarch64-linux-gnu-gcc` for `aarch64-unknown-linux-gnu`).
