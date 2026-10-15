//! Format I/O benchmarks: parsing, serialization, and end-to-end conversion.
//!
//! All measurements parse from / serialize to in-memory buffers so that disk
//! I/O is excluded and the numbers reflect the CPU cost of the codecs, with one
//! exception: inputs at or above 256 MiB are streamed from disk via a buffered
//! reader instead of being pre-read, because materializing the raw bytes plus
//! the parsed graph plus derived VG/GBZ buffers would exhaust memory. Each group
//! reports throughput in bytes, so criterion derives MB/s automatically.
//!
//! The only on-disk inputs are GFA files (the in-repo fixtures by default, or a
//! directory pointed to by `PANGRAPHX_BENCH_DATA_DIR`); the VG and GBZ inputs
//! are derived from them by the harness (outside the timed region), so no
//! generated fixtures need to be checked in or created beforehand.
#![allow(clippy::missing_panics_doc, clippy::must_use_candidate)]

mod common;

use common::{
    BenchInput, gfa_inputs, parse_bytes, parse_file, read_bytes, sample_size, serialize_bytes,
    set_timing,
};
use criterion::{
    BatchSize, BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main,
};
use pangraphx_core::{CoreGraphDTO, GraphFormat};

fn as_u64(len: usize) -> u64 {
    u64::try_from(len).expect("byte length fits in u64")
}

/// Loads a GFA dataset from disk and returns its raw bytes plus the parsed
/// [`CoreGraphDTO`] (used to derive the other formats).
fn load_gfa(input: &BenchInput) -> (Vec<u8>, CoreGraphDTO) {
    let bytes = read_bytes(&input.path);
    let dto = parse_bytes(&bytes, GraphFormat::GFA);
    (bytes, dto)
}

/// Group `parse`: decode each format across the size range (GFA, VG, GBZ).
fn bench_parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("parse");
    for input in gfa_inputs() {
        set_timing(&mut group, &input);
        if input.is_huge() {
            // Stream the GFA from disk and derive VG/GBZ one at a time so peak
            // memory is `DTO + one buffer` instead of `raw GFA + DTO + VG + GBZ`.
            let dto = parse_file(&input.path, GraphFormat::GFA);

            group.throughput(Throughput::Bytes(input.size));
            group.sample_size(sample_size(&input));
            group.bench_with_input(
                BenchmarkId::new(GraphFormat::GFA.to_string(), input.label.as_str()),
                &input.path,
                |b, path| b.iter(|| parse_file(black_box(path), GraphFormat::GFA)),
            );

            let vg_bytes = serialize_bytes(&dto, GraphFormat::VG);
            group.throughput(Throughput::Bytes(as_u64(vg_bytes.len())));
            group.sample_size(sample_size(&input));
            group.bench_with_input(
                BenchmarkId::new(GraphFormat::VG.to_string(), input.label.as_str()),
                &vg_bytes,
                |b, bytes| b.iter(|| parse_bytes(black_box(bytes), GraphFormat::VG)),
            );
            drop(vg_bytes);

            let gbz_bytes = serialize_bytes(&dto, GraphFormat::GBZ);
            group.throughput(Throughput::Bytes(as_u64(gbz_bytes.len())));
            group.sample_size(sample_size(&input));
            group.bench_with_input(
                BenchmarkId::new(GraphFormat::GBZ.to_string(), input.label.as_str()),
                &gbz_bytes,
                |b, bytes| b.iter(|| parse_bytes(black_box(bytes), GraphFormat::GBZ)),
            );
            drop(gbz_bytes);
        } else {
            let (gfa_bytes, dto) = load_gfa(&input);
            let vg_bytes = serialize_bytes(&dto, GraphFormat::VG);
            let gbz_bytes = serialize_bytes(&dto, GraphFormat::GBZ);

            for (format, bytes) in [
                (GraphFormat::GFA, &gfa_bytes),
                (GraphFormat::VG, &vg_bytes),
                (GraphFormat::GBZ, &gbz_bytes),
            ] {
                group.throughput(Throughput::Bytes(as_u64(bytes.len())));
                group.sample_size(sample_size(&input));
                group.bench_with_input(
                    BenchmarkId::new(format.to_string(), input.label.as_str()),
                    bytes,
                    |b, bytes| b.iter(|| parse_bytes(black_box(bytes), format)),
                );
            }
        }
    }
    group.finish();
}

