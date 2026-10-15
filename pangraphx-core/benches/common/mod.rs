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

/// Inputs at or above this size (bytes) are benchmarked with a reduced sample
/// size, mirroring the `large` flag on the curated fixtures.
const LARGE_THRESHOLD: u64 = 512 * 1024;

/// A single benchmark input: a human-readable label, an absolute path to a GFA
/// file, and whether it is large enough to warrant a reduced sample size.
pub struct BenchInput {
    pub label: String,
    pub path: PathBuf,
    /// Large inputs get a reduced criterion sample size to keep wall-clock sane.
    pub large: bool,
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

/// The curated in-repo fixtures, smallest to largest. GFA carries paths, so
/// these double as the de Bruijn benchmark inputs.
fn curated_inputs() -> Vec<BenchInput> {
    [
        ("tiny", "gfa/tiny.gfa", false),
        ("random", "gfa/random.gfa", false),
        ("phix", "gfa/phix.gfa", false),
        ("lambda", "gfa/escherichia_phage_lambda.gfa", false),
        ("mycoplasma", "gfa/mycoplasma_genitalium.gfa", true),
        ("merged", "gfa/merged.gfa", true),
    ]
    .into_iter()
    .map(|(label, rel, large)| BenchInput {
        label: label.to_string(),
        path: test_file(rel),
        large,
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
            BenchInput {
                label,
                large: size >= LARGE_THRESHOLD,
                path,
            }
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
/// and exclude disk I/O from the measurement.
pub fn read_bytes(path: impl AsRef<Path>) -> Vec<u8> {
    let path = path.as_ref();
    std::fs::read(path)
        .unwrap_or_else(|e| panic!("failed to read bench input {}: {e}", path.display()))
}

/// Parses an in-memory buffer into a [`CoreGraphDTO`], panicking on error.
pub fn parse_bytes(bytes: &[u8], format: GraphFormat) -> CoreGraphDTO {
    let mut cursor = Cursor::new(bytes);
    CoreGraphDTO::load(&mut cursor, format)
        .unwrap_or_else(|e| panic!("failed to parse {format} input: {e}"))
}

/// Serializes a [`CoreGraphDTO`] into an in-memory buffer in the given format.
pub fn serialize_bytes(dto: &CoreGraphDTO, format: GraphFormat) -> Vec<u8> {
    let mut buf = Vec::new();
    dto.save(&mut buf, format)
        .unwrap_or_else(|e| panic!("failed to serialize {format}: {e}"));
    buf
}
