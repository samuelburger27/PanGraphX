use crate::cli::args_parser::DbgArgs;
use crate::graph_io::{load_graph, output_format, save_graph};
use crate::utils::group;
use anyhow::{Context, Result};
use log::info;
use pangraphx_core::{ColoredDBG, CoreGraphDTO, DeBruijn};
use std::time::Instant;

pub fn handle_dbg(args: &DbgArgs) -> Result<()> {
    let format = output_format(&args.output)?;
    let graph = load_graph(&args.input)?;

    let mode = if args.colored {
        "colored"
    } else if args.full_topology {
        "full topology"
    } else {
        "paths"
    };
    let nodes = if args.compact { "unitigs" } else { "k-mers" };
    info!(
        "Building de Bruijn graph (k = {}, mode: {mode}, nodes: {nodes})",
        args.kmer_size
    );
    let start = Instant::now();
    let dbg = build_dbg(graph, args).context("failed to build the de Bruijn graph")?;
    info!(
        "Built {} nodes, {} edges in {:.2?}",
        group(dbg.nodes.len()),
        group(dbg.edges.len()),
        start.elapsed()
    );

    save_graph(&dbg, &args.output, format)
}

/// Builds the de Bruijn graph of `graph` in the mode selected by `args`.
fn build_dbg(graph: CoreGraphDTO, args: &DbgArgs) -> Result<CoreGraphDTO> {
    let k = args.kmer_size;
    if args.colored {
        let dbg = ColoredDBG::from_directed_graph(graph, k)?;
        return Ok(if args.compact {
            dbg.into_compacted_dto()?
        } else {
            dbg.try_into()?
        });
    }
    let dbg = if args.full_topology {
        DeBruijn::from_directed_graph_full_topology(graph, k)?
    } else {
        DeBruijn::from_directed_graph(graph, k)?
    };
    Ok(if args.compact {
        dbg.into_compacted_dto()?
    } else {
        dbg.try_into()?
    })
}
