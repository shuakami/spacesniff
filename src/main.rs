mod fmt_util;
mod output;
mod scan;

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};

use crate::scan::{ScanOptions, Scanner};

/// spacesniff — agent-first disk space analyzer.
///
/// Answers "where did my disk space go?" in one fast, compact, machine-readable
/// shot. Designed so an AI agent (or a human) can see the whole picture in a
/// few KB of output, then drill down by re-running on a subdirectory.
#[derive(Parser)]
#[command(name = "spacesniff", version, about, arg_required_else_help = false)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Path to scan (shorthand for `spacesniff scan <PATH>`)
    #[arg(value_name = "PATH")]
    path: Option<PathBuf>,

    #[command(flatten)]
    scan_args: ScanArgs,
}

#[derive(Subcommand)]
enum Command {
    /// Scan a directory tree and show where the space goes (default)
    Scan {
        /// Path to scan
        #[arg(value_name = "PATH", default_value = ".")]
        path: PathBuf,
        #[command(flatten)]
        args: ScanArgs,
    },
    /// List the largest individual files under a path
    Files {
        /// Path to scan
        #[arg(value_name = "PATH", default_value = ".")]
        path: PathBuf,
        /// Number of files to show
        #[arg(short = 'n', long, default_value_t = 25)]
        top: usize,
        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
        /// Number of scanner threads (default: all cores)
        #[arg(long)]
        threads: Option<usize>,
    },
    /// Find every directory with a given name (e.g. all node_modules) and its size
    Find {
        /// Path to scan
        #[arg(value_name = "PATH")]
        path: PathBuf,
        /// Directory names to match exactly (e.g. node_modules target dist)
        #[arg(value_name = "NAMES", required = true)]
        names: Vec<String>,
        /// Number of matches to show
        #[arg(short = 'n', long, default_value_t = 50)]
        top: usize,
        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
        /// Number of scanner threads (default: all cores)
        #[arg(long)]
        threads: Option<usize>,
    },
    /// Print the compact usage protocol for AI agents (start here)
    Agent,
    /// Delete paths and report reclaimed space (dry-run unless --force)
    Delete {
        /// Paths to delete
        #[arg(value_name = "PATHS", required = true)]
        paths: Vec<PathBuf>,
        /// Actually delete (without this flag, only reports what would happen)
        #[arg(long)]
        force: bool,
        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Args, Default)]
struct ScanArgs {
    /// Max tree depth to display (scan is always full-depth)
    #[arg(short, long, default_value_t = 3)]
    depth: usize,

    /// Show at most N entries per directory; the rest fold into one line
    #[arg(short = 'n', long, default_value_t = 10)]
    top: usize,

    /// Hide entries smaller than this (e.g. 10MB, 1.5GB)
    #[arg(long, value_name = "SIZE")]
    min_size: Option<String>,

    /// Emit machine-readable JSON
    #[arg(long)]
    json: bool,

    /// Exclude directories whose name matches exactly (repeatable)
    #[arg(long, value_name = "NAME")]
    exclude: Vec<String>,

    /// Number of scanner threads (default: all cores)
    #[arg(long)]
    threads: Option<usize>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Scan { path, args }) => run_scan(path, args),
        Some(Command::Files {
            path,
            top,
            json,
            threads,
        }) => run_files(path, top, json, threads),
        Some(Command::Find {
            path,
            names,
            top,
            json,
            threads,
        }) => run_find(path, names, top, json, threads),
        Some(Command::Agent) => {
            print!("{}", AGENT_PROTOCOL);
            Ok(())
        }
        Some(Command::Delete { paths, force, json }) => run_delete(paths, force, json),
        None => {
            let path = cli.path.unwrap_or_else(|| PathBuf::from("."));
            run_scan(path, cli.scan_args)
        }
    }
}

const AGENT_PROTOCOL: &str = r#"spacesniff agent protocol v1

Goal: answer "where did the disk space go" and reclaim space safely.

Loop:
1. spacesniff scan <root> --json          # full picture in a few KB
   - tree: children sorted by size desc, max --depth levels, --top per level
   - "other" = folded children; sizes always add up to the parent
2. spacesniff scan <root>/<biggest-subdir> --json   # drill down (fast, re-run freely)
3. spacesniff find <root> node_modules target .venv --json
   # every dir with one of those names + its size, sorted desc — the fast way to
   # answer "how much would deleting all node_modules reclaim". Never re-implement
   # this with your own recursive directory walk; find is a single parallel scan.
