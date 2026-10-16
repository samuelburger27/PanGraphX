//! Rayon thread-scaling benchmarks.
//!
//! `PanGraphX` parallelizes its heavy operations on Rayon's global pool. Each
//! benchmark here pins the work to a dedicated pool of `n` threads via
//! `ThreadPoolBuilder::install`, with `n = 1` serving as the sequential
//! baseline, so criterion plots a speedup curve over thread count.
//!
//! Parse and serialize use the largest GFA available (the curated `merged.gfa`
//! by default, or the biggest file in `PANGRAPHX_BENCH_DATA_DIR` when set);
//! huge inputs are streamed from disk. DBG construction uses the largest input
//! under the de Bruijn path-base cap instead, since its memory grows with both
//! path bases and thread count (`dbg_memory` measures exactly that).
#![allow(clippy::missing_panics_doc)]

mod common;

use common::{Source, configure, dbg_inputs, largest_input, serialize_bytes};
use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use pangraphx_core::{DeBruijn, GraphFormat};
use rayon::ThreadPool;

const DBG_K: usize = 31;

/// Thread counts to sweep: powers of two up to the machine's parallelism, plus
/// the machine maximum itself.
fn thread_counts() -> Vec<usize> {
    let max = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    let mut counts: Vec<usize> = [1, 2, 4, 8].into_iter().filter(|&n| n <= max).collect();
    if !counts.contains(&max) {
        counts.push(max);
    }
    counts
}

fn build_pool(n: usize) -> ThreadPool {
    rayon::ThreadPoolBuilder::new()
        .num_threads(n)
        .build()
        .expect("failed to build rayon thread pool")
}

/// Group `parallel_dbg`: de Bruijn construction scaled across thread counts.
fn bench_parallel_dbg(c: &mut Criterion) {
    let Some(input) = dbg_inputs(&["merged"]).pop() else {
        eprintln!("skipping parallel_dbg: no input under the de Bruijn path-base cap");
        return;
    };

    let mut group = c.benchmark_group("parallel_dbg");
    group.sample_size(common::MIN_SAMPLE_SIZE);
    for n in thread_counts() {
        let pool = build_pool(n);
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter_batched(
                || input.dto.clone(),
                |graph| pool.install(|| DeBruijn::from_directed_graph(graph, DBG_K)),
                BatchSize::LargeInput,
            );
        });
    }
    group.finish();
}

/// Group `parallel_parse`: GFA parsing (edge/path construction uses `par_iter`).
fn bench_parallel_parse(c: &mut Criterion) {
    let input = largest_input();
    let source = Source::for_input(&input);

    let mut group = c.benchmark_group("parallel_parse");
    configure(&mut group, &input, None);
    for n in thread_counts() {
        let pool = build_pool(n);
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| pool.install(|| source.parse(GraphFormat::GFA)));
        });
    }
    group.finish();
}

/// Group `parallel_serialize`: GFA serialization (formats records with `par_iter`).
fn bench_parallel_serialize(c: &mut Criterion) {
    let input = largest_input();
    let dto = input.parse();
    let out_len = serialize_bytes(&dto, GraphFormat::GFA).len();

    let mut group = c.benchmark_group("parallel_serialize");
    configure(&mut group, &input, None);
    for n in thread_counts() {
        let pool = build_pool(n);
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter_batched(
                || Vec::with_capacity(out_len),
                |mut buf| {
                    pool.install(|| {
                        dto.save(&mut buf, GraphFormat::GFA)
                            .expect("serialization failed");
                    });
                    buf
                },
                BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_parallel_dbg,
    bench_parallel_parse,
    bench_parallel_serialize
);
criterion_main!(benches);
