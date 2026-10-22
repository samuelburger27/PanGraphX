use crate::cli::args_parser::ConvertArgs;
use crate::graph_io::{load_graph, output_format, save_graph};
use anyhow::Result;

pub fn handle_convert(args: &ConvertArgs) -> Result<()> {
    let format = output_format(&args.output)?;
    let graph = load_graph(&args.input)?;
    save_graph(&graph, &args.output, format)
}
