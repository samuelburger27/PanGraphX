# pangraphx-core

Core library for [PanGraphX](https://github.com/samuelburger27/PanGraphX): a toolkit for
genome graph loading, manipulation, and format conversion.

## Features

- A common graph DTO (`CoreGraphDTO`) and a mutable graph type with lookup/manipulation
  helpers (`CoreGraph`)
- Parsing and serialization for multiple pangenome graph formats: GFA, VG, GBZ, FASTG,
  and (optionally) ODGI
- de Bruijn graph construction, including colored de Bruijn graphs
- Graph statistics (sequence-length/N50, connected components, degree and path distributions)

## Usage

```toml
[dependencies]
pangraphx-core = "0.1"
```

```rust,no_run
use std::fs::File;
use pangraphx_core::GraphFormat;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = File::open("graph.gfa")?;
    let parser = GraphFormat::GFA.get_parser();
    let graph = parser.parse(&mut input)?;

    let serializer = GraphFormat::GBZ.get_serializer();
    let mut output = File::create("graph.gbz")?;
    serializer.serialize(&graph, &mut output)?;
    Ok(())
}
```

## Features

- `odgi` — enables ODGI binary graph support (Linux only, requires native C++ libraries).

## License

MIT
