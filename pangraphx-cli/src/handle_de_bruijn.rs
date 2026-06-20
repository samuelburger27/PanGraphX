use super::handle_convert::infer_graph_format;
use crate::cli::args_parser::DeBruijnArgs;
use anyhow::{Context, Result};
use colored::Colorize;
use log::{debug, warn};
use pangraphx_core::{ColoredDBG, CoreGraphDTO, DeBruijn};

pub fn handle_de_bruijn(args: &DeBruijnArgs) -> Result<()> {
    let input_format = infer_graph_format(&args.input, args.from.as_ref()).ok_or_else(|| {
        anyhow::anyhow!(
            "Input graph format is not supported or couldn't be inferred: {}",
            args.input
        )
    })?;
    let output_format = infer_graph_format(&args.output, args.to.as_ref()).ok_or_else(|| {
        anyhow::anyhow!(
            "Output graph format is not supported or couldn't be inferred: {}",
            args.output
        )
    })?;

    debug!("Input format: {input_format:?}");
    debug!("Output format: {output_format:?}");

    println!(
        "{} {}",
        "📂".bright_cyan(),
        format!("Loading graph from file: {}", args.input).bold()
    );
    println!(
        "   {} {}",
        "Format:".dimmed(),
        input_format.to_string().green()
    );

    let graph = CoreGraphDTO::load_from_file(&args.input, input_format).with_context(|| {
        format!(
            "failed to load '{}' as {}",
            args.input,
            input_format.to_string().to_uppercase()
        )
    })?;
    println!(
        "{} {}",
        "✓".green().bold(),
        "Successfully loaded graph from file".green()
    );

    println!();
    println!(
        "{} {}",
        "🔄".bright_magenta(),
        "Converting to de Bruijn graph".bold()
    );
    println!(
        "   {} {}",
        "K-mer size:".dimmed(),
        args.kmer_size.to_string().cyan()
    );
    if args.colored {
        println!("   {} {}", "Mode:".dimmed(), "Colored".yellow());
    } else if args.full_topology {
        println!("   {} {}", "Mode:".dimmed(), "Full topology".yellow());
    } else {
        println!("   {} {}", "Mode:".dimmed(), "Standard".yellow());
    }

    let final_graph =
        create_converted_dbg_graph(graph, args.kmer_size, args.full_topology, args.colored);
    println!(
        "{} {}",
        "✓".green().bold(),
        "Successfully converted graph".green()
    );

    println!();
    println!(
        "{} {}",
        "📁".bright_cyan(),
        format!("Saving to file: {}", args.output).bold()
    );
    println!(
        "   {} {}",
        "Format:".dimmed(),
        output_format.to_string().green()
    );

    final_graph
        .save_to_file(&args.output, output_format)
        .with_context(|| {
            format!(
                "failed to save '{}' as {}",
                args.output,
                output_format.to_string().to_uppercase()
            )
        })?;
    println!(
        "{} {}",
        "✓".green().bold(),
        "Successfully saved de Bruijn graph".green()
    );
    Ok(())
}

/// Converts a directed graph into a de Bruijn graph with the specified k-mer size, optionally using full topology and/or coloring.
fn create_converted_dbg_graph(
    graph: CoreGraphDTO,
    kmer_size: usize,
    full_topology: bool,
    colored: bool,
) -> CoreGraphDTO {
    // Colored only makes sense with specified Paths
    if colored {
        if full_topology {
            warn!(
                "Colored de Bruijn graph with full topology is not supported. Ignoring full topology flag."
            );
        }
        ColoredDBG::from_directed_graph(graph, kmer_size).into()
    } else if full_topology {
        DeBruijn::from_directed_graph_full_topography(graph, kmer_size).into()
    } else {
        DeBruijn::from_directed_graph(graph, kmer_size).into()
    }
}
