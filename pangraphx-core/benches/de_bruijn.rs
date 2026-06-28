//! de Bruijn graph construction benchmarks.
//!
//! Two angles:
//! * `dbg_vs_k`   — standard construction as a function of the k-mer size `k`.
//! * `dbg_modes`  — standard vs. full-topology vs. colored at a fixed `k`.
//!
//! The construction functions consume the [`CoreGraphDTO`] by value, so every
//! sample clones a freshly parsed fixture via `iter_batched` /
//! `BatchSize::LargeInput`; cloning is not part of the measured routine.
#![allow(clippy::missing_panics_doc)]

mod common;

use common::{parse_bytes, read_bytes};
use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use pangraphx_core::{ColoredDBG, CoreGraphDTO, DeBruijn, GraphFormat};

/// k-mer sizes to sweep. Capped at 63 because k-mers are 2-bit packed into a
/// `u128` (2 * 64 bits would overflow the rolling mask).
const K_VALUES: &[usize] = &[7, 11, 15, 21, 31, 47, 63];

/// Fixed k for the mode comparison.
const MODE_K: usize = 31;

/// Inputs for the k-sweep: both carry paths. `mycoplasma` is the headline
/// (~1.2 MB) graph; `lambda` gives a second, smaller scaling curve.
const VS_K_INPUTS: &[(&str, &str, usize)] = &[
    ("lambda", "gfa/escherichia_phage_lambda.gfa", 50),
    ("mycoplasma", "gfa/mycoplasma_genitalium.gfa", 10),
];

fn fixture(rel_path: &str) -> CoreGraphDTO {
    parse_bytes(&read_bytes(rel_path), GraphFormat::GFA)
}

/// Group `dbg_vs_k`: standard construction across k-mer sizes.
fn bench_dbg_vs_k(c: &mut Criterion) {
    let mut group = c.benchmark_group("dbg_vs_k");
    for &(label, path, sample) in VS_K_INPUTS {
        let dto = fixture(path);
        group.sample_size(sample);
        for &k in K_VALUES {
            group.bench_with_input(BenchmarkId::new(label, k), &k, |b, &k| {
                b.iter_batched(
                    || dto.clone(),
                    |graph| DeBruijn::from_directed_graph(graph, k),
                    BatchSize::LargeInput,
                );
            });
        }
    }
    group.finish();
}

/// Group `dbg_modes`: standard vs. full-topology vs. colored at fixed k.
///
/// Uses the smaller lambda graph because full-topology enumerates topological
/// walks, which is substantially heavier than the path-based modes.
fn bench_dbg_modes(c: &mut Criterion) {
    let dto = fixture("gfa/escherichia_phage_lambda.gfa");
    let mut group = c.benchmark_group("dbg_modes");
    group.sample_size(30);

    group.bench_function(BenchmarkId::new("standard", MODE_K), |b| {
        b.iter_batched(
            || dto.clone(),
            |graph| DeBruijn::from_directed_graph(graph, MODE_K),
            BatchSize::LargeInput,
        );
    });

    group.bench_function(BenchmarkId::new("full_topology", MODE_K), |b| {
        b.iter_batched(
            || dto.clone(),
            |graph| DeBruijn::from_directed_graph_full_topography(graph, MODE_K),
            BatchSize::LargeInput,
        );
    });

    group.bench_function(BenchmarkId::new("colored", MODE_K), |b| {
        b.iter_batched(
            || dto.clone(),
            |graph| ColoredDBG::from_directed_graph(graph, MODE_K),
            BatchSize::LargeInput,
        );
    });

    group.finish();
}

criterion_group!(benches, bench_dbg_vs_k, bench_dbg_modes);
criterion_main!(benches);
