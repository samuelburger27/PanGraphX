use super::convert::infer_graph_format;
use crate::cli::args_parser::DeBruijnArgs;
use anyhow::{Ok, Result};
use log::debug;
use pangraphx_core::{ColoredDBG, CoreGraph, DeBruijn, core::graph};

pub fn handle_de_bruijn(args: &DeBruijnArgs) -> Result<()> {
    let input_format = infer_graph_format(&args.input, &args.from).ok_or_else(|| {
        anyhow::anyhow!(
            "Input graph format is not supported or couldn't be inferred: {}",
            args.input
        )
    })?;
    let output_format = infer_graph_format(&args.output, &args.to).ok_or_else(|| {
        anyhow::anyhow!(
            "Output graph format is not supported or couldn't be inferred: {}",
            args.output
        )
    })?;

    debug!("Input format: {:?}", input_format);
    debug!("Output format: {:?}", output_format);
    let graph = CoreGraph::load_from_file(&args.input, input_format)?;
    let final_graph: CoreGraph;
    if args.colored {
        let col_dbg = ColoredDBG::from_directed_graph(&graph, args.kmer_size);
        final_graph = col_dbg.into();
    } else {
        let db_graph = DeBruijn::from_directed_graph(&graph, args.kmer_size);
        final_graph = db_graph.into();
    }
    final_graph.save_to_file(&args.output, output_format)?;
    Ok(())
}

///
fn create_dbg_graph(
    graph: &CoreGraph,
    kmer_size: usize,
    full_topology: bool,
    colored: bool,
) -> CoreGraph {
    if full_topology {
        DeBruijn::from_directed_graph_full_topography(graph, kmer_size)
    } else {
        DeBruijn::from_directed_graph(graph, kmer_size)
    }
}
