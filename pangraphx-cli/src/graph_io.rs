//! Loading and saving graphs for the subcommands.
//!
//! A path of `-` means standard input or output. Parsers need a seekable
//! reader (the GFA parser reads its input twice), so standard input is first
//! copied to an anonymous temporary file; this keeps memory use the same as
//! for a file argument. The format of a stream cannot be inferred and must be
//! given with `--from` / `--to`.

use crate::cli::args_parser::{InputArgs, OutputArgs};
use crate::utils::group;
use anyhow::{Context, Result, anyhow, bail};
use log::{debug, info};
use pangraphx_core::{CoreGraphDTO, GraphFormat};
use std::io::{BufReader, BufWriter, Seek, Write};
use std::path::Path;
use std::time::Instant;

const STDIO: &str = "-";

/// Reads the graph named by `args`, logging progress.
pub fn load_graph(args: &InputArgs) -> Result<CoreGraphDTO> {
    let format = resolve_format(&args.input, args.from, "--from")?;
    let name = display_name(&args.input, "stdin");
    info!("Loading {name} ({format})");
    let start = Instant::now();

    let graph = if args.input == STDIO {
        let mut spool = tempfile::tempfile().context("failed to create a temporary file")?;
        let bytes = std::io::copy(&mut std::io::stdin().lock(), &mut spool)
            .context("failed to read standard input")?;
        spool.rewind()?;
        debug!("Buffered {bytes} bytes of stdin in a temporary file");
        CoreGraphDTO::load(&mut BufReader::new(spool), format)
    } else {
        CoreGraphDTO::load_from_file(&args.input, format)
    }
    .with_context(|| format!("failed to load {name} as {format}"))?;

    info!(
        "Loaded {} nodes, {} edges, {} paths in {:.2?}",
        group(graph.nodes.len()),
        group(graph.edges.len()),
        group(graph.paths.len()),
        start.elapsed()
    );
    Ok(graph)
}

/// Resolves the output format of `args` before any work is done, so that a bad
/// output path fails fast.
pub fn output_format(args: &OutputArgs) -> Result<GraphFormat> {
    resolve_format(&args.output, args.to, "--to")
}

/// Writes `graph` to the destination named by `args`, logging progress.
pub fn save_graph(graph: &CoreGraphDTO, args: &OutputArgs, format: GraphFormat) -> Result<()> {
    let name = display_name(&args.output, "stdout");
    info!("Writing {name} ({format})");
    let start = Instant::now();

    if args.output == STDIO {
        let mut writer = BufWriter::new(std::io::stdout().lock());
        graph
            .save(&mut writer, format)
            .map_err(anyhow::Error::from)
            .and_then(|()| writer.flush().map_err(anyhow::Error::from))
    } else {
        graph
            .save_to_file(&args.output, format)
            .map_err(anyhow::Error::from)
    }
    .with_context(|| format!("failed to write {name} as {format}"))?;

    info!("Wrote {name} in {:.2?}", start.elapsed());
    Ok(())
}

/// Uses the explicit format if given, otherwise the file extension.
fn resolve_format(path: &str, explicit: Option<GraphFormat>, flag: &str) -> Result<GraphFormat> {
    if let Some(format) = explicit {
        return Ok(format);
    }
    if path == STDIO {
        bail!("the format of a stream cannot be inferred; pass {flag}");
    }
    let extension = Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .ok_or_else(|| anyhow!("'{path}' has no file extension; pass {flag}"))?;
    GraphFormat::from_extension(extension)
        .map_err(|_| anyhow!("unknown extension '.{extension}' of '{path}'; pass {flag}"))
}

fn display_name(path: &str, stream: &str) -> String {
    if path == STDIO {
        stream.to_string()
    } else {
        format!("'{path}'")
    }
}
