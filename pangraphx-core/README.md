# pangraphx-core

Library behind [PanGraphX](https://github.com/samuelburger27/PanGraphX): reading,
writing and transforming pangenome graphs.

- Formats: GFA 1.0/1.1, VG, GBZ, ODGI (`.og`) and FASTG, dispatched through `GraphFormat`
- `CoreGraphDTO` (what parsers return and serializers take) and the mutable `CoreGraph`
- Graph statistics: N50, connected components, degree and path-length distributions
- de Bruijn graphs (`DeBruijn`, colored `ColoredDBG`) for k = 1–63, optionally compacted into unitigs

Building requires `protoc`.

## Usage

```toml
[dependencies]
pangraphx-core = "0.1"
```

```rust,no_run
use pangraphx_core::{CoreGraph, CoreGraphDTO, DeBruijn, GraphFormat};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Convert GFA to GBZ
    let graph = CoreGraphDTO::load_from_file("graph.gfa", GraphFormat::GFA)?;
    graph.save_to_file("graph.gbz", GraphFormat::GBZ)?;

    // Statistics
    let stats = CoreGraph::new(graph.clone()).compute_stats();
    println!("{} nodes, N50 {} bp", stats.node_count, stats.node_len.n50);

    // de Bruijn graph with k = 31, compacted into unitigs
    let dbg = DeBruijn::from_directed_graph(graph, 31)?;
    dbg.into_compacted_dto()?.save_to_file("unitigs.gfa", GraphFormat::GFA)?;
    Ok(())
}
```

For streams, use `CoreGraphDTO::load` / `save`, or `GraphFormat::get_parser` /
`get_serializer` directly.

## License

MIT
