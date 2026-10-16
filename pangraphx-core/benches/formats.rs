//! Format I/O benchmarks: parsing, serialization, and end-to-end conversion.
//!
//! Inputs are parsed from / serialized to in-memory buffers so that disk I/O is
//! excluded and the numbers reflect the CPU cost of the codecs. The exception
//! is huge GFA inputs (>= 256 MiB), which are streamed from disk (see
//! [`common::Source`]). Each group reports throughput in bytes, so criterion
//! derives MB/s automatically.
//!
//! The only on-disk inputs are GFA files (see [`common::gfa_inputs`]); the VG
//! and GBZ inputs are derived from them by the harness, outside the timed
//! region, and materialized one at a time to bound peak memory.
#![allow(clippy::missing_panics_doc, clippy::must_use_candidate)]

mod common;

use common::{BenchInput, Source, as_u64, configure, gfa_inputs, serialize_bytes};
use criterion::{BatchSize, BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use pangraphx_core::{CoreGraphDTO, GraphFormat};

/// The formats derived from each GFA input for the parse benchmarks.
const DERIVED_FORMATS: [GraphFormat; 2] = [GraphFormat::VG, GraphFormat::GBZ];

/// Group `parse`: decode each format across the size range (GFA, VG, GBZ).
fn bench_parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("parse");
    for input in gfa_inputs() {
        let gfa = Source::for_input(&input);
        let dto = gfa.parse(GraphFormat::GFA);

        let mut bench = |format: GraphFormat, source: &Source| {
            configure(&mut group, &input, Some(source.len()));
            group.bench_with_input(
                BenchmarkId::new(format.to_string(), input.label.as_str()),
                source,
                |b, source| b.iter(|| black_box(source).parse(format)),
            );
        };

        bench(GraphFormat::GFA, &gfa);
        drop(gfa);
        for format in DERIVED_FORMATS {
            bench(format, &Source::Bytes(serialize_bytes(&dto, format)));
        }
    }
    group.finish();
}

/// Group `serialize`: encode a parsed graph back to bytes (GFA, VG, and GBZ).
fn bench_serialize(c: &mut Criterion) {
    let mut group = c.benchmark_group("serialize");
    for input in gfa_inputs() {
        let dto = input.parse();

        for format in [GraphFormat::GFA, GraphFormat::VG, GraphFormat::GBZ] {
            // Serialize once to learn the output size (for throughput + buffer
            // pre-allocation) without timing it.
            let out_len = serialize_bytes(&dto, format).len();

            configure(&mut group, &input, Some(as_u64(out_len)));
            group.bench_with_input(
                BenchmarkId::new(format.to_string(), input.label.as_str()),
                &dto,
                |b, dto| {
                    b.iter_batched(
                        || Vec::with_capacity(out_len),
                        |mut buf| {
                            dto.save(&mut buf, format).expect("serialization failed");
                            buf
                        },
                        BatchSize::SmallInput,
                    );
                },
            );
        }
    }
    group.finish();
}

/// Parses `source` as `from` and serializes the result as `to`.
fn convert(source: &Source, from: GraphFormat, to: GraphFormat) -> Vec<u8> {
    let dto: CoreGraphDTO = source.parse(from);
    let mut out = Vec::new();
    dto.save(&mut out, to).expect("serialization failed");
    out
}

/// Group `convert`: full parse + serialize, mirroring what `vg convert` does.
///
/// Only the real pangenome fixtures are used (the trivial `tiny` and the
/// synthetic `random` are skipped). The VG side of each pair is generated from
/// the GFA by the harness.
fn bench_convert(c: &mut Criterion) {
    let mut group = c.benchmark_group("convert");
    let inputs: Vec<BenchInput> = gfa_inputs()
        .into_iter()
        .filter(|i| !matches!(i.label.as_str(), "tiny" | "random"))
        .collect();
    for input in &inputs {
        let gfa = Source::for_input(input);
        let vg = Source::Bytes(serialize_bytes(
            &gfa.parse(GraphFormat::GFA),
            GraphFormat::VG,
        ));

        for (name, source, from, to) in [
            ("gfa_to_vg", &gfa, GraphFormat::GFA, GraphFormat::VG),
            ("vg_to_gfa", &vg, GraphFormat::VG, GraphFormat::GFA),
        ] {
            configure(&mut group, input, Some(source.len()));
            group.bench_with_input(
                BenchmarkId::new(name, input.label.as_str()),
                source,
                |b, source| b.iter(|| convert(black_box(source), from, to)),
            );
        }
    }
    group.finish();
}

criterion_group!(benches, bench_parse, bench_serialize, bench_convert);
criterion_main!(benches);
