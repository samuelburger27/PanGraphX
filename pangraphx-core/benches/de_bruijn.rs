//! de Bruijn graph construction benchmarks (time; see `dbg_memory` for memory).
//!
//! Two angles:
//! * `dbg_vs_k`   — standard construction as a function of the k-mer size `k`.
//! * `dbg_modes`  — standard vs. full-topology vs. colored at a fixed `k`.
//!
//! The construction functions consume the [`CoreGraphDTO`] by value, so every
//! sample clones a freshly parsed fixture via `iter_batched` /
//! `BatchSize::LargeInput`; cloning is not part of the measured routine.
//!
//! Inputs come from [`common::dbg_inputs`]: the named curated fixtures by
//! default, or the largest GFA under the path-base cap when
//! `PANGRAPHX_BENCH_DATA_DIR` is set. Inputs over the cap are skipped, since
//! in-memory construction would exhaust memory.
//!
//! [`CoreGraphDTO`]: pangraphx_core::CoreGraphDTO
#![allow(clippy::missing_panics_doc)]

mod common;

use common::{dbg_inputs, dbg_sample_size};
use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use pangraphx_core::{ColoredDBG, DeBruijn};

/// k-mer sizes to sweep. Capped at 63 because k-mers are 2-bit packed into a
/// `u128` (2 * 64 bits would overflow the rolling mask).
const K_VALUES: &[usize] = &[7, 11, 15, 21, 31, 47, 63];

/// Fixed k for the mode comparison.
const MODE_K: usize = 31;

/// Group `dbg_vs_k`: standard construction across k-mer sizes. `mycoplasma` is
/// the headline graph; `lambda` gives a second, smaller scaling curve.
fn bench_dbg_vs_k(c: &mut Criterion) {
    let mut group = c.benchmark_group("dbg_vs_k");
    for input in dbg_inputs(&["lambda", "mycoplasma"]) {
        group.sample_size(dbg_sample_size(input.path_bp));
        for &k in K_VALUES {
            group.bench_with_input(
                BenchmarkId::new(input.input.label.as_str(), k),
                &k,
                |b, &k| {
                    b.iter_batched(
                        || input.dto.clone(),
                        |graph| DeBruijn::from_directed_graph(graph, k),
                        BatchSize::LargeInput,
                    );
                },
            );
        }
    }
    group.finish();
}

/// Group `dbg_modes`: standard vs. full-topology vs. colored at fixed k.
///
/// Uses the smaller lambda graph by default because full-topology enumerates
/// topological walks, which is substantially heavier than the path-based modes.
fn bench_dbg_modes(c: &mut Criterion) {
    let mut group = c.benchmark_group("dbg_modes");
    for input in dbg_inputs(&["lambda"]) {
        group.sample_size(dbg_sample_size(input.path_bp));
        let label = input.input.label.as_str();
        let dto = &input.dto;

        group.bench_function(BenchmarkId::new(format!("standard/{label}"), MODE_K), |b| {
            b.iter_batched(
                || dto.clone(),
                |graph| DeBruijn::from_directed_graph(graph, MODE_K),
                BatchSize::LargeInput,
            );
        });

        group.bench_function(
            BenchmarkId::new(format!("full_topology/{label}"), MODE_K),
            |b| {
                b.iter_batched(
                    || dto.clone(),
                    |graph| DeBruijn::from_directed_graph_full_topography(graph, MODE_K),
                    BatchSize::LargeInput,
                );
            },
        );

        group.bench_function(BenchmarkId::new(format!("colored/{label}"), MODE_K), |b| {
            b.iter_batched(
                || dto.clone(),
                |graph| ColoredDBG::from_directed_graph(graph, MODE_K),
                BatchSize::LargeInput,
            );
        });
    }
    group.finish();
}

criterion_group!(benches, bench_dbg_vs_k, bench_dbg_modes);
criterion_main!(benches);
