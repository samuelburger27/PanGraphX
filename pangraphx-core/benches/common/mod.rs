//! Shared helpers for the `pangraphx-core` criterion benchmarks.
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

use pangraphx_core::{CoreGraphDTO, GraphFormat};
use std::io::Cursor;
use std::path::PathBuf;

/// A single benchmark input: a human-readable label, a path relative to
/// `tests/test_files/`, and the format it should be parsed as.
pub struct BenchInput {
    pub label: &'static str,
    pub rel_path: &'static str,
    pub format: GraphFormat,
    /// Large inputs get a reduced criterion sample size to keep wall-clock sane.
    pub large: bool,
}

/// GFA inputs, smallest to largest. GFA carries paths, so these double as the
/// de Bruijn benchmark inputs.
pub const GFA_INPUTS: &[BenchInput] = &[
    input("tiny", "gfa/tiny.gfa", GraphFormat::GFA, false),
    input("random", "gfa/random.gfa", GraphFormat::GFA, false),
    input("phix", "gfa/phix.gfa", GraphFormat::GFA, false),
    input(
        "lambda",
        "gfa/escherichia_phage_lambda.gfa",
        GraphFormat::GFA,
        false,
    ),
    input(
        "mycoplasma",
        "gfa/mycoplasma_genitalium.gfa",
        GraphFormat::GFA,
        true,
    ),
    input("merged", "gfa/merged.gfa", GraphFormat::GFA, true),
];

/// VG (protobuf) inputs, smallest to largest.
pub const VG_INPUTS: &[BenchInput] = &[
    input("random", "vg/random.vg", GraphFormat::VG, false),
    input("phix", "vg/phix.vg", GraphFormat::VG, false),
    input(
        "lambda",
        "vg/escherichia_phage_lambda.vg",
        GraphFormat::VG,
        false,
    ),
    input(
        "mycoplasma",
        "vg/mycoplasma_genitalium.vg",
        GraphFormat::VG,
        true,
    ),
    input("merged", "vg/merged.vg", GraphFormat::VG, true),
];

/// GBZ inputs (load-only format, parse benchmarks only).
pub const GBZ_INPUTS: &[BenchInput] = &[
    input("tiny", "gbz/tiny.gbz", GraphFormat::GBZ, false),
    input("random", "gbz/random.gbz", GraphFormat::GBZ, false),
    input(
        "mycoplasma",
        "gbz/mycoplasma_genitalium.gbz",
        GraphFormat::GBZ,
        true,
    ),
];

const fn input(
    label: &'static str,
    rel_path: &'static str,
    format: GraphFormat,
    large: bool,
) -> BenchInput {
    BenchInput {
        label,
        rel_path,
        format,
        large,
    }
}

/// Resolves a path relative to `tests/test_files/` against the crate root.
pub fn test_file(rel_path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/test_files")
        .join(rel_path)
}

/// Reads a test input fully into memory so benchmarks can parse from a `Cursor`
/// and exclude disk I/O from the measurement.
pub fn read_bytes(rel_path: &str) -> Vec<u8> {
    let path = test_file(rel_path);
    std::fs::read(&path)
        .unwrap_or_else(|e| panic!("failed to read bench input {}: {e}", path.display()))
}

/// Parses an in-memory buffer into a [`CoreGraphDTO`], panicking on error.
pub fn parse_bytes(bytes: &[u8], format: GraphFormat) -> CoreGraphDTO {
    let mut cursor = Cursor::new(bytes);
    CoreGraphDTO::load(&mut cursor, format)
        .unwrap_or_else(|e| panic!("failed to parse {format} input: {e}"))
}

/// Convenience: read a file and parse it in one step (used to prepare fixtures
/// outside the measured loop).
pub fn load_input(input: &BenchInput) -> CoreGraphDTO {
    let bytes = read_bytes(input.rel_path);
    parse_bytes(&bytes, input.format)
}
