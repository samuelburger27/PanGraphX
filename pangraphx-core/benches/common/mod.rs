//! Shared helpers for the `pangraphx-core` benchmarks.
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

use criterion::measurement::Measurement;
use criterion::{BenchmarkGroup, Throughput};
use pangraphx_core::{CoreGraphDTO, GraphFormat};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Environment variable that overrides the directory scanned for GFA datasets.
/// Set it to a directory of `.gfa` files to benchmark those instead of the
/// in-repo fixtures, or to `auto` to benchmark the in-repo fixtures plus every
/// GFA in the workspace's `benchmarks/data/` (where `utils/download_gfa.sh`
/// puts the large graphs).
pub const DATASET_DIR_ENV: &str = "PANGRAPHX_BENCH_DATA_DIR";

/// Environment variable overriding [`DEFAULT_DBG_MAX_PATH_BP`].
pub const DBG_MAX_BP_ENV: &str = "PANGRAPHX_BENCH_DBG_MAX_BP";

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
    /// Builds an input from a path, reading its size from disk.
    pub fn from_path(label: impl Into<String>, path: PathBuf) -> Self {
        let size = std::fs::metadata(&path)
            .unwrap_or_else(|e| panic!("failed to stat {}: {e}", path.display()))
            .len();
        Self {
            label: label.into(),
            path,
            size,
        }
    }

    #[must_use]
    pub const fn size_class(&self) -> SizeClass {
        match self.size {
            s if s >= HUGE_THRESHOLD => SizeClass::Huge,
            s if s >= LARGE_THRESHOLD => SizeClass::Large,
            s if s >= MEDIUM_THRESHOLD => SizeClass::Medium,
            _ => SizeClass::Small,
        }
    }

    /// Huge inputs are streamed from disk rather than held fully in memory.
    #[must_use]
    pub fn is_huge(&self) -> bool {
        self.size_class() == SizeClass::Huge
    }

    /// Parses the input as GFA (streamed for huge inputs, in-memory otherwise).
    pub fn parse(&self) -> CoreGraphDTO {
        Source::for_input(self).parse(GraphFormat::GFA)
    }
}

/// Where a benchmark reads a serialized graph from.
///
/// Inputs below the huge threshold are pre-read into memory so the measurement
/// excludes disk I/O. Huge inputs are streamed from disk through a buffered
/// reader instead, because holding the raw bytes alongside the parsed graph
/// (and derived VG/GBZ buffers) would exhaust memory; after the first read the
/// page cache makes the measurement mostly CPU work.
pub enum Source {
    Bytes(Vec<u8>),
    File(PathBuf),
}

impl Source {
    /// The GFA source for an input: streamed when huge, pre-read otherwise.
    pub fn for_input(input: &BenchInput) -> Self {
        if input.is_huge() {
            Self::File(input.path.clone())
        } else {
            Self::Bytes(read_bytes(&input.path))
        }
    }

    /// The serialized size in bytes, used for throughput.
    pub fn len(&self) -> u64 {
        match self {
            Self::Bytes(bytes) => as_u64(bytes.len()),
            Self::File(path) => std::fs::metadata(path).map_or(0, |m| m.len()),
        }
    }

    /// Parses the source in the given format, panicking on error.
    pub fn parse(&self, format: GraphFormat) -> CoreGraphDTO {
        match self {
            Self::Bytes(bytes) => parse_bytes(bytes, format),
            Self::File(path) => parse_file(path, format),
        }
    }
}

/// Criterion's hard minimum for `sample_size`; its setter asserts `n >= 10`.
pub const MIN_SAMPLE_SIZE: usize = 10;

/// Criterion sample size for an input, derived from its size class.
///
/// Criterion rejects `sample_size` below [`MIN_SAMPLE_SIZE`], so huge inputs use
/// the minimum and a shorter measurement time instead (see [`configure`]).
#[must_use]
pub const fn sample_size(input: &BenchInput) -> usize {
    match input.size_class() {
        SizeClass::Small => 100,
        SizeClass::Medium => 20,
        SizeClass::Large | SizeClass::Huge => MIN_SAMPLE_SIZE,
    }
}

