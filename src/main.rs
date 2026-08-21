use anyhow::Result;
use clap::{Parser, Subcommand};
use rust_thanos::world::{
    merge, optimize, MergeReportIO, MergeRequestBuilder, OptimizerRequestBuilder, ProgressMode,
    ProgressSink, RangePattern, ReportFormat, ReportIO, TermProgressSink,
};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser)]
#[command(
    name = "rust-thanos",
    version,
    about = "Optimize Minecraft Java worlds",
    long_about = "Scan MCA region files and remove unused chunks. Keep chunks by InhabitedTime threshold and force-loaded tickets. Supports RAW/ZLIB/GZIP/LZ4. Recursively detects dimension directories. Friendly progress display. Optional zip archive of OUTPUT_DIR.",
    after_help = "Examples:\n  rust-thanos backup /world /out\n  rust-thanos backup /world /out -t 600\n  rust-thanos backup /world /out --remove-unknown --progress-mode global\n  rust-thanos backup /world --in-place\n  rust-thanos backup /world /out --zip-output\n  rust-thanos backup /world /out -f\n  rust-thanos merge BASE PATCH OUTPUT\n\nNotes:\n  • InhabitedTime threshold unit: seconds (1s = 20 ticks).\n  • Non in-place mode requires OUTPUT_DIR to be empty; if it exists and is not empty, you will be prompted. Use -f/--force to overwrite without prompt.\n  • If OUTPUT_DIR is omitted, a temporary directory is used and replaces the input directory.\n  • On Windows, WSL is recommended for better performance."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Optimize a world by removing unused chunks (default workflow).
    Backup(BackupArgs),
    /// Merge a backup patch back into a world (restore). Phase 4.
    Merge(MergeArgs),
}

#[derive(clap::Args)]
struct BackupArgs {
    #[arg(value_name = "WORLD_DIR", help = "Minecraft world root")]
    input: PathBuf,
    #[arg(
        value_name = "OUTPUT_DIR",
        help = "Output directory (must be empty; optional)"
    )]
    output: Option<PathBuf>,
    #[arg(
        long = "inhabited-time-seconds",
        short = 't',
        default_value_t = 300,
        help = "InhabitedTime threshold in seconds (1s = 20 ticks)"
    )]
    inhabited_time_seconds: i64,
    #[arg(
        long,
        default_value_t = false,
        help = "Treat unknown/external-compressed chunks as removable"
    )]
    remove_unknown: bool,
    #[arg(long, value_enum, default_value_t = ProgressMode::Region, help = "Progress display: off | global | region")]
    progress_mode: ProgressMode,
    #[arg(
        long,
        default_value_t = false,
        help = "Process in-place: ignore OUTPUT_DIR and replace WORLD_DIR"
    )]
    in_place: bool,
    #[arg(
        long,
        default_value_t = false,
        help = "Zip OUTPUT_DIR to timestamped archive (YYYYMMddHHmmss.zip) and remove it"
    )]
    zip_output: bool,
    #[arg(
        short = 'f',
        long,
        default_value_t = false,
        help = "Force overwrite OUTPUT_DIR if it exists (no prompt)"
    )]
    force: bool,
    #[arg(long, default_value_t = 0, help = "Region parallelism (0 = auto)")]
    parallelism: usize,
    #[arg(
        long,
        default_value_t = false,
        help = "Skip fsync on region finalize (faster, less durable)"
    )]
    no_fsync: bool,
    #[arg(
        long,
        default_value_t = false,
        help = "Exit non-zero when any error was recorded (Phase 3)"
    )]
    strict: bool,
    #[arg(
        long,
        default_value_t = false,
        help = "Dry-run: analyze without writing any output (zero bytes)"
    )]
    dry_run: bool,
    #[arg(
        long,
        default_value_t = true,
        help = "Copy non-MCA files (level.dat, players/, data/, ...) to the output"
    )]
    copy_misc: bool,
    #[arg(long, help = "Disable copying non-MCA files to the output")]
    no_copy_misc: bool,
    #[arg(
        long,
        default_value_t = false,
        help = "Print a plain-text report to stdout after the run"
    )]
    report: bool,
    #[arg(
        long,
        value_name = "FILE",
        help = "Write the report to a file (JSON/CSV)"
    )]
    report_file: Option<PathBuf>,
    #[arg(
        long,
        value_enum,
        default_value_t = ReportFormat::Json,
        help = "Report file format: json | csv | text"
    )]
    report_format: ReportFormat,
    #[arg(
        long,
        default_value_t = 1000,
        help = "Emit a progress update every N chunks"
    )]
    progress_interval: u64,
    #[arg(
        long,
        default_value_t = 0,
        help = "Emit a progress update every N milliseconds (0 = count-based)"
    )]
    progress_interval_ms: u64,
    #[arg(
        long,
        num_args = 4,
        value_names = ["X1", "Z1", "X2", "Z2"],
        help = "Keep chunks inside the rectangle [X1,Z1] x [X2,Z2] (inclusive)"
    )]
    keep_range: Option<Vec<i32>>,
}

