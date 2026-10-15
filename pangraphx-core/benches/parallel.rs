//! Rayon thread-scaling benchmarks.
//!
//! `PanGraphX` parallelizes its heavy operations on Rayon's global pool. Each
//! benchmark here pins the work to a dedicated pool of `n` threads via
//! `ThreadPoolBuilder::install`, with `n = 1` serving as the sequential
//! baseline, so criterion plots a speedup curve over thread count.
//!
//! The fixture is the largest GFA available: the curated `merged.gfa` by
//! default, or the biggest file in `PANGRAPHX_BENCH_DATA_DIR` when set. Huge
//! inputs are streamed from disk; DBG construction is skipped for them because
//! the in-memory de Bruijn graph would be impractically large.
#![allow(clippy::missing_panics_doc)]

mod common;

use common::{largest_input, parse_bytes, parse_file, parse_input, read_bytes};
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
    let input = largest_input();
    if input.is_huge() {
        eprintln!(
            "skipping parallel_dbg: fixture {} ({} bytes) is too large for in-memory DBG construction",
            input.label, input.size
        );
        return;
    }

    let dto = parse_input(&input);
    let mut group = c.benchmark_group("parallel_dbg");
    group.sample_size(10);
    for n in thread_counts() {
        let pool = build_pool(n);
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter_batched(
                || dto.clone(),
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
    // Huge inputs are streamed from disk; everything else is pre-read once so
    // the measurement stays CPU-only.
    let bytes = if input.is_huge() {
        None
    } else {
        Some(read_bytes(&input.path))
    };

    let mut group = c.benchmark_group("parallel_parse");
    group.sample_size(if input.is_huge() { 3 } else { 20 });
    for n in thread_counts() {
        let pool = build_pool(n);
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| {
                pool.install(|| match bytes.as_deref() {
                    Some(bytes) => parse_bytes(bytes, GraphFormat::GFA),
                    None => parse_file(&input.path, GraphFormat::GFA),
                })
            });
        });
    }
    group.finish();
}

/// Group `parallel_serialize`: GFA serialization (formats records with `par_iter`).
fn bench_parallel_serialize(c: &mut Criterion) {
    let input = largest_input();
    let dto = parse_input(&input);
    let mut probe = Vec::new();
    dto.save(&mut probe, GraphFormat::GFA)
        .expect("probe serialization failed");
    let out_len = probe.len();

    let mut group = c.benchmark_group("parallel_serialize");
    group.sample_size(if input.is_huge() { 3 } else { 20 });
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