// Criterion's defaults, restored for non-huge inputs so a huge input earlier in
// the same group does not leak its shortened timing into later benchmarks.
const DEFAULT_MEASUREMENT_TIME: Duration = Duration::from_secs(5);
const DEFAULT_WARM_UP_TIME: Duration = Duration::from_secs(3);
const HUGE_MEASUREMENT_TIME: Duration = Duration::from_secs(1);
const HUGE_WARM_UP_TIME: Duration = Duration::from_secs(1);

/// Prepares `group` for the next benchmark on `input`: sample size and timing
/// from the input's size class, and byte throughput when given.
pub fn configure<M: Measurement>(
    group: &mut BenchmarkGroup<'_, M>,
    input: &BenchInput,
    throughput_bytes: Option<u64>,
) {
    group.sample_size(sample_size(input));
    if input.is_huge() {
        group.measurement_time(HUGE_MEASUREMENT_TIME);
        group.warm_up_time(HUGE_WARM_UP_TIME);
    } else {
        group.measurement_time(DEFAULT_MEASUREMENT_TIME);
        group.warm_up_time(DEFAULT_WARM_UP_TIME);
    }
    if let Some(bytes) = throughput_bytes {
        group.throughput(Throughput::Bytes(bytes));
    }
}

/// Returns the GFA datasets to benchmark, smallest first.
///
/// * [`DATASET_DIR_ENV`] unset: the curated in-repo fixtures.
/// * set to `auto`: the curated fixtures plus every `*.gfa` in `benchmarks/data/`.
/// * set to a directory: every `*.gfa` in that directory.
///
/// The VG and GBZ inputs are always derived from these in-memory.
pub fn gfa_inputs() -> Vec<BenchInput> {
    let mut inputs = match std::env::var_os(DATASET_DIR_ENV) {
        None => curated_inputs(),
        Some(dir) if dir == "auto" => {
            let mut inputs = curated_inputs();
            let data_dir = workspace_root().join("benchmarks/data");
            if data_dir.is_dir() {
                inputs.extend(discover_inputs(&data_dir));
            }
            inputs
        }
        Some(dir) => discover_inputs(Path::new(&dir)),
    };
    inputs.sort_by_key(|i| i.size);
    inputs
}

/// The single largest GFA dataset (the curated `merged` by default).
pub fn largest_input() -> BenchInput {
    gfa_inputs().pop().expect("no GFA inputs available")
}

/// Looks up a dataset by label among [`gfa_inputs`].
pub fn input_named(label: &str) -> Option<BenchInput> {
    gfa_inputs().into_iter().find(|i| i.label == label)
}

/// The curated in-repo fixtures. GFA carries paths, so these double as the de
/// Bruijn benchmark inputs.
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
    .map(|(label, rel)| BenchInput::from_path(label, test_file(rel)))
    .collect()
}

/// Scans `dir` for `*.gfa` files.
fn discover_inputs(dir: &Path) -> Vec<BenchInput> {
    let entries = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("failed to read dataset dir {}: {e}", dir.display()));

    let inputs: Vec<BenchInput> = entries
        .map(|entry| entry.unwrap_or_else(|e| panic!("failed to read dir entry: {e}")))
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("gfa"))
        .map(|path| {
            let label = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("gfa")
                .to_string();
            BenchInput::from_path(label, path)
        })
        .collect();

    assert!(
        !inputs.is_empty(),
        "no .gfa files found in {}",
        dir.display()
    );
    inputs
}

// --- de Bruijn inputs -------------------------------------------------------

/// Default cap on *total path bases* for de Bruijn construction benches.
///
/// DBG memory scales with path bases, not file size: a 55 MB GFA whose paths
/// reuse nodes can spell out 500 Mbp and need tens of GiB. Path-based
/// construction at k = 31 peaks around ~190 B per path base on one thread and
/// ~480 B on eight, because the per-thread partial sets overlap (measured with
/// `benches/dbg_memory.rs`), so the default keeps a run within a few GiB.
/// Override with [`DBG_MAX_BP_ENV`].
pub const DEFAULT_DBG_MAX_PATH_BP: u64 = 10_000_000;