#[derive(clap::Args)]
struct MergeArgs {
    /// Original (pre-optimization) world.
    base: PathBuf,
    /// Backup/optimized world to merge back in.
    patch: PathBuf,
    /// Destination world directory.
    output: PathBuf,
    #[arg(short = 'f', long, help = "Overwrite a non-empty OUTPUT")]
    force: bool,
    #[arg(long, value_enum, default_value_t = ProgressMode::Off, help = "Progress display: off | global | region")]
    progress_mode: ProgressMode,
    #[arg(long, default_value_t = 0, help = "Region parallelism (0 = auto)")]
    parallelism: usize,
    #[arg(
        long,
        default_value_t = false,
        help = "Skip fsync on region finalize (faster, less durable)"
    )]
    no_fsync: bool,
    #[arg(
        long,
        default_value_t = 1000,
        help = "Emit a progress update every N files"
    )]
    progress_interval: u64,
    #[arg(
        long,
        default_value_t = false,
        help = "Print a plain-text report to stdout after the run"
    )]
    report: bool,
    #[arg(
        long,
        value_name = "FILE",
        help = "Write the report to a file (JSON/CSV)"
    )]
    report_file: Option<PathBuf>,
    #[arg(
        long,
        value_enum,
        default_value_t = ReportFormat::Json,
        help = "Report file format: json | csv | text"
    )]
    report_format: ReportFormat,
}

fn main() -> Result<()> {
    env_logger::init();
    let cli = Cli::parse();
    match cli.command {
        Commands::Backup(args) => cmd_backup(args),
        Commands::Merge(args) => cmd_merge(args),
    }
}

fn cmd_backup(args: BackupArgs) -> Result<()> {
    if !args.in_place {
        if let Some(ref out_dir) = args.output {
            if out_dir.exists() {
                let non_empty = out_dir.read_dir()?.next().is_some();
                if non_empty {
                    if args.force {
                        std::fs::remove_dir_all(out_dir)?;
                    } else {
                        print!("输出目录已存在，是否覆盖？[y/N]: ");
                        std::io::stdout().flush().ok();
                        let mut line = String::new();
                        std::io::stdin().read_line(&mut line)?;
                        let resp = line.trim().to_lowercase();
                        if resp == "y" || resp == "yes" {
                            std::fs::remove_dir_all(out_dir)?;
                        } else {
                            return Ok(());
                        }
                    }
                }
            }
        }
    }
    let dest = if args.in_place {
        None
    } else {
        args.output.clone()
    };
    let sink: Arc<dyn ProgressSink> = Arc::new(TermProgressSink::new(args.progress_mode));
    let mut builder = OptimizerRequestBuilder::new(args.input)
        .output(dest)
        .inhabited_threshold_seconds(args.inhabited_time_seconds)
        .remove_unknown(args.remove_unknown)
        .progress_mode(args.progress_mode)
        .in_place(args.in_place)
        .force(args.force)
        .parallelism(args.parallelism)
        .sync_on_finalize(!args.no_fsync)
        .strict(args.strict)
        .dry_run(args.dry_run)
        .copy_misc(args.copy_misc && !args.no_copy_misc)
        .zip_output(args.zip_output)
        .progress_sink(sink)
        .progress_interval(args.progress_interval)
        .progress_interval_ms(args.progress_interval_ms);
    if let Some(v) = &args.keep_range {
        builder = builder.pattern(Box::new(RangePattern::new(v[0], v[1], v[2], v[3])));
    }
    let request = builder.build();
    let report = optimize(&request)?;

    if args.report {
        println!("\n{}", ReportIO::to_text(&report));
    }
    if let Some(path) = &args.report_file {
        if let Err(e) = ReportIO::write(&report, path, args.report_format) {
            eprintln!("failed to write report: {}", e);
        }
    }

    if args.strict && !report.errors.is_empty() {
        eprintln!("{} 个错误，strict 模式退出码 1", report.errors.len());
        std::process::exit(1);
    }

    Ok(())
}

fn cmd_merge(args: MergeArgs) -> Result<()> {
    let sink: Arc<dyn ProgressSink> = Arc::new(TermProgressSink::new(args.progress_mode));
    let request = MergeRequestBuilder::new(args.base, args.patch, args.output)
        .force(args.force)
        .progress_mode(args.progress_mode)
        .progress_sink(sink)
        .progress_interval(args.progress_interval)
        .parallelism(args.parallelism)
        .sync_on_finalize(!args.no_fsync)
        .build();
    let report = merge(&request)?;

    if args.report {
        println!("\n{}", MergeReportIO::to_text(&report));
    }
    if let Some(path) = &args.report_file {
        if let Err(e) = MergeReportIO::write(&report, path, args.report_format) {
            eprintln!("failed to write report: {}", e);
        }
    }

    if !report.errors.is_empty() {
        eprintln!("{} 个错误，merge 退出码 1", report.errors.len());
        std::process::exit(1);
    }
    Ok(())
}
