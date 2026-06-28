//! Format I/O benchmarks: parsing, serialization, and end-to-end conversion.
//!
//! All measurements parse from / serialize to in-memory buffers so that disk
//! I/O is excluded and the numbers reflect the CPU cost of the codecs. Each
//! group reports throughput in bytes, so criterion derives MB/s automatically.
#![allow(clippy::missing_panics_doc, clippy::must_use_candidate)]

mod common;

use common::{BenchInput, GBZ_INPUTS, GFA_INPUTS, VG_INPUTS, parse_bytes, read_bytes};
use criterion::{
    BatchSize, BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main,
};
use pangraphx_core::GraphFormat;

/// Sample size for large inputs; criterion's default (100) for everything else.
const LARGE_SAMPLE_SIZE: usize = 10;
const DEFAULT_SAMPLE_SIZE: usize = 100;

const fn sample_size(input: &BenchInput) -> usize {
    if input.large {
        LARGE_SAMPLE_SIZE
    } else {
        DEFAULT_SAMPLE_SIZE
    }
}

fn as_u64(len: usize) -> u64 {
    u64::try_from(len).expect("byte length fits in u64")
}

/// Group `parse`: decode each format across the size range (GFA, VG, GBZ).
fn bench_parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("parse");
    for inputs in [GFA_INPUTS, VG_INPUTS, GBZ_INPUTS] {
        for input in inputs {
            let bytes = read_bytes(input.rel_path);
            group.throughput(Throughput::Bytes(as_u64(bytes.len())));
            group.sample_size(sample_size(input));
            group.bench_with_input(
                BenchmarkId::new(input.format.to_string(), input.label),
                &bytes,
                |b, bytes| {
                    b.iter(|| parse_bytes(black_box(bytes), input.format));
                },
            );
        }
    }
    group.finish();
}

/// Group `serialize`: encode a parsed graph back to bytes (GFA and VG only;
/// GBZ is load-only).
fn bench_serialize(c: &mut Criterion) {
    let mut group = c.benchmark_group("serialize");
    for inputs in [GFA_INPUTS, VG_INPUTS] {
        for input in inputs {
            let bytes = read_bytes(input.rel_path);
            let dto = parse_bytes(&bytes, input.format);

            // Serialize once to learn the output size (for throughput + buffer
            // pre-allocation) without timing it.
            let mut probe = Vec::new();
            dto.save(&mut probe, input.format)
                .expect("probe serialization failed");
            let out_len = probe.len();

            group.throughput(Throughput::Bytes(as_u64(out_len)));
            group.sample_size(sample_size(input));
            group.bench_with_input(
                BenchmarkId::new(input.format.to_string(), input.label),
                &dto,
                |b, dto| {
                    b.iter_batched(
                        || Vec::with_capacity(out_len),
                        |mut buf| {
                            dto.save(&mut buf, input.format)
                                .expect("serialization failed");
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

/// A GFA/VG pair of the same graph, used for end-to-end conversion both ways.
struct ConvertPair {
    label: &'static str,
    gfa_path: &'static str,
    vg_path: &'static str,
    large: bool,
}

const CONVERT_PAIRS: &[ConvertPair] = &[
    ConvertPair {
        label: "phix",
        gfa_path: "gfa/phix.gfa",
        vg_path: "vg/phix.vg",
        large: false,
    },
    ConvertPair {
        label: "lambda",
        gfa_path: "gfa/escherichia_phage_lambda.gfa",
        vg_path: "vg/escherichia_phage_lambda.vg",
        large: false,
    },
    ConvertPair {
        label: "mycoplasma",
        gfa_path: "gfa/mycoplasma_genitalium.gfa",
        vg_path: "vg/mycoplasma_genitalium.vg",
        large: true,
    },
    ConvertPair {
        label: "merged",
        gfa_path: "gfa/merged.gfa",
        vg_path: "vg/merged.vg",
        large: true,
    },
];

/// Group `convert`: full parse + serialize, mirroring what `vg convert` does.
fn bench_convert(c: &mut Criterion) {
    let mut group = c.benchmark_group("convert");
    for pair in CONVERT_PAIRS {
        let size = if pair.large {
            LARGE_SAMPLE_SIZE
        } else {
            DEFAULT_SAMPLE_SIZE
        };

        // GFA -> VG
        let gfa_bytes = read_bytes(pair.gfa_path);
        group.throughput(Throughput::Bytes(as_u64(gfa_bytes.len())));
        group.sample_size(size);
        group.bench_with_input(
            BenchmarkId::new("gfa_to_vg", pair.label),
            &gfa_bytes,
            |b, bytes| {
                b.iter(|| {
                    let dto = parse_bytes(bytes, GraphFormat::GFA);
                    let mut out = Vec::new();
                    dto.save(&mut out, GraphFormat::VG)
                        .expect("GFA->VG serialization failed");
                    out
                });
            },
        );

        // VG -> GFA
        let vg_bytes = read_bytes(pair.vg_path);
        group.throughput(Throughput::Bytes(as_u64(vg_bytes.len())));
        group.sample_size(size);
        group.bench_with_input(
            BenchmarkId::new("vg_to_gfa", pair.label),
            &vg_bytes,
            |b, bytes| {
                b.iter(|| {
                    let dto = parse_bytes(bytes, GraphFormat::VG);
                    let mut out = Vec::new();
                    dto.save(&mut out, GraphFormat::GFA)
                        .expect("VG->GFA serialization failed");
                    out
                });
            },
        );
    }
    group.finish();
}

criterion_group!(benches, bench_parse, bench_serialize, bench_convert);
criterion_main!(benches);