/// The effective path-base cap (default or [`DBG_MAX_BP_ENV`]).
pub fn dbg_max_path_bp() -> u64 {
    std::env::var(DBG_MAX_BP_ENV)
        .ok()
        .and_then(|v| v.replace('_', "").parse().ok())
        .unwrap_or(DEFAULT_DBG_MAX_PATH_BP)
}

/// Total bases spelled out by all paths of a graph (overlaps not subtracted, so
/// this slightly over-counts, which is the safe direction for a memory cap).
pub fn path_bases(dto: &CoreGraphDTO) -> u64 {
    dto.paths()
        .iter()
        .flat_map(|path| &path.steps)
        .map(|step| as_u64(dto.nodes()[step.node_id()].sequence.len()))
        .sum()
}

/// A parsed input that passed the de Bruijn path-base cap.
pub struct DbgInput {
    pub input: BenchInput,
    pub dto: CoreGraphDTO,
    pub path_bp: u64,
}

/// Parses `input` and keeps it only if its paths are under the cap.
pub fn dbg_input(input: BenchInput) -> Option<DbgInput> {
    let dto = input.parse();
    let path_bp = path_bases(&dto);
    let cap = dbg_max_path_bp();
    if path_bp == 0 || path_bp > cap {
        eprintln!(
            "skipping {} for de Bruijn benches: {path_bp} path bp (cap {cap}, set {DBG_MAX_BP_ENV} to change)",
            input.label
        );
        return None;
    }
    Some(DbgInput {
        input,
        dto,
        path_bp,
    })
}

/// de Bruijn inputs: the named curated fixtures by default, or — when
/// [`DATASET_DIR_ENV`] is set — the largest input under the path-base cap.
///
/// Larger inputs are tried first and rejected one by one, so a directory of
/// huge graphs costs a parse per graph but never a DBG construction.
pub fn dbg_inputs(default_labels: &[&str]) -> Vec<DbgInput> {
    if std::env::var_os(DATASET_DIR_ENV).is_none() {
        return default_labels
            .iter()
            .filter_map(|label| input_named(label))
            .filter_map(dbg_input)
            .collect();
    }
    gfa_inputs()
        .into_iter()
        .rev()
        .find_map(dbg_input)
        .into_iter()
        .collect()
}

/// Criterion sample size for de Bruijn construction, by path bases.
#[must_use]
pub const fn dbg_sample_size(path_bp: u64) -> usize {
    match path_bp {
        0..100_000 => 50,
        100_000..1_000_000 => 20,
        _ => MIN_SAMPLE_SIZE,
    }
}

// --- I/O helpers ------------------------------------------------------------

pub fn as_u64(len: usize) -> u64 {
    u64::try_from(len).expect("length fits in u64")
}

pub fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate has a parent directory")
        .to_path_buf()
}

/// Resolves a path relative to `tests/test_files/` against the crate root.
pub fn test_file(rel_path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/test_files")
        .join(rel_path)
}

/// Reads a file fully into memory.
pub fn read_bytes(path: &Path) -> Vec<u8> {
    std::fs::read(path)
        .unwrap_or_else(|e| panic!("failed to read bench input {}: {e}", path.display()))
}

/// Parses an in-memory buffer into a [`CoreGraphDTO`], panicking on error.
pub fn parse_bytes(bytes: &[u8], format: GraphFormat) -> CoreGraphDTO {
    let mut cursor = Cursor::new(bytes);
    CoreGraphDTO::load(&mut cursor, format)
        .unwrap_or_else(|e| panic!("failed to parse {format} input: {e}"))
}

/// Parses a file on disk into a [`CoreGraphDTO`] through a buffered reader.
pub fn parse_file(path: &Path, format: GraphFormat) -> CoreGraphDTO {
    CoreGraphDTO::load_from_file(&path.to_string_lossy(), format)
        .unwrap_or_else(|e| panic!("failed to parse {format} from {}: {e}", path.display()))
}

/// Serializes a [`CoreGraphDTO`] into an in-memory buffer in the given format.
pub fn serialize_bytes(dto: &CoreGraphDTO, format: GraphFormat) -> Vec<u8> {
    let mut buf = Vec::new();
    dto.save(&mut buf, format)
        .unwrap_or_else(|e| panic!("failed to serialize {format}: {e}"));
    buf
}