/// Group `serialize`: encode a parsed graph back to bytes (GFA, VG, and GBZ).
fn bench_serialize(c: &mut Criterion) {
    let mut group = c.benchmark_group("serialize");
    for input in gfa_inputs() {
        set_timing(&mut group, &input);
        let dto = if input.is_huge() {
            parse_file(&input.path, GraphFormat::GFA)
        } else {
            load_gfa(&input).1
        };

        for format in [GraphFormat::GFA, GraphFormat::VG, GraphFormat::GBZ] {
            // Serialize once to learn the output size (for throughput + buffer
            // pre-allocation) without timing it.
            let mut probe = Vec::new();
            dto.save(&mut probe, format)
                .expect("probe serialization failed");
            let out_len = probe.len();

            group.throughput(Throughput::Bytes(as_u64(out_len)));
            group.sample_size(sample_size(&input));
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

/// Group `convert`: full parse + serialize, mirroring what `vg convert` does.
///
/// Only the real pangenome fixtures are used (the trivial `tiny` and the
/// synthetic `random` are skipped). The VG side of each pair is generated from
/// the GFA by the harness.
fn bench_convert(c: &mut Criterion) {
    let mut group = c.benchmark_group("convert");
    for input in gfa_inputs()
        .iter()
        .filter(|i| !matches!(i.label.as_str(), "tiny" | "random"))
    {
        // Huge inputs get the minimum sample size (Criterion rejects anything
        // below 10) plus a reduced measurement time via `set_timing`.
        let size = sample_size(input);
        set_timing(&mut group, input);

        if input.is_huge() {
            // Stream the GFA for the forward conversion; derive the VG in-memory
            // once for the reverse conversion, then drop the setup DTO.
            let vg_bytes =
                serialize_bytes(&parse_file(&input.path, GraphFormat::GFA), GraphFormat::VG);

            group.throughput(Throughput::Bytes(input.size));
            group.sample_size(size);
            group.bench_with_input(
                BenchmarkId::new("gfa_to_vg", input.label.as_str()),
                &input.path,
                |b, path| {
                    b.iter(|| {
                        let dto = parse_file(black_box(path), GraphFormat::GFA);
                        let mut out = Vec::new();
                        dto.save(&mut out, GraphFormat::VG)
                            .expect("GFA->VG serialization failed");
                        out
                    });
                },
            );

            group.throughput(Throughput::Bytes(as_u64(vg_bytes.len())));
            group.sample_size(size);
            group.bench_with_input(
                BenchmarkId::new("vg_to_gfa", input.label.as_str()),
                &vg_bytes,
                |b, bytes| {
                    b.iter(|| {
                        let dto = parse_bytes(black_box(bytes), GraphFormat::VG);
                        let mut out = Vec::new();
                        dto.save(&mut out, GraphFormat::GFA)
                            .expect("VG->GFA serialization failed");
                        out
                    });
                },
            );
        } else {
            let (gfa_bytes, dto) = load_gfa(input);
            let vg_bytes = serialize_bytes(&dto, GraphFormat::VG);

            // GFA -> VG
            group.throughput(Throughput::Bytes(as_u64(gfa_bytes.len())));
            group.sample_size(size);
            group.bench_with_input(
                BenchmarkId::new("gfa_to_vg", input.label.as_str()),
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
            group.throughput(Throughput::Bytes(as_u64(vg_bytes.len())));
            group.sample_size(size);
            group.bench_with_input(
                BenchmarkId::new("vg_to_gfa", input.label.as_str()),
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
    }
    group.finish();
}

criterion_group!(benches, bench_parse, bench_serialize, bench_convert);
criterion_main!(benches);
