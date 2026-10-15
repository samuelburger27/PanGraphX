//! de Bruijn graph construction benchmarks.
//!
//! Two angles:
//! * `dbg_vs_k`   — standard construction as a function of the k-mer size `k`.
//! * `dbg_modes`  — standard vs. full-topology vs. colored at a fixed `k`.
//!
//! The construction functions consume the [`CoreGraphDTO`] by value, so every
//! sample clones a freshly parsed fixture via `iter_batched` /
//! `BatchSize::LargeInput`; cloning is not part of the measured routine.
//!
//! By default the curated fixtures are used; when `PANGRAPHX_BENCH_DATA_DIR` is
//! set, the largest GFA under [`DBG_MAX_BYTES`] there is benchmarked (and the
//! bench is skipped if every file exceeds the cap, since in-memory de Bruijn
//! graph construction would exhaust memory).
#![allow(clippy::missing_panics_doc)]

mod common;

use common::{BenchInput, DATASET_DIR_ENV, largest_dbg_input, parse_input, test_file};
use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use pangraphx_core::{ColoredDBG, DeBruijn};

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

/// The `(input, sample_size)` pairs to sweep over `k`.
///
/// Returns `None` when `PANGRAPHX_BENCH_DATA_DIR` is set but no GFA there is
/// under [`DBG_MAX_BYTES`], so the caller can skip the group entirely.
fn vs_k_inputs() -> Option<Vec<(BenchInput, usize)>> {
    if std::env::var_os(DATASET_DIR_ENV).is_some() {
        largest_dbg_input().map(|input| vec![(input, 10)])
    } else {
        Some(
            VS_K_INPUTS
                .iter()
                .map(|&(label, rel, sample)| {
                    let path = test_file(rel);
                    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                    (
                        BenchInput {
                            label: label.to_string(),
                            path,
                            size,
                        },
                        sample,
                    )
                })
                .collect(),
        )
    }
}

/// The fixture for the mode comparison: lambda by default, or the largest GFA
/// under [`DBG_MAX_BYTES`] in `PANGRAPHX_BENCH_DATA_DIR` when set.
fn modes_input() -> Option<BenchInput> {
    if std::env::var_os(DATASET_DIR_ENV).is_some() {
        largest_dbg_input()
    } else {
        Some(BenchInput {
            label: "lambda".to_string(),
            path: test_file("gfa/escherichia_phage_lambda.gfa"),
            size: 0,
        })
    }
}

/// Group `dbg_vs_k`: standard construction across k-mer sizes.
fn bench_dbg_vs_k(c: &mut Criterion) {
    let Some(inputs) = vs_k_inputs() else {
        eprintln!(
            "skipping dbg_vs_k: no GFA input under the {} MiB de Bruijn cap",
            common::DBG_MAX_BYTES / (1024 * 1024)
        );
        return;
    };

    let mut group = c.benchmark_group("dbg_vs_k");
    for (input, sample) in inputs {
        let dto = parse_input(&input);
        group.sample_size(sample);
        for &k in K_VALUES {
            group.bench_with_input(BenchmarkId::new(input.label.as_str(), k), &k, |b, &k| {
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
/// Uses the smaller lambda graph by default because full-topology enumerates
/// topological walks, which is substantially heavier than the path-based modes.
fn bench_dbg_modes(c: &mut Criterion) {
    let Some(input) = modes_input() else {
        eprintln!(
            "skipping dbg_modes: no GFA input under the {} MiB de Bruijn cap",
            common::DBG_MAX_BYTES / (1024 * 1024)
        );
        return;
    };

    let dto = parse_input(&input);
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
