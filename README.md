# PanGraphX

[![CI](https://github.com/samuelburger27/PanGraphX/actions/workflows/ci.yml/badge.svg)](https://github.com/samuelburger27/PanGraphX/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

A Rust library and command-line tool for converting, inspecting and
transforming pangenome graphs, including de Bruijn graph construction.

- `pangraphx-core` — the library ([README](pangraphx-core/README.md))
- `pangraphx-cli` — the command-line tool

## Supported formats

| Format | Extension | Notes |
|---|---|---|
| GFA 1.0 / 1.1 | `.gfa` | `W` lines are read as paths with vg-style names and written as `P` lines; `C`/`J` lines are ignored |
| VG | `.vg` | Older protobuf format, read plain or BGZF-compressed; per-step path overlaps are dropped |
| GBZ | `.gbz` | keeps only what the paths imply (no overlaps, off-path edges); nodes are not chopped |
| ODGI | `.og` | native reader/writer; overlaps are dropped |
| FASTG | `.fastg` | SPAdes dialect; paths and overlaps are dropped |

VG and ODGI keep node ids when every node name is a distinct positive integer;
otherwise nodes are numbered `1..=n`.

## Performance

> **Converts the HPRC chr22 graph to VG in 1.8 s instead of 4.5 minutes:
> 150× faster than `vg convert` with 29× less memory.**
> Faster than vg and odgi in 5 of 6 conversions on large graphs.

| Conversion | vs. | chr22 PanGraphX | chr22 reference | Speedup | ecoli50 speedup |
|---|---|---:|---:|---:|---:|
| GFA → VG | `vg convert` | **1.8 s** / 0.6 GiB | 269.8 s / 17.3 GiB | **150×** | **98×** |
| VG → GFA | `vg convert` | **7.2 s** / 2.0 GiB | 38.2 s / 4.2 GiB | **5.3×** | **5.7×** |
| GFA → OG | `odgi build` | **2.5 s** / 1.4 GiB | 7.2 s / 1.6 GiB | **2.9×** | **2.8×** |
| OG → GFA | `odgi view` | **2.6 s** / 1.3 GiB | 6.7 s / 0.9 GiB | **2.6×** | **2.3×** |
| GFA → GBZ | `vg gbwt` | **33.4 s** / 1.3 GiB | 36.3 s / 1.8 GiB | **1.1×** | **1.4×** |
| GBZ → GFA | `vg convert` | 1.5 s / 0.7 GiB | **1.0 s** / 0.6 GiB | 0.7× | 0.7× |

Wall-clock time (mean of 3 runs) / peak RSS against vg v1.73.0 and odgi
v0.9.4, all threads, Intel Core i5-14500 (20 threads), Linux.

- `chr22`: HPRC v1.1 Minigraph-Cactus chr22, 659 MB GFA, 1.28 M nodes, 512 paths
- `ecoli50`: 50 *E. coli* genomes, 390 MB GFA, 1.55 M nodes

Full results: [`benchmarks/nymfe_results.md`](benchmarks/nymfe_results.md);
how to reproduce them: [`benchmarks/README.md`](benchmarks/README.md).

## Installation

Requires Rust 1.85+ and `protoc` (the VG protobuf schema is compiled at build time).

```bash
git clone https://github.com/samuelburger27/PanGraphX.git
cd PanGraphX
cargo build --release          # → target/release/pangraphx-cli
cargo build --profile dist     # fat LTO, slower build, fastest binary → target/dist/pangraphx-cli
```

## Usage

```bash
# Convert; formats are inferred from the extensions
pangraphx-cli convert graph.gfa -o graph.gbz

# Use - for stdin/stdout; the format must then be given explicitly
zcat graph.gfa.gz | pangraphx-cli convert - --from gfa -o - --to vg > graph.vg

# Summary and detailed statistics (N50, components, degrees, path lengths)
pangraphx-cli info graph.gfa
pangraphx-cli stats graph.gfa

# de Bruijn graph with k = 31 (1–63) from the sequences spelled by the paths
pangraphx-cli dbg graph.vg -k 31 -o dbg.gfa

# Colored DBG (keeps every input path), compacted into unitigs
pangraphx-cli dbg graph.vg -k 31 -c -u -o unitigs.gfa

# Unitig sequences as FASTA, one record per unitig
pangraphx-cli dbg graph.vg -k 31 -u -o unitigs.fa

# List formats
pangraphx-cli formats
```

`dbg` options: `-c/--colored` keeps the input paths as walks through the
k-mers, `-t/--full-topology` uses every walk of the bidirected graph instead of
only the paths, and `-u/--compact` merges non-branching k-mer chains into
unitigs. The output is GFA, or FASTA for unitigs: the other graph formats
cannot store the `k − 1` overlaps between de Bruijn nodes. It is written
straight from the k-mer tables, and colored paths are walked again on output
instead of being stored, so peak memory is that of the construction.

Global options: `--threads N` (default: all cores), `-q` / `-v` to reduce or
increase logging. Logs go to stderr, so stdout carries only data; `RUST_LOG`
overrides the log level.

## Library

```rust
use pangraphx_core::{CoreGraph, CoreGraphDTO, DeBruijn, GraphFormat};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let graph = CoreGraphDTO::load_from_file("graph.gfa", GraphFormat::GFA)?;
    graph.save_to_file("graph.vg", GraphFormat::VG)?;

    let stats = CoreGraph::new(graph.clone()).compute_stats();
    println!("{} nodes, N50 {} bp", stats.node_count, stats.node_len.n50);

    let dbg = DeBruijn::from_directed_graph(graph, 31)?;
    dbg.into_compacted_dto()?.save_to_file("unitigs.gfa", GraphFormat::GFA)?;
    Ok(())
}
```

## Development

```bash
cargo test --workspace
cargo clippy --all-targets --all-features
cargo fmt --all --check
```

Cross-validation against real `vg` and `odgi` (skipped when they are not on `PATH`):

```bash
cargo test --release -p pangraphx-core --test cross_validation
```

## License

MIT — see [LICENSE](LICENSE).
