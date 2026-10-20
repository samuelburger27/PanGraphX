# PanGraphX
[![CI](https://github.com/samuelburger27/PanGraphX/actions/workflows/ci.yml/badge.svg)](https://github.com/samuelburger27/PanGraphX/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
---

A fast and efficient Rust library and command-line tool for working with pangenome graphs. PanGraphX provides seamless conversion between multiple genome graph formats and includes tools for de Bruijn graph construction and genome graph analysis.

## Features

- **Format Conversion**: Convert between GFA, GBZ, VG, ODGI and FASTG formats
- **Graph Inspection**: Query basic information about graph files
- **Graph Statistics**: Report sequence-length metrics (incl. N50), connected components, degree distribution, and path lengths
- **Format Validation**: Validate graph structure and format compliance
- **De Bruijn Graphs**: Construct de Bruijn graphs from existing genome graphs with configurable k-mer sizes
- **Colored De Bruijn Graphs**: Generate colored de Bruijn graphs for multi-sample analysis
- **Library & CLI**: Use as a Rust library or standalone command-line tool

## Supported Formats

- **GFA** (Graphical Fragment Assembly) v1.0, v1.1 with local extensions
- **GBZ** (Graph Bioinformatics Zipped) - compressed graph format
- **VG** (Variation Graph) - protobuf-based variation graph format
- **FASTG** (FASTA Gapped) - sequence format with assembly graph metadata; written in the SPAdes style (both strands, `EDGE_<name>_length_<len>_cov_1` names). Paths and overlaps are dropped
- **ODGI** (Optimized Dynamic Genome/Graph Implementation) - odgi's binary `.og` format, read and written natively. Node ids are kept when the node names are positive integers, otherwise nodes are numbered `1..=n`; overlaps are dropped

## Quick Start

### Installation

#### From Source

```bash
git clone https://github.com/samuelburger27/PanGraphX.git
cd PanGraphX
cargo build --release
```

The compiled binary will be available at `target/release/pangraphx-cli`.

For the fastest binary (fat LTO, used by the benchmarks), build with `cargo build --profile dist`; it is placed at `target/dist/pangraphx-cli`. The build takes noticeably longer.

### CLI Usage

#### List Supported Formats

```bash
pangraphx-cli formats
```

#### Convert Between Formats

Convert a GFA file to GBZ format with automatic format detection:

```bash
pangraphx-cli convert graph.gfa -o graph.gbz
```

Explicitly specify input and output formats:

```bash
pangraphx-cli convert graph.gfa -o graph.gbz --from gfa --to gbz
```

#### Inspect Graph Files

Display basic information about a graph:

```bash
pangraphx-cli info graph.gfa
```

#### Compute Graph Statistics

Print detailed statistics about a graph — sequence-length distribution (total,
min/avg/max, N50), the number of weakly connected components, the in/out degree
distribution, and the path-length distribution:

```bash
pangraphx-cli stats graph.gfa
```

Override the input format when it cannot be inferred from the file extension:

```bash
pangraphx-cli stats graph.dat --format gfa
```

Example output:

```text
Graph Statistics:
--------------------------------------------------
 Sequence
   Nodes:             169
   Total length:      5,386 bp
   Node length:       min 10 / avg 31.9 / max 32 bp
   N50:               32 bp
 Topology
   Edges:             168
   Components:        1   (largest 169 nodes)
   Isolated nodes:    0
   In-degree:         min 0 / avg 0.99 / max 1
   Out-degree:        min 0 / avg 0.99 / max 1
   Degree histogram:  1:2  2:167
 Paths
   Paths:             1
   Path length:       min 5,386 / avg 5386.0 / max 5,386 bp   (total 5,386 bp)
   Path steps:        min 169 / avg 169.0 / max 169
```

#### De Bruijn Graph Construction

Convert a variation graph to a de Bruijn graph with k-mer size 31 (1-63).
Nodes are the canonical k-mers of the sequences spelled by the paths:

```bash
pangraphx-cli ddb graph.vg -k 31 -o dbg_graph.gfa
```

Generate a colored de Bruijn graph (for multi-sample analysis); every input
path is kept as a walk through the k-mers:

```bash
pangraphx-cli ddb graph.vg -k 31 -c -o colored_dbg.gfa
```

Use full topology (all topological walks instead of just haplotype paths):

```bash
pangraphx-cli ddb graph.vg -k 31 --full-topology -o full_dbg.gfa
```

Add `--compact` to any of these to merge non-branching k-mer chains into
unitig nodes, which makes the output graph much smaller:

```bash
pangraphx-cli ddb graph.vg -k 31 -c --compact -o colored_unitigs.gfa
```

### Library Usage

Add PanGraphX to your `Cargo.toml`:

```toml
[dependencies]
pangraphx-core = { path = "./pangraphx-core" }
```

#### Parse a Graph

```rust
use std::fs::File;
use pangraphx_core::{GraphFormat, traits::GraphParser};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut file = File::open("graph.gfa")?;
    let parser = GraphFormat::GFA.get_parser();
    let graph = parser.parse(&mut file)?;
    
    println!("Graph loaded successfully");
    Ok(())
}
```

#### Convert Between Formats Code

```rust
use std::fs::File;
use pangraphx_core::{GraphFormat, traits::{GraphParser, GraphSerializer}};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse GFA
    let mut input = File::open("graph.gfa")?;
    let parser = GraphFormat::GFA.get_parser();
    let graph = parser.parse(&mut input)?;
    
    // Serialize to GBZ
    let serializer = GraphFormat::GBZ.get_serializer();
    let output = File::create("graph.gbz")?;
    serializer.serialize(&graph, output)?;
    
    Ok(())
}
```

#### Work with De Bruijn Graphs

```rust
use pangraphx_core::{CoreGraphDTO, DeBruijn, GraphFormat};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let graph = CoreGraphDTO::load_from_file("graph.vg", GraphFormat::VG)?;

    // Create de Bruijn graph with k=31
    let dbg = DeBruijn::from_directed_graph(graph, 31)?;
    println!("De Bruijn graph created with {} nodes", dbg.node_count());

    // One node per k-mer, or `dbg.into_compacted_dto()?` for unitigs
    let dto = CoreGraphDTO::try_from(dbg)?;
    dto.save_to_file("dbg.gfa", GraphFormat::GFA)?;
    Ok(())
}
```

#### Compute Graph Statistics

```rust
use pangraphx_core::{CoreGraph, CoreGraphDTO, GraphFormat};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dto = CoreGraphDTO::load_from_file("graph.gfa", GraphFormat::GFA)?;
    let graph = CoreGraph::new(dto);
    let stats = graph.compute_stats();

    println!("Nodes: {}", stats.node_count);
    println!("Total sequence length: {} bp", stats.node_len.total);
    println!("N50: {} bp", stats.node_len.n50);
    println!("Connected components: {}", stats.component_count);
    Ok(())
}
```

## Performance

Wall-clock time (mean of 3 runs) and peak RSS (median) of `pangraphx-cli convert`
against vg v1.73.0 and odgi v0.9.4 on the two large benchmark graphs, measured
with `benchmarks/compare_tools.sh` on an Intel Core i5-14500 (20 hardware threads,
31 GiB RAM, Linux). All tools ran with all threads available; every input in a
non-GFA format was written by the reference tool.

- `ecoli50`: 50 *E. coli* genomes, 390 MB GFA, 1.55 M nodes, 250 Mbp of paths
- `chr22`: HPRC v1.1 Minigraph-Cactus chr22, 659 MB GFA, 1.28 M nodes, 512 paths, 3.2 Gbp of paths

| Conversion | Reference tool | ecoli50 PanGraphX | ecoli50 reference | chr22 PanGraphX | chr22 reference |
|---|---|---:|---:|---:|---:|
| GFA → VG | `vg convert` | **1.3 s** / 0.5 GiB | 127.7 s / 9.6 GiB | **1.8 s** / 0.6 GiB | 269.8 s / 17.3 GiB |
| VG → GFA | `vg convert` | **4.0 s** / 1.3 GiB | 22.7 s / 2.4 GiB | **7.2 s** / 2.0 GiB | 38.2 s / 4.2 GiB |
| GFA → GBZ | `vg gbwt` | **17.5 s** / 1.3 GiB | 24.3 s / 1.6 GiB | **33.4 s** / 1.3 GiB | 36.3 s / 1.8 GiB |
| GBZ → GFA | `vg convert` | 1.0 s / 0.6 GiB | **0.7 s** / 0.6 GiB | 1.5 s / 0.7 GiB | **1.0 s** / 0.6 GiB |
| GFA → OG | `odgi build` | **1.7 s** / 1.0 GiB | 4.8 s / 1.2 GiB | **2.5 s** / 1.4 GiB | 7.2 s / 1.6 GiB |
| OG → GFA | `odgi view` | **1.5 s** / 0.8 GiB | 3.5 s / 0.6 GiB | **2.6 s** / 1.3 GiB | 6.7 s / 0.9 GiB |

PanGraphX is faster on five of the six conversions (on chr22 GFA → GBZ only by
9 %); vg's GBZ → GFA is
1.4–1.6× faster, and odgi needs less memory to write GFA from `.og`. On inputs
below ~1 MB, where every tool finishes in tens of milliseconds, the results are
mixed. See [`benchmarks/nymfe_results.md`](benchmarks/nymfe_results.md) for every
dataset and the analysis, and [`benchmarks/README.md`](benchmarks/README.md) to
reproduce the measurements.

## Project Structure

``` text
pangraphx-core/          # Core library crate
├── src/
│   ├── core/           # Internal graph representation (CoreGraph, CoreGraphDTO)
│   ├── formats/        # Format-specific parsers and serializers
│   ├── de_bruijn_conversion/  # De Bruijn graph algorithms
│   ├── traits.rs       # GraphParser and GraphSerializer traits
│   └── error.rs        # Error types
├── tests/              # Integration tests
└── Cargo.toml

pangraphx-cli/          # CLI tool crate
├── src/
│   ├── main.rs         # Entry point
│   ├── cli/            # CLI argument definitions
│   ├── handle_convert.rs  # Format conversion handler
│   ├── handle_info.rs     # Graph info handler
│   ├── handle_stats.rs    # Graph statistics handler
│   └── ...
└── Cargo.toml
```

## Building & Testing

### Build the Project

```bash
# Build both library and CLI
cargo build --workspace

# Build release version (optimized)
cargo build --workspace --release
```

### Run Tests

```bash
# Run all tests
cargo test --workspace

# Run tests for a specific package
cargo test --package pangraphx-core

# Run tests for a specific module
cargo test --package pangraphx-core -- formats::gfa_format
```

### Environment Setup

Enable debug logging:

```bash
RUST_LOG=debug pangraphx-cli convert graph.gfa -o graph.vg
```

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## References

- [GFA Format Specification](https://github.com/GFA-spec/GFA-spec)
- [Variation Graph Toolkit](https://github.com/vgteam/vg)
- [Genome Graphs and Indexing](https://www.ncbi.nlm.nih.gov/pmc/articles/PMC8289141/)
