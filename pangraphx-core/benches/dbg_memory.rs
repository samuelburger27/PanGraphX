//! de Bruijn graph construction *memory* benchmark.
//!
//! Not a criterion bench: a peak-tracking global allocator records the heap
//! high-water mark of each construction, sweeping mode × k × thread count. Heap
//! bytes are deterministic (unlike RSS, they don't depend on the OS allocator
//! returning pages or on page-cache state), so the numbers are well suited to
//! tracking memory optimizations across commits.
//!
//! For every configuration it reports heap bytes (above the heap in use before
//! the run, so the input graph handed to the construction is included):
//! * `input`        — the parsed input graph;
//! * `build_peak`   — peak while constructing the DBG;
//! * `dbg_retained` — held by the finished DBG;
//! * `total_peak`   — peak including conversion to a `CoreGraphDTO` (what
//!   `pangraphx-cli ddb` does before writing the output);
//! * `bytes_per_bp` — `total_peak` per path base, the figure to drive down.
//!
//! Modes: `standard`, `colored`, `full_topology`, and `compact` /
//! `colored_compact` (the same constructions with unitig output).
//!
//! Configurations run one at a time. Inputs come from [`common::dbg_inputs`]
//! and are capped by total path bases; `full_topology` additionally only runs
//! on inputs under [`FULL_TOPOLOGY_MAX_BP`].
//!
//! ```bash
//! cargo bench -p pangraphx-core --bench dbg_memory
//! DBG_MEMORY_K="31" DBG_MEMORY_THREADS="1 8" cargo bench -p pangraphx-core --bench dbg_memory
//! ```
//!
//! Results are printed and written to `target/dbg_memory.csv` (override with
//! `DBG_MEMORY_CSV`). `cargo bench -- --test` runs a single tiny configuration.
#![allow(clippy::missing_panics_doc, clippy::cast_precision_loss)]

mod common;

use common::{DbgInput, dbg_inputs, workspace_root};
use pangraphx_core::{ColoredDBG, CoreGraphDTO, DeBruijn, PanResult};
use std::alloc::{GlobalAlloc, Layout, System};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

/// `System` allocator wrapper tracking current and peak live heap bytes.
struct PeakAlloc;

static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

impl PeakAlloc {
    fn added(size: usize) {
        let now = CURRENT.fetch_add(size, Ordering::Relaxed) + size;
        PEAK.fetch_max(now, Ordering::Relaxed);
    }

    fn removed(size: usize) {
        CURRENT.fetch_sub(size, Ordering::Relaxed);
    }

    fn current() -> usize {
        CURRENT.load(Ordering::Relaxed)
    }

    fn peak() -> usize {
        PEAK.load(Ordering::Relaxed)
    }

    /// Restarts peak tracking from the current live heap.
    fn reset_peak() {
        PEAK.store(Self::current(), Ordering::Relaxed);
    }
}

// SAFETY: every method forwards to `System` with the caller's arguments
// unchanged; the counters are only bookkeeping and never affect allocation.
unsafe impl GlobalAlloc for PeakAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded verbatim; the caller upholds `alloc`'s contract.
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            Self::added(layout.size());
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded verbatim; the caller upholds `alloc_zeroed`'s contract.
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            Self::added(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: forwarded verbatim; the caller upholds `dealloc`'s contract.
        unsafe { System.dealloc(ptr, layout) };
        Self::removed(layout.size());
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: forwarded verbatim; the caller upholds `realloc`'s contract.
        let new = unsafe { System.realloc(ptr, layout, new_size) };
        if !new.is_null() {
            Self::removed(layout.size());
            Self::added(new_size);
        }
        new
    }
}

#[global_allocator]
static ALLOC: PeakAlloc = PeakAlloc;

/// `full_topology` enumerates walks of the whole graph and can blow up far
/// beyond the path-based modes, so it gets a tighter path-base cap.
const FULL_TOPOLOGY_MAX_BP: u64 = 2_000_000;

#[derive(Clone, Copy)]
enum Mode {
    Standard,
    Colored,
    FullTopology,
    /// Standard construction, output compacted into unitigs.
    Compact,
    /// Colored construction, output compacted into unitigs.
    ColoredCompact,
}

impl Mode {
    const ALL: [Self; 5] = [
        Self::Standard,
        Self::Colored,
        Self::FullTopology,
        Self::Compact,
        Self::ColoredCompact,
    ];

    const fn name(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Colored => "colored",
            Self::FullTopology => "full_topology",
            Self::Compact => "compact",
            Self::ColoredCompact => "colored_compact",
        }
    }
}

struct Sample {
    input: usize,
    build_peak: usize,
    retained: usize,
    total_peak: usize,
    nodes: usize,
    edges: usize,
}

