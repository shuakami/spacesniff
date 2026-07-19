use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;
use serde::Serialize;

use crate::fmt_util::{bar, human_size};
use crate::scan::{DirNode, ScanResult};

/// A display-pruned tree node: at most `top` children per level, at most
/// `depth` levels, entries below `min_size` folded into `other`.
#[derive(Serialize)]
struct JsonNode {
    name: String,
    size: u64,
    files: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    children: Vec<JsonNode>,
    /// Size of children not shown (folded by top/depth/min_size limits).
    #[serde(skip_serializing_if = "Option::is_none")]
    other: Option<Other>,
}

#[derive(Serialize)]
struct Other {
    dirs: usize,
    size: u64,
}

#[derive(Serialize)]
struct ScanReport<'a> {
    path: &'a str,
    size: u64,
    files: u64,
    dirs: u64,
    errors: u64,
    duration_ms: u128,
    tree: JsonNode,
}

fn prune(node: &DirNode, depth: usize, top: usize, min_size: u64) -> JsonNode {
    let mut children = Vec::new();
    let mut folded_dirs = 0usize;
    let mut folded_size = 0u64;
    if depth > 0 {
        for (i, child) in node.children.iter().enumerate() {
            if i < top && child.size >= min_size {
                children.push(prune(child, depth - 1, top, min_size));
            } else {
                folded_dirs += 1;
                folded_size += child.size;
            }
        }
    } else {
        folded_dirs = node.children.len();
        folded_size = node.children.iter().map(|c| c.size).sum();
    }
    JsonNode {
        name: node.name.clone(),
        size: node.size,
        files: node.files,
        children,
        other: if folded_dirs > 0 {
            Some(Other {
                dirs: folded_dirs,
                size: folded_size,
            })
        } else {
            None
        },
    }
}

pub fn print_scan_json(
    root: &Path,
    result: &ScanResult,
    elapsed: Duration,
    depth: usize,
    top: usize,
    min_size: u64,
) -> Result<()> {
    let report = ScanReport {
        path: &root.display().to_string(),
        size: result.root.size,
        files: result.root.files,
        dirs: result.dirs,
        errors: result.errors,
        duration_ms: elapsed.as_millis(),
        tree: prune(&result.root, depth, top, min_size),
    };
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

pub fn print_scan_human(
    root: &Path,
    result: &ScanResult,
    elapsed: Duration,
    depth: usize,
    top: usize,
    min_size: u64,
) {
    let tree = prune(&result.root, depth, top, min_size);
    println!(
        "{}  {}  ({} files, {} dirs, scanned in {:.2}s{})",
        root.display(),
        human_size(result.root.size),
        result.root.files,
        result.dirs,
        elapsed.as_secs_f64(),
        if result.errors > 0 {
            format!(", {} unreadable", result.errors)
        } else {
            String::new()
        }
    );
    let total = result.root.size.max(1);
    let bars = std::io::stdout().is_terminal();
    print_children(&tree, total, "", bars);
}

fn print_children(node: &JsonNode, total: u64, indent: &str, bars: bool) {
    let count = node.children.len() + node.other.as_ref().map_or(0, |_| 1);
    for (i, child) in node.children.iter().enumerate() {
        let last = i + 1 == count;
        let branch = if last { "└─" } else { "├─" };
        let frac = child.size as f64 / total as f64;
        println!(
            "{indent}{branch}{} {:>9}  {:>4.1}%  {}",
            if bars {
                format!(" {}", bar(frac, 10))
            } else {
                String::new()
            },
            human_size(child.size),
            frac * 100.0,
            child.name
        );
        let child_indent = format!("{indent}{}  ", if last { " " } else { "│" });
        print_children(child, total, &child_indent, bars);
    }
    if let Some(other) = &node.other {
        let frac = other.size as f64 / total as f64;
        println!(
            "{indent}└─{} {:>9}  {:>4.1}%  … {} more dir{}",
            if bars {
                format!(" {}", bar(frac, 10))
            } else {
                String::new()
            },
            human_size(other.size),
            frac * 100.0,
            other.dirs,
            if other.dirs == 1 { "" } else { "s" }
        );
    }
}

#[derive(Serialize)]
struct FilesReport<'a> {
    path: &'a str,
    size: u64,
    files: u64,
    dirs: u64,
    errors: u64,
    duration_ms: u128,
    top_files: &'a [crate::scan::FileEntry],
}

pub fn print_files_json(root: &Path, result: &ScanResult, elapsed: Duration) -> Result<()> {
    let report = FilesReport {
        path: &root.display().to_string(),
        size: result.root.size,
        files: result.root.files,
        dirs: result.dirs,
        errors: result.errors,
        duration_ms: elapsed.as_millis(),
        top_files: &result.top_files,
    };
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

pub fn print_files_human(root: &Path, result: &ScanResult, elapsed: Duration) {
    println!(
        "{}  {}  ({} files, scanned in {:.2}s)",
        root.display(),
        human_size(result.root.size),
        result.root.files,
        elapsed.as_secs_f64()
    );
    let total = result.root.size.max(1);
    for entry in &result.top_files {
        let frac = entry.size as f64 / total as f64;
        println!(
            "{:>9}  {:>4.1}%  {}",
            human_size(entry.size),
            frac * 100.0,
            entry.path.display()
        );
    }
}

#[derive(Serialize)]
struct FindReport<'a> {
    path: &'a str,
    matches: usize,
    /// Total size of all matches (including ones beyond the display limit).
    total_size: u64,
    errors: u64,
    duration_ms: u128,
    found: &'a [crate::scan::FoundDir],
}

