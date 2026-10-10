use crate::cli::args_parser::{ExtractArgs, ExtractMode};
use crate::handle_convert::infer_graph_format;
use anyhow::{Context, Result};
use colored::Colorize;
use log::debug;
use pangraphx_core::{CoreGraph, CoreGraphDTO};

pub fn handle_extract(args: &ExtractArgs) -> Result<()> {
    debug!("Arguments for extract: {args:#?}");

    match &args.mode {
        ExtractMode::Path(args) => {
            let graph = load_graph(&args.io.input, args.io.from.as_ref())?;
            let extracted = graph
                .extract_path(args.path.as_bytes())
                .with_context(|| format!("failed to extract path '{}'", args.path))?;
            save_graph(&extracted, &args.io.output, args.io.to.as_ref())
        }
        ExtractMode::Neighborhood(args) => {
            let graph = load_graph(&args.io.input, args.io.from.as_ref())?;
            let node_id = graph
                .resolve_node_id(&args.node)
                .with_context(|| format!("failed to resolve node '{}'", args.node))?;
            let extracted = graph
                .extract_neighborhood(node_id, args.radius)
                .with_context(|| {
                    format!(
                        "failed to extract {}-hop neighborhood of node '{}'",
                        args.radius, args.node
                    )
                })?;
            save_graph(&extracted, &args.io.output, args.io.to.as_ref())
        }
        ExtractMode::Component(args) => {
            let graph = load_graph(&args.io.input, args.io.from.as_ref())?;
            let node_id = graph
                .resolve_node_id(&args.node)
                .with_context(|| format!("failed to resolve node '{}'", args.node))?;
            let extracted = graph.extract_component(node_id).with_context(|| {
                format!("failed to extract component containing '{}'", args.node)
            })?;
            save_graph(&extracted, &args.io.output, args.io.to.as_ref())
        }
    }
}

fn load_graph(input: &str, from: Option<&String>) -> Result<CoreGraph> {
    let input_format = infer_graph_format(input, from).ok_or_else(|| {
        anyhow::anyhow!("Input graph format is not supported or couldn't be inferred: {input}")
    })?;
    debug!("Input format: {input_format:?}");

    println!(
        "{} {}",
        "📂".bright_cyan(),
        format!("Loading graph from file: {input}").bold()
    );
    println!(
        "   {} {}",
        "Format:".dimmed(),
        input_format.to_string().green()
    );

    let dto = CoreGraphDTO::load_from_file(input, input_format).with_context(|| {
        format!(
            "failed to load '{input}' as {}",
            input_format.to_string().to_uppercase()
        )
    })?;
    println!(
        "{} {}",
        "✓".green().bold(),
        "Successfully loaded graph from file".green()
    );

    Ok(CoreGraph::new(dto))
}

fn save_graph(graph: &CoreGraphDTO, output: &str, to: Option<&String>) -> Result<()> {
    let output_format = infer_graph_format(output, to).ok_or_else(|| {
        anyhow::anyhow!("Output graph format is not supported or couldn't be inferred: {output}")
    })?;
    debug!("Output format: {output_format:?}");

    println!(
        "   {} {}",
        "Extracted:".dimmed(),
        format!(
            "{} nodes / {} edges / {} paths",
            graph.nodes.len(),
            graph.edges.len(),
            graph.paths.len()
        )
        .yellow()
    );

    println!(
        "{} {}",
        "📁".bright_cyan(),
        format!("Saving to file: {output}").bold()
    );
    println!(
        "   {} {}",
        "Format:".dimmed(),
        output_format.to_string().green()
    );

    graph.save_to_file(output, output_format).with_context(|| {
        format!(
            "failed to save '{output}' as {}",
            output_format.to_string().to_uppercase()
        )
    })?;
    println!(
        "{} {}",
        "✓".green().bold(),
        "Successfully saved extracted subgraph".green()
    );

    Ok(())
}