/// Builds the DBG for one configuration. All figures are heap bytes above the
/// live heap before the input graph was cloned, so they include the input
/// (which construction consumes), as in `pangraphx-cli ddb`.
fn measure(dto: &CoreGraphDTO, mode: Mode, k: usize) -> Sample {
    let base = PeakAlloc::current();
    let graph = dto.clone();
    let input = PeakAlloc::current() - base;
    PeakAlloc::reset_peak();

    let (build_peak, retained, out) = match mode {
        Mode::Standard => after_build(base, standard(graph, k), CoreGraphDTO::try_from),
        Mode::FullTopology => after_build(
            base,
            DeBruijn::from_directed_graph_full_topology(graph, k).expect("valid k"),
            CoreGraphDTO::try_from,
        ),
        Mode::Colored => after_build(base, colored(graph, k), CoreGraphDTO::try_from),
        Mode::Compact => after_build(base, standard(graph, k), DeBruijn::into_compacted_dto),
        Mode::ColoredCompact => {
            after_build(base, colored(graph, k), ColoredDBG::into_compacted_dto)
        }
    };
    Sample {
        input,
        build_peak,
        retained,
        total_peak: PeakAlloc::peak().saturating_sub(base),
        nodes: out.nodes.iter().count(),
        edges: out.edges.len(),
    }
}

fn standard(graph: CoreGraphDTO, k: usize) -> DeBruijn {
    DeBruijn::from_directed_graph(graph, k).expect("valid k")
}

fn colored(graph: CoreGraphDTO, k: usize) -> ColoredDBG {
    ColoredDBG::from_directed_graph(graph, k).expect("valid k")
}

/// Records the construction peak and retained size, then converts to a DTO.
fn after_build<D>(
    base: usize,
    dbg: D,
    to_dto: impl FnOnce(D) -> PanResult<CoreGraphDTO>,
) -> (usize, usize, CoreGraphDTO) {
    let build_peak = PeakAlloc::peak().saturating_sub(base);
    let retained = PeakAlloc::current().saturating_sub(base);
    (build_peak, retained, to_dto(dbg).expect("DTO conversion"))
}

fn env_list(name: &str, default: &[usize]) -> Vec<usize> {
    std::env::var(name).map_or_else(
        |_| default.to_vec(),
        |v| {
            v.split_whitespace()
                .map(|s| {
                    s.parse()
                        .unwrap_or_else(|_| panic!("{name}: bad number '{s}'"))
                })
                .collect()
        },
    )
}

fn thread_counts() -> Vec<usize> {
    let max = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    let mut counts: Vec<usize> = [1, 2, 4, 8].into_iter().filter(|&n| n <= max).collect();
    if !counts.contains(&max) {
        counts.push(max);
    }
    env_list("DBG_MEMORY_THREADS", &counts)
}

fn mib(bytes: usize) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn csv_path() -> PathBuf {
    std::env::var_os("DBG_MEMORY_CSV").map_or_else(
        || {
            std::env::var_os("CARGO_TARGET_DIR")
                .map_or_else(|| workspace_root().join("target"), PathBuf::from)
                .join("dbg_memory.csv")
        },
        PathBuf::from,
    )
}

fn main() {
    // `cargo bench -- --test` (smoke test): one tiny configuration.
    let smoke = std::env::args().any(|a| a == "--test");
    let (labels, ks, threads): (&[&str], Vec<usize>, Vec<usize>) = if smoke {
        (&["phix"], vec![31], vec![1])
    } else {
        (
            &["lambda", "mycoplasma", "merged"],
            env_list("DBG_MEMORY_K", &[21, 31, 63]),
            thread_counts(),
        )
    };

    let mut csv = String::from(
        "dataset,path_bp,mode,k,threads,input_bytes,build_peak_bytes,dbg_retained_bytes,total_peak_bytes,bytes_per_bp,dbg_nodes,dbg_edges\n",
    );
    println!(
        "{:<14} {:>10} {:<14} {:>3} {:>3} {:>10} {:>10} {:>10} {:>10} {:>7}",
        "dataset",
        "path_bp",
        "mode",
        "k",
        "thr",
        "input MiB",
        "build MiB",
        "retain MiB",
        "total MiB",
        "B/bp"
    );

    let inputs: Vec<DbgInput> = dbg_inputs(labels);
    for input in &inputs {
        let label = input.input.label.as_str();
        for mode in Mode::ALL {
            if matches!(mode, Mode::FullTopology) && input.path_bp > FULL_TOPOLOGY_MAX_BP {
                eprintln!("skipping full_topology on {label}: over {FULL_TOPOLOGY_MAX_BP} path bp");
                continue;
            }
            for &k in &ks {
                for &n in &threads {
                    let pool = rayon::ThreadPoolBuilder::new()
                        .num_threads(n)
                        .build()
                        .expect("failed to build rayon thread pool");
                    let s = pool.install(|| measure(&input.dto, mode, k));
                    let per_bp = s.total_peak as f64 / input.path_bp as f64;
                    println!(
                        "{label:<14} {:>10} {:<14} {k:>3} {n:>3} {:>10.1} {:>10.1} {:>10.1} {:>10.1} {per_bp:>7.1}",
                        input.path_bp,
                        mode.name(),
                        mib(s.input),
                        mib(s.build_peak),
                        mib(s.retained),
                        mib(s.total_peak),
                    );
                    let _ = writeln!(
                        csv,
                        "{label},{},{},{k},{n},{},{},{},{},{per_bp:.1},{},{}",
                        input.path_bp,
                        mode.name(),
                        s.input,
                        s.build_peak,
                        s.retained,
                        s.total_peak,
                        s.nodes,
                        s.edges
                    );
                }
            }
        }
    }

    if !smoke {
        let path = csv_path();
        std::fs::write(&path, csv)
            .unwrap_or_else(|e| panic!("failed to write {}: {e}", path.display()));
        println!("\nwrote {}", path.display());
    }
}
