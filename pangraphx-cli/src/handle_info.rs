use anyhow::{Context, Result};
use colored::Colorize;
use log::debug;

use crate::{cli::args_parser::InfoArgs, handle_convert::infer_graph_format};

pub fn handle_info(args: &InfoArgs) -> Result<()> {
    debug!("Arguments for info: {args:#?}");
    // Handle info command
    let format = infer_graph_format(&args.file, args.format.as_ref()).ok_or_else(|| {
        anyhow::anyhow!(
            "Input graph format is not supported or couldn't be inferred: {}",
            args.file
        )
    })?;
    println!(
        "{} {}",
        "📂".bright_cyan(),
        format!("Loading file: {}", args.file).bold()
    );
    println!("   {} {}", "Format:".dimmed(), format.to_string().green());

    let graph =
        pangraphx_core::CoreGraphDTO::load_from_file(&args.file, format).with_context(|| {
            format!(
                "failed to load '{}' as {}",
                args.file,
                format.to_string().to_uppercase()
            )
        })?;
    println!(
        "{} {}",
        "✓".green().bold(),
        "Successfully loaded graph from file".green()
    );
    println!();
    println!("{}", "Graph Summary:".bold().bright_cyan());
    println!("{}", format!("{:-<50}", "").dimmed());

    println!(
        "   {} {}",
        "Nodes:".yellow(),
        graph.nodes.len().to_string().bright_white().bold()
    );
    println!(
        "   {} {}",
        "Edges:".yellow(),
        graph.edges.len().to_string().bright_white().bold()
    );
    println!(
        "   {} {}",
        "Paths:".yellow(),
        graph.paths.len().to_string().bright_white().bold()
    );

    if graph.node_name_map.is_some() {
        println!("   {} {}", "Node names:".cyan(), "Available".green());
    } else {
        println!(
            "   {} {}",
            "Node names:".cyan(),
            "Not available (node IDs only)".dimmed()
        );
    }
    Ok(())
}
