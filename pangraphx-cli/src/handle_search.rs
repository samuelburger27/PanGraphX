use anyhow::{Context, Result};
use colored::Colorize;
use log::debug;
use pangraphx_core::{CoreGraph, CoreGraphDTO};

use crate::{cli::args_parser::SearchArgs, handle_convert::infer_graph_format};

pub fn handle_search(args: &SearchArgs) -> Result<()> {
    debug!("Arguments for search: {args:#?}");
    let format = infer_graph_format(&args.input, args.format.as_ref()).ok_or_else(|| {
        anyhow::anyhow!(
            "Input graph format is not supported or couldn't be inferred: {}",
            args.input
        )
    })?;

    let dto = CoreGraphDTO::load_from_file(&args.input, format).with_context(|| {
        format!(
            "failed to load '{}' as {}",
            args.input,
            format.to_string().to_uppercase()
        )
    })?;
    let graph = CoreGraph::new(dto);

    let hits = graph
        .search(args.query.as_bytes(), args.kmer_size)
        .with_context(|| "sequence search failed")?;

    if args.tsv {
        println!("node\torientation\toffset");
        for hit in &hits {
            println!(
                "{}\t{}\t{}",
                graph.get_node_name(hit.node_id),
                hit.orientation,
                hit.offset
            );
        }
        return Ok(());
    }

    println!(
        "{} {}",
        "📂".bright_cyan(),
        format!("Loading file: {}", args.input).bold()
    );
    println!("   {} {}", "Format:".dimmed(), format.to_string().green());
    println!(
        "{} {}",
        "🔍".bright_magenta(),
        format!("Searching for query: {}", args.query).bold()
    );
    println!();
    println!(
        "{} {}",
        "Hits:".bold().bright_cyan(),
        hits.len().to_string().bright_white().bold()
    );

    if !hits.is_empty() {
        println!("{}", format!("{:-<50}", "").dimmed());
        println!("   {:<14} {:<12} Offset", "Node", "Orientation");
        for hit in &hits {
            println!(
                "   {:<14} {:<12} {}",
                graph.get_node_name(hit.node_id).yellow(),
                hit.orientation.to_string().cyan(),
                hit.offset.to_string().bright_white().bold()
            );
        }
    }
    Ok(())
}