4. spacesniff files <root> --json -n 25   # largest individual files
5. Decide what is safe to delete. YOU own this judgment; typical candidates are
   rebuildable artifacts (node_modules, target, .venv, caches, old installers).
6. spacesniff delete <paths...> --json    # DRY-RUN: per-path size + total "reclaimed"
7. Confirm with the user if the data is not trivially rebuildable.
8. spacesniff delete <paths...> --force --json      # execute; best-effort
   # locked/no-access items are skipped, everything else is deleted; receipt has
   # per-path deleted_bytes + failed_items + explained error (e.g. "locked by
   # another process", "access denied; may need administrator rights"), plus
   # freed_disk = real volume free-space gain (trust this over reclaimed:
   # hardlinks/compression make apparent sizes overestimate). Long deletes
   # print progress to stderr every 2s; stdout JSON stays clean.

Rules:
- delete NEVER removes anything without --force.
- Scans never follow symlinks/junctions and never abort on permission errors
  (unreadable dirs are counted in "errors").
- All sizes are bytes (apparent size) — an ESTIMATE. Actual freed disk space can
  be lower (NTFS compression, hardlinks, sparse files); check free space to verify.
- duration_ms tells you how cheap re-scans are.
- Running via npx? npm's cache is NOT safe for concurrent cold starts. Either
  `npm i -g spacesniff` first, or run one npx call to completion before issuing
  parallel commands.
- Useful flags: --depth N, --top N, --min-size 100MB, --exclude NAME, --threads N.
"#;

fn init_threads(threads: Option<usize>) {
    if let Some(n) = threads {
        rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .build_global()
            .ok();
    }
}

fn run_scan(path: PathBuf, args: ScanArgs) -> Result<()> {
    init_threads(args.threads);
    let min_size = args
        .min_size
        .as_deref()
        .map(fmt_util::parse_size)
        .transpose()?
        .unwrap_or(0);
    let root = canonical(&path)?;
    let started = Instant::now();
    let scanner = Scanner::new(ScanOptions {
        exclude: args.exclude.clone(),
        top_files: 0,
        find: Vec::new(),
    });
    let result = scanner.scan(&root);
    let elapsed = started.elapsed();
    if args.json {
        output::print_scan_json(&root, &result, elapsed, args.depth, args.top, min_size)?;
    } else {
        output::print_scan_human(&root, &result, elapsed, args.depth, args.top, min_size);
    }
    Ok(())
}

fn run_files(path: PathBuf, top: usize, json: bool, threads: Option<usize>) -> Result<()> {
    init_threads(threads);
    let root = canonical(&path)?;
    let started = Instant::now();
    let scanner = Scanner::new(ScanOptions {
        exclude: Vec::new(),
        top_files: top,
        find: Vec::new(),
    });
    let result = scanner.scan(&root);
    let elapsed = started.elapsed();
    if json {
        output::print_files_json(&root, &result, elapsed)?;
    } else {
        output::print_files_human(&root, &result, elapsed);
    }
    Ok(())
}

fn run_find(
    path: PathBuf,
    names: Vec<String>,
    top: usize,
    json: bool,
    threads: Option<usize>,
) -> Result<()> {
    init_threads(threads);
    let root = canonical(&path)?;
    let started = Instant::now();
    let scanner = Scanner::new(ScanOptions {
        exclude: Vec::new(),
        top_files: 0,
        find: names,
    });
    let result = scanner.scan(&root);
    let elapsed = started.elapsed();
    if json {
        output::print_find_json(&root, &result, elapsed, top)?;
    } else {
        output::print_find_human(&root, &result, elapsed, top);
    }
    Ok(())
}

fn run_delete(paths: Vec<PathBuf>, force: bool, json: bool) -> Result<()> {
    output::delete_paths(&paths, force, json)
}

fn canonical(path: &Path) -> Result<PathBuf> {
    let root = path
        .canonicalize()
        .with_context(|| format!("cannot access {}", path.display()))?;
    if !root.is_dir() {
        bail!("{} is not a directory", root.display());
    }
    Ok(strip_verbatim(root))
}

/// Windows `canonicalize` returns verbatim paths like `\\?\C:\...`;
/// strip the prefix for readable output. No-op elsewhere.
fn strip_verbatim(path: PathBuf) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        path
    }
}