pub fn print_find_json(
    root: &Path,
    result: &ScanResult,
    elapsed: Duration,
    top: usize,
) -> Result<()> {
    let shown = &result.found[..result.found.len().min(top)];
    let report = FindReport {
        path: &root.display().to_string(),
        matches: result.found.len(),
        total_size: result.found.iter().map(|d| d.size).sum(),
        errors: result.errors,
        duration_ms: elapsed.as_millis(),
        found: shown,
    };
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

pub fn print_find_human(root: &Path, result: &ScanResult, elapsed: Duration, top: usize) {
    let total: u64 = result.found.iter().map(|d| d.size).sum();
    println!(
        "{}  {} match{}, {} total  (scanned in {:.2}s)",
        root.display(),
        result.found.len(),
        if result.found.len() == 1 { "" } else { "es" },
        human_size(total),
        elapsed.as_secs_f64()
    );
    for dir in result.found.iter().take(top) {
        println!("{:>9}  {}", human_size(dir.size), dir.path.display());
    }
    if result.found.len() > top {
        println!("… {} more (raise -n to show)", result.found.len() - top);
    }
}

#[derive(Serialize)]
struct DeleteEntry {
    path: PathBuf,
    /// Apparent size (estimate; actual filesystem space freed can be lower).
    size: u64,
    deleted: bool,
    /// Bytes actually removed (force mode; equals `size` on full success).
    #[serde(skip_serializing_if = "Option::is_none")]
    deleted_bytes: Option<u64>,
    /// Items skipped because they could not be removed (locked, no access).
    #[serde(skip_serializing_if = "Option::is_none")]
    failed_items: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Serialize)]
struct DeleteReport {
    dry_run: bool,
    reclaimed: u64,
    entries: Vec<DeleteEntry>,
}

/// Best-effort recursive delete: skips entries that cannot be removed
/// (locked or access denied) and keeps deleting everything else.
struct DeleteOutcome {
    deleted_bytes: u64,
    failed_items: u64,
    first_error: Option<String>,
}

fn explain_io_error(e: &std::io::Error) -> String {
    match e.raw_os_error() {
        #[cfg(windows)]
        Some(32) => format!("{e} [file is locked by another process]"),
        #[cfg(windows)]
        Some(5) => format!("{e} [access denied; may need administrator rights]"),
        _ => e.to_string(),
    }
}

fn best_effort_delete(path: &Path, out: &mut DeleteOutcome) {
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) => {
            out.failed_items += 1;
            out.first_error.get_or_insert(explain_io_error(&e));
            return;
        }
    };
    if meta.file_type().is_symlink() {
        // Remove the link itself; never follow it. Windows dir links need remove_dir.
        if fs::remove_file(path).is_err() {
            if let Err(e) = fs::remove_dir(path) {
                out.failed_items += 1;
                out.first_error.get_or_insert(explain_io_error(&e));
            }
        }
    } else if meta.is_dir() {
        match fs::read_dir(path) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    best_effort_delete(&entry.path(), out);
                }
            }
            Err(e) => {
                out.failed_items += 1;
                out.first_error.get_or_insert(explain_io_error(&e));
                return;
            }
        }
        if let Err(e) = fs::remove_dir(path) {
            out.failed_items += 1;
            out.first_error.get_or_insert(explain_io_error(&e));
        }
    } else {
        let size = meta.len();
        match fs::remove_file(path) {
            Ok(()) => out.deleted_bytes += size,
            Err(e) => {
                out.failed_items += 1;
                out.first_error.get_or_insert(explain_io_error(&e));
            }
        }
    }
}

pub fn delete_paths(paths: &[PathBuf], force: bool, json: bool) -> Result<()> {
    let mut entries = Vec::new();
    let mut reclaimed = 0u64;
    for path in paths {
        let size = measure(path);
        let entry = if !force {
            DeleteEntry {
                path: path.clone(),
                size,
                deleted: false,
                deleted_bytes: None,
                failed_items: None,
                error: None,
            }
        } else {
            let mut out = DeleteOutcome {
                deleted_bytes: 0,
                failed_items: 0,
                first_error: None,
            };
            best_effort_delete(path, &mut out);
            reclaimed += out.deleted_bytes;
            DeleteEntry {
                path: path.clone(),
                size,
                deleted: out.failed_items == 0,
                deleted_bytes: Some(out.deleted_bytes),
                failed_items: Some(out.failed_items),
                error: out.first_error,
            }
        };
        if !force {
            reclaimed += size;
        }
        entries.push(entry);
    }
    if json {
        let report = DeleteReport {
            dry_run: !force,
            reclaimed,
            entries,
        };
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        for entry in &entries {
            let status = if !force {
                "would delete".to_string()
            } else if entry.deleted {
                "deleted".to_string()
            } else {
                format!(
                    "partial: freed {}, {} item{} left ({})",
                    human_size(entry.deleted_bytes.unwrap_or(0)),
                    entry.failed_items.unwrap_or(0),
                    if entry.failed_items == Some(1) {
                        ""
                    } else {
                        "s"
                    },
                    entry.error.as_deref().unwrap_or("unknown error")
                )
            };
            println!(
                "{:>9}  {}  {}",
                human_size(entry.size),
                status,
                entry.path.display()
            );
        }
        if force {
            println!("reclaimed {}", human_size(reclaimed));
        } else {
            println!(
                "dry run: would reclaim {} (re-run with --force to delete)",
                human_size(reclaimed)
            );
        }
    }
    Ok(())
}

fn measure(path: &Path) -> u64 {
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(_) => return 0,
    };
    if meta.is_dir() {
        let scanner = crate::scan::Scanner::new(crate::scan::ScanOptions {
            exclude: Vec::new(),
            top_files: 0,
            find: Vec::new(),
        });
        scanner.scan(path).root.size
    } else {
        meta.len()
    }
}
