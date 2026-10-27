use crate::cli::args_parser::{DbgArgs, DbgFormat};
use crate::graph_io::{file_extension, load_graph, write_output};
use crate::utils::group;
use anyhow::{Context, Result, bail};
use log::info;
use pangraphx_core::{ColoredDBG, CoreGraphDTO, DeBruijn, GraphFormat};
use std::time::Instant;

/// The constructed graph, in the form it is written from.
enum Built {
    /// One node per k-mer, written from the k-mer tables (or compacted into
    /// FASTA unitigs while writing).
    Kmers(DeBruijn),
    /// One node per k-mer, with the input paths walked again while writing.
    Colored(ColoredDBG),
    /// Unitigs; small enough to hold as a DTO.
    Unitigs(CoreGraphDTO),
}

pub fn handle_dbg(args: &DbgArgs) -> Result<()> {
    let format = output_format(args)?;
    let graph = load_graph(&args.input)?;

    let mode = if args.colored {
        "colored"
    } else if args.full_topology {
        "full topology"
    } else {
        "paths"
    };
    let nodes = if args.compact { "unitigs" } else { "k-mers" };
    info!(
        "Building de Bruijn graph (k = {}, mode: {mode}, nodes: {nodes})",
        args.kmer_size
    );
    let start = Instant::now();
    let built = build(graph, args, format).context("failed to build the de Bruijn graph")?;
    let (node_count, edge_count) = match &built {
        Built::Kmers(dbg) => (dbg.node_count(), dbg.edge_count()),
        Built::Colored(dbg) => (dbg.dbg.node_count(), dbg.dbg.edge_count()),
        Built::Unitigs(dto) => (dto.nodes.len(), dto.edges.len()),
    };
    info!(
        "Built {} nodes, {} edges in {:.2?}",
        group(node_count),
        group(edge_count),
        start.elapsed()
    );

    write_output(&args.output.output, format, |writer| match built {
        Built::Kmers(dbg) if format == DbgFormat::Fasta => dbg.write_unitigs_fasta(writer),
        Built::Kmers(dbg) => dbg.write_gfa(writer),
        Built::Colored(dbg) => dbg.write_gfa(writer),
        Built::Unitigs(dto) => dto.save(writer, GraphFormat::GFA),
    })
}

/// Resolves and checks the output format before any work is done.
///
/// Only GFA can store the `k - 1` overlaps between the nodes of a de Bruijn
/// graph, so the other graph formats are rejected; FASTA holds unitig
/// sequences only.
fn output_format(args: &DbgArgs) -> Result<DbgFormat> {
    let output = &args.output;
    let format = if let Some(format) = output.to {
        format
    } else {
        let extension = file_extension(&output.output, "--to")?;
        match extension.to_ascii_lowercase().as_str() {
            "gfa" => DbgFormat::Gfa,
            "fa" | "fasta" | "fna" => DbgFormat::Fasta,
            _ if GraphFormat::from_extension(extension).is_ok() => bail!(
                "de Bruijn graphs have k-1 overlaps that only GFA can store; \
                 write .gfa (or .fa for unitigs with --compact)"
            ),
            _ => bail!(
                "unknown extension '.{extension}' of '{}'; pass --to",
                output.output
            ),
        }
    };
    if format == DbgFormat::Fasta {
        if args.colored {
            bail!("FASTA cannot hold the paths of a colored graph; write GFA instead");
        }
        if !args.compact {
            bail!("FASTA output holds unitigs; add -u/--compact");
        }
    }
    Ok(format)
}

/// Builds the de Bruijn graph of `graph` in the mode selected by `args`.
fn build(graph: CoreGraphDTO, args: &DbgArgs, format: DbgFormat) -> Result<Built> {
    let k = args.kmer_size;
    if args.colored {
        let dbg = ColoredDBG::from_directed_graph(graph, k)?;
        return Ok(if args.compact {
            Built::Unitigs(dbg.into_compacted_dto()?)
        } else {
            Built::Colored(dbg)
        });
    }
    let dbg = if args.full_topology {
        DeBruijn::from_directed_graph_full_topology(graph, k)?
    } else {
        DeBruijn::from_directed_graph(graph, k)?
    };
    // FASTA unitigs are compacted while writing.
    Ok(if args.compact && format == DbgFormat::Gfa {
        Built::Unitigs(dbg.into_compacted_dto()?)
    } else {
        Built::Kmers(dbg)
    })
}
