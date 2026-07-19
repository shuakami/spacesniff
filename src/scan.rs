use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use rayon::prelude::*;
use serde::Serialize;

/// One directory in the scanned tree. Children are subdirectories only,
/// sorted by size descending; plain files are aggregated into `size`/`files`.
#[derive(Serialize)]
pub struct DirNode {
    pub name: String,
    pub size: u64,
    pub files: u64,
    pub children: Vec<DirNode>,
}

#[derive(Serialize, Clone)]
pub struct FileEntry {
    pub path: PathBuf,
    pub size: u64,
}

/// A directory matched by name during a `find` scan.
#[derive(Serialize, Clone)]
pub struct FoundDir {
    pub path: PathBuf,
    pub size: u64,
    pub files: u64,
}

pub struct ScanResult {
    pub root: DirNode,
    pub dirs: u64,
    pub errors: u64,
    pub top_files: Vec<FileEntry>,
    pub found: Vec<FoundDir>,
}

pub struct ScanOptions {
    /// Directory names to skip entirely.
    pub exclude: Vec<String>,
    /// Keep this many largest files (0 = don't track files).
    pub top_files: usize,
    /// Directory names to report as matches (outermost match wins;
    /// matches nested inside another match are not reported separately).
    pub find: Vec<String>,
}

pub struct Scanner {
    opts: ScanOptions,
    dirs: AtomicU64,
    errors: AtomicU64,
    heap: Mutex<BinaryHeap<Reverse<(u64, PathBuf)>>>,
    found: Mutex<Vec<FoundDir>>,
    /// Fast-path filter: smallest size currently in a full heap.
    heap_floor: AtomicU64,
    /// Directories currently being scanned as parallel tasks. Fanning every
    /// tiny directory out to the thread pool costs more in task overhead and
    /// scheduler contention than it saves (measured 10x slower on Windows),
    /// so recursion goes sequential once enough tasks are in flight to keep
    /// all threads busy.
    inflight: AtomicUsize,
    /// Adaptive in-flight budget, tuned during the scan by `Tuner`.
    max_inflight: AtomicUsize,
    tuner: Mutex<Tuner>,
}

/// Hill-climbing controller for the in-flight budget: every window of
/// scanned directories, compare throughput (dirs/sec) with the previous
/// window; keep moving the budget in the same direction while throughput
/// improves, reverse direction when it degrades.
struct Tuner {
    window_start: Instant,
    window_start_dirs: u64,
    prev_rate: f64,
    increasing: bool,
}

const TUNE_WINDOW: u64 = 2048;

impl Tuner {
    fn tick(&mut self, dirs: u64, budget: &AtomicUsize, min: usize, max: usize) {
        let elapsed = self.window_start.elapsed().as_secs_f64();
        let scanned = dirs - self.window_start_dirs;
        if scanned < TUNE_WINDOW || elapsed <= 0.0 {
            return;
        }
        let rate = scanned as f64 / elapsed;
        if self.prev_rate > 0.0 && rate < self.prev_rate {
            self.increasing = !self.increasing;
        }
        let current = budget.load(Ordering::Relaxed);
        let next = if self.increasing {
            (current + current / 2).clamp(min, max)
        } else {
            (current - current / 4).clamp(min, max)
        };
        budget.store(next, Ordering::Relaxed);
        self.prev_rate = rate;
        self.window_start = Instant::now();
        self.window_start_dirs = dirs;
    }
}

impl Scanner {
    pub fn new(opts: ScanOptions) -> Self {
        Scanner {
            opts,
            dirs: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            heap: Mutex::new(BinaryHeap::new()),
            found: Mutex::new(Vec::new()),
            heap_floor: AtomicU64::new(0),
            inflight: AtomicUsize::new(0),
            max_inflight: AtomicUsize::new(rayon::current_num_threads() * 4),
            tuner: Mutex::new(Tuner {
                window_start: Instant::now(),
                window_start_dirs: 0,
                prev_rate: 0.0,
                increasing: true,
            }),
        }
    }

    fn budget_bounds() -> (usize, usize) {
        let threads = rayon::current_num_threads();
        (threads, threads * 64)
    }

