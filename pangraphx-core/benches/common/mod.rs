//! Shared helpers for the `pangraphx-core` criterion benchmarks.
//!
//! This module lives in a subdirectory so it is *not* picked up as a bench
//! target (see `autobenches = false` in `Cargo.toml`); each bench file pulls it
//! in with `mod common;`. Because not every bench uses every helper, the whole
//! module allows `dead_code`.
#![allow(
    dead_code,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::missing_errors_doc
)]

use pangraphx_core::{CoreGraphDTO, GraphFormat};
use std::io::Cursor;
use std::path::{Path, PathBuf};

/// Environment variable that overrides the directory scanned for GFA datasets.
/// Set it to a directory of `.gfa` files to benchmark those instead of the
/// in-repo fixtures.
pub const DATASET_DIR_ENV: &str = "PANGRAPHX_BENCH_DATA_DIR";

// Size-class thresholds (bytes). Inputs above these are benchmarked with a
// progressively smaller criterion sample size to keep wall-clock sane.
const MEDIUM_THRESHOLD: u64 = 1024 * 1024; //    1 MiB
const LARGE_THRESHOLD: u64 = 64 * 1024 * 1024; //  64 MiB
const HUGE_THRESHOLD: u64 = 256 * 1024 * 1024; // 256 MiB

/// How large an input is, driving the criterion sample size and whether the
/// file is parsed in-memory or streamed from disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizeClass {
    Small,
    Medium,
    Large,
    Huge,
}

/// A single benchmark input: a human-readable label, an absolute path to a GFA
/// file, and its size in bytes.
pub struct BenchInput {
    pub label: String,
    pub path: PathBuf,
    pub size: u64,
}

impl BenchInput {
    #[must_use]
    pub const fn size_class(&self) -> SizeClass {
        classify(self.size)
    }

    /// Huge inputs are streamed from disk rather than held fully in memory.
    #[must_use]
    pub fn is_huge(&self) -> bool {
        self.size_class() == SizeClass::Huge
    }
}

#[must_use]
const fn classify(size: u64) -> SizeClass {
    if size >= HUGE_THRESHOLD {
        SizeClass::Huge
    } else if size >= LARGE_THRESHOLD {
        SizeClass::Large
    } else if size >= MEDIUM_THRESHOLD {
        SizeClass::Medium
    } else {
        SizeClass::Small
    }
}

/// Criterion sample size for an input, derived from its size class.
#[must_use]
pub const fn sample_size(input: &BenchInput) -> usize {
    match input.size_class() {
        SizeClass::Small => 100,
        SizeClass::Medium => 20,
        SizeClass::Large => 10,
        SizeClass::Huge => 3,
    }
}

/// Returns the GFA datasets to benchmark.
///
/// When [`DATASET_DIR_ENV`] is set, every `*.gfa` file in that directory is used
/// (sorted by size); otherwise the curated in-repo fixtures under
/// `tests/test_files/gfa/` are used. The VG and GBZ inputs are always derived
/// from these in-memory by the bench harness.
pub fn gfa_inputs() -> Vec<BenchInput> {
    std::env::var_os(DATASET_DIR_ENV)
        .map(PathBuf::from)
        .map_or_else(curated_inputs, |dir| discover_inputs(&dir))
}

/// The single largest GFA dataset. Used by the parallel and de Bruijn benches,
/// which pin one fixture (the curated `merged` by default, or the biggest file
/// in `PANGRAPHX_BENCH_DATA_DIR` when set).
pub fn largest_input() -> BenchInput {
    let mut inputs = gfa_inputs();
    inputs.sort_by_key(|i| i.size);
    inputs.pop().expect("no GFA inputs available")
}

