use super::convert::infer_graph_format;
use crate::cli::args_parser::DeBruijnArgs;
use anyhow::{Ok, Result};
use log::{debug, warn};
use pangraphx_core::{CoreGraph, GraphFormat, Kmer, LookUpGraph};
use std::path::Path;

pub fn handle_de_bruijn(args: &DeBruijnArgs) -> Result<()> {
    // Function implementation goes here
    let input_format = infer_graph_format(&args.input, &args.from).ok_or_else(|| {
        anyhow::anyhow!(
            "Input graph format is not supported or couldn't be inferred: {}",
            args.input
        )
    })?;
    debug!("Input format: {:?}", input_format);
    let graph = CoreGraph::load_from_file(&args.input, input_format)?;
    // Here would be the logic to convert to de Bruijn graph
    // For now only print k-mers
    // TODO
    let lookup_graph = LookUpGraph::new(&graph);
    let kmers = lookup_graph.extract_canonical_kmers(args.kmer_size);
    println!("Extracted k-mers:");
    for (i, kmer) in kmers.iter().enumerate() {
        println!("{}.: {}", i, kmer.to_string());
    }
    println!("De Bruijn graph conversion is not yet implemented.");
    Ok(())
}
