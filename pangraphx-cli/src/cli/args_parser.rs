use clap::{Args, Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    version,
    about,
    subcommand_required = true,
    arg_required_else_help = true,
    long_about = None
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Convert genome graphs between formats
    #[command(
        arg_required_else_help = true,
        after_help = "Examples:\n  pangraphx-cli convert graph.gfa -o graph.gbz\n  pangraphx-cli convert graph.dat -o graph.vg --from gfa"
    )]
    Convert(ConvertArgs),

    /// Convert variation graph to De Bruijn graph
    #[command(
        arg_required_else_help = true,
        after_help = "Examples:\n  pangraphx-cli ddb graph.vg -k 31 -o dbg.gfa\n  pangraphx-cli ddb graph.gfa -k 21 -c --compact -o unitigs.gfa"
    )]
    Ddb(DeBruijnArgs),

    /// Show basic information about a graph file
    #[command(arg_required_else_help = true)]
    Info(InfoArgs),

    /// Show detailed statistics about a graph file
    #[command(arg_required_else_help = true)]
    Stats(StatsArgs),

    /// List supported graph formats
    #[command(visible_alias = "formats")]
    Format,
}

#[derive(Args, Debug)]
pub struct ConvertArgs {
    /// Input file path (format inferred from suffix)
    #[arg(value_name = "INPUT")]
    pub input: String,

    /// Output file path (format inferred from suffix)
    #[arg(short = 'o', long)]
    pub output: String,

    /// Override input format (e.g. gfa, gbz, fastg)
    #[arg(long)]
    pub from: Option<String>,

    /// Override output format (e.g. gfa, gbz, fastg)
    #[arg(long)]
    pub to: Option<String>,
}

#[derive(Args, Debug)]
pub struct InfoArgs {
    #[arg(help = "Graph file to inspect")]
    pub file: String,

    /// Override input format (e.g. gfa, gbz, fastg)
    #[arg(short = 'f', long)]
    pub format: Option<String>,
}

#[derive(Args, Debug)]
pub struct StatsArgs {
    #[arg(help = "Graph file to analyze")]
    pub file: String,

    /// Override input format (e.g. gfa, gbz, fastg)
    #[arg(short = 'f', long)]
    pub format: Option<String>,
}

#[derive(Args, Debug)]
pub struct DeBruijnArgs {
    /// k-mer size (1-63)
    #[arg(
        short = 'k',
        long,
        default_value_t = 31,
        value_parser = clap::builder::RangedU64ValueParser::<usize>::new().range(1..=63)
    )]
    pub kmer_size: usize,

    /// Colored DBG
    #[arg(short = 'c', long, default_value_t = false)]
    pub colored: bool,

    /// Use all topological walks to create edges ( otherwise use only haplotype paths)
    #[arg(short = 'f', long = "full-topology", default_value_t = false)]
    pub full_topology: bool,

    /// Merge non-branching chains of k-mers into unitig nodes
    #[arg(long, default_value_t = false)]
    pub compact: bool,

    /// Input file path (format inferred from suffix)
    #[arg(value_name = "INPUT")]
    pub input: String,

    /// Output file path (format inferred from suffix)
    #[arg(short = 'o', long)]
    pub output: String,

    /// Override input format (e.g. gfa, gbz, fastg)
    #[arg(long)]
    pub from: Option<String>,

    /// Override output format (e.g. gfa, gbz, fastg)
    #[arg(long)]
    pub to: Option<String>,
}