/// The curated in-repo fixtures, smallest to largest. GFA carries paths, so
/// these double as the de Bruijn benchmark inputs.
fn curated_inputs() -> Vec<BenchInput> {
    [
        ("tiny", "gfa/tiny.gfa"),
        ("random", "gfa/random.gfa"),
        ("phix", "gfa/phix.gfa"),
        ("lambda", "gfa/escherichia_phage_lambda.gfa"),
        ("mycoplasma", "gfa/mycoplasma_genitalium.gfa"),
        ("merged", "gfa/merged.gfa"),
    ]
    .into_iter()
    .map(|(label, rel)| {
        let path = test_file(rel);
        let size = std::fs::metadata(&path)
            .unwrap_or_else(|e| panic!("failed to stat {}: {e}", path.display()))
            .len();
        BenchInput {
            label: label.to_string(),
            path,
            size,
        }
    })
    .collect()
}

/// Scans `dir` for `*.gfa` files and turns them into inputs sorted by size.
fn discover_inputs(dir: &Path) -> Vec<BenchInput> {
    let entries = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("failed to read dataset dir {}: {e}", dir.display()));

    let mut files: Vec<(u64, PathBuf)> = Vec::new();
    for entry in entries {
        let entry = entry.unwrap_or_else(|e| panic!("failed to read dir entry: {e}"));
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("gfa") {
            continue;
        }
        let size = std::fs::metadata(&path)
            .unwrap_or_else(|e| panic!("failed to stat {}: {e}", path.display()))
            .len();
        files.push((size, path));
    }

    assert!(
        !files.is_empty(),
        "no .gfa files found in {}",
        dir.display()
    );

    files.sort_unstable();
    files
        .into_iter()
        .map(|(size, path)| {
            let label = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("gfa")
                .to_string();
            BenchInput { label, path, size }
        })
        .collect()
}

/// Resolves a path relative to `tests/test_files/` against the crate root.
pub fn test_file(rel_path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/test_files")
        .join(rel_path)
}

/// Reads a test input fully into memory so benchmarks can parse from a `Cursor`
/// and exclude disk I/O from the measurement. Relative paths are resolved
/// against `tests/test_files/`; absolute paths (e.g. from `PANGRAPHX_BENCH_DATA_DIR`)
/// are used as-is.
pub fn read_bytes(path: impl AsRef<Path>) -> Vec<u8> {
    let path = path.as_ref();
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        test_file(path.to_string_lossy().as_ref())
    };
    std::fs::read(&resolved)
        .unwrap_or_else(|e| panic!("failed to read bench input {}: {e}", resolved.display()))
}

/// Parses an in-memory buffer into a [`CoreGraphDTO`], panicking on error.
pub fn parse_bytes(bytes: &[u8], format: GraphFormat) -> CoreGraphDTO {
    let mut cursor = Cursor::new(bytes);
    CoreGraphDTO::load(&mut cursor, format)
        .unwrap_or_else(|e| panic!("failed to parse {format} input: {e}"))
}

/// Parses a file on disk into a [`CoreGraphDTO`], streaming through a buffered
/// reader so the raw bytes are never fully materialized by the harness. Used for
/// huge inputs; the first read primes the page cache, so subsequent iterations
/// measure mostly CPU work.
pub fn parse_file(path: impl AsRef<Path>, format: GraphFormat) -> CoreGraphDTO {
    let path = path.as_ref();
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        test_file(path.to_string_lossy().as_ref())
    };
    CoreGraphDTO::load_from_file(&resolved.to_string_lossy(), format)
        .unwrap_or_else(|e| panic!("failed to parse {format} from {}: {e}", resolved.display()))
}

/// Parses a [`BenchInput`] as GFA, choosing the streaming path for huge files
/// and the in-memory path otherwise.
pub fn parse_input(input: &BenchInput) -> CoreGraphDTO {
    if input.is_huge() {
        parse_file(&input.path, GraphFormat::GFA)
    } else {
        parse_bytes(&read_bytes(&input.path), GraphFormat::GFA)
    }
}

/// Serializes a [`CoreGraphDTO`] into an in-memory buffer in the given format.
pub fn serialize_bytes(dto: &CoreGraphDTO, format: GraphFormat) -> Vec<u8> {
    let mut buf = Vec::new();
    dto.save(&mut buf, format)
        .unwrap_or_else(|e| panic!("failed to serialize {format}: {e}"));
    buf
}
