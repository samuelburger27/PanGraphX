use std::collections::HashSet;

use super::convert::infer_graph_format;
use crate::cli::args_parser::DeBruijnArgs;
use anyhow::{Ok, Result};
use log::debug;
use pangraphx_core::{CoreGraph, DeBruijn};

pub fn handle_de_bruijn(args: &DeBruijnArgs) -> Result<()> {
    // Function implementation goes here
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
    let mut graph = CoreGraph::load_from_file(&args.input, input_format)?;
    let db_graph = DeBruijn::from_directed_graph(&graph, args.kmer_size);
    graph = db_graph.into();
    // debug
    let set = graph
        .nodes
        .iter()
        .map(|node| node.sequence.clone())
        .collect::<HashSet<_>>();
    println!("Different nodes in de Bruijn graph: {}", set.len());
    graph.save_to_file(&args.output, output_format)?;
    Ok(())
}
