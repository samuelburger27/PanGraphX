use crate::cli::args_parser::ConvertArgs;
use anyhow::{Context, Result};
use colored::Colorize;
use log::{debug, warn};
use pangraphx_core::{CoreGraphDTO, GraphFormat};
use std::path::Path;

pub fn handle_conversion(args: &ConvertArgs) -> Result<()> {
    // Handle conversion command
    debug!("Arguments for conversion: {:#?}", args);
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

    graph
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
        "Successfully converted graph and saved to file".green()
    );
    Ok(())
}

/// Infer graph format from specified format or file extension if no format specified
pub fn infer_graph_format(path: &str, specified_format: &Option<String>) -> Option<GraphFormat> {
    if let Some(ext) = specified_format {
        GraphFormat::from_extension(ext).ok()
    } else {
        let extension = Path::new(path)
            .extension()
            .and_then(|ext| ext.to_str())?
            .to_lowercase();
        if extension.is_empty() {
            warn!("File {} has no extension", path);
            return None;
        }
        GraphFormat::from_extension(&extension).ok()
    }
}