    pub fn scan(&self, root: &Path) -> ScanResult {
        let name = root.display().to_string();
        let node = self.scan_dir(root, name, true);
        let mut top_files: Vec<FileEntry> = self
            .heap
            .lock()
            .unwrap()
            .iter()
            .map(|Reverse((size, path))| FileEntry {
                path: path.clone(),
                size: *size,
            })
            .collect();
        top_files.sort_by_key(|f| Reverse(f.size));
        let mut found = std::mem::take(&mut *self.found.lock().unwrap());
        found.sort_by_key(|d| Reverse(d.size));
        ScanResult {
            root: node,
            dirs: self.dirs.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
            top_files,
            found,
        }
    }

    fn scan_dir(&self, path: &Path, name: String, report_matches: bool) -> DirNode {
        let dirs = self.dirs.fetch_add(1, Ordering::Relaxed) + 1;
        if dirs.is_multiple_of(TUNE_WINDOW) {
            if let Ok(mut tuner) = self.tuner.try_lock() {
                let (min, max) = Self::budget_bounds();
                tuner.tick(dirs, &self.max_inflight, min, max);
            }
        }
        let entries = match fs::read_dir(path) {
            Ok(entries) => entries,
            Err(_) => {
                self.errors.fetch_add(1, Ordering::Relaxed);
                return DirNode {
                    name,
                    size: 0,
                    files: 0,
                    children: Vec::new(),
                };
            }
        };

        let mut file_size: u64 = 0;
        let mut file_count: u64 = 0;
        let mut subdirs: Vec<(PathBuf, String)> = Vec::new();

        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    self.errors.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
            };
            let file_type = match entry.file_type() {
                Ok(t) => t,
                Err(_) => {
                    self.errors.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
            };
            if file_type.is_dir() {
                let child_name = entry.file_name().to_string_lossy().into_owned();
                if self.opts.exclude.iter().any(|e| e == &child_name) {
                    continue;
                }
                subdirs.push((entry.path(), child_name));
            } else if file_type.is_file() {
                // On Windows and most platforms DirEntry::metadata is served
                // from the directory read, so this stays cheap.
                match entry.metadata() {
                    Ok(meta) => {
                        let size = meta.len();
                        file_size += size;
                        file_count += 1;
                        if self.opts.top_files > 0 {
                            self.offer_file(entry.path(), size);
                        }
                    }
                    Err(_) => {
                        self.errors.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
            // Symlinks and other special entries are intentionally not followed.
        }

        let scan_child = |child_path: &Path, child_name: String| {
            let is_match = report_matches && self.opts.find.iter().any(|f| f == &child_name);
            let node = self.scan_dir(child_path, child_name, report_matches && !is_match);
            if is_match {
                self.found.lock().unwrap().push(FoundDir {
                    path: child_path.to_path_buf(),
                    size: node.size,
                    files: node.files,
                });
            }
            node
        };
        let fork = subdirs.len() > 1
            && self.inflight.load(Ordering::Relaxed) + subdirs.len()
                <= self.max_inflight.load(Ordering::Relaxed);
        let mut children: Vec<DirNode> = if fork {
            self.inflight.fetch_add(subdirs.len(), Ordering::Relaxed);
            let children = subdirs
                .par_iter()
                .map(|(child_path, child_name)| scan_child(child_path, child_name.clone()))
                .collect();
            self.inflight.fetch_sub(subdirs.len(), Ordering::Relaxed);
            children
        } else {
            subdirs
                .into_iter()
                .map(|(child_path, child_name)| scan_child(&child_path, child_name))
                .collect()
        };
        children.sort_by_key(|c| Reverse(c.size));

        let size = file_size + children.iter().map(|c| c.size).sum::<u64>();
        let files = file_count + children.iter().map(|c| c.files).sum::<u64>();
        DirNode {
            name,
            size,
            files,
            children,
        }
    }

    fn offer_file(&self, path: PathBuf, size: u64) {
        if size <= self.heap_floor.load(Ordering::Relaxed) {
            return;
        }
        let mut heap = self.heap.lock().unwrap();
        if heap.len() < self.opts.top_files {
            heap.push(Reverse((size, path)));
        } else if let Some(Reverse((smallest, _))) = heap.peek() {
            if size > *smallest {
                heap.pop();
                heap.push(Reverse((size, path)));
            }
        }
        if heap.len() == self.opts.top_files {
            if let Some(Reverse((smallest, _))) = heap.peek() {
                self.heap_floor.store(*smallest, Ordering::Relaxed);
            }
        }
    }
}
