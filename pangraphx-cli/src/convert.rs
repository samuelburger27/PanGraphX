use crate::cli::args_parser::{Cli, Commands, ConvertArgs};
use anyhow::{Ok, Result};
use env_logger::{Target, builder};
use log::{debug, error, info, warn};
use pangraphx_core::{CoreGraph, GraphFormat, PanResult};
use std::{io::BufReader, path::Path};

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
    let graph = CoreGraph::load_from_file(&args.input, input_format)?;
    graph.save_to_file(&args.output, output_format)?;
    Ok(())
}

/// Infer graph format from specified format or file extension if no format specified
fn infer_graph_format(path: &str, specified_format: &Option<String>) -> Option<GraphFormat> {
    if let Some(ext) = specified_format {
        GraphFormat::from_extension(&ext)
    } else {
        let extension = Path::new(path)
            .extension()
            .and_then(|ext| ext.to_str())?
            .to_lowercase();
        if extension.is_empty() {
            warn!("File {} has no extension", path);
            return None;
        }
        GraphFormat::from_extension(&extension)
    }
}
