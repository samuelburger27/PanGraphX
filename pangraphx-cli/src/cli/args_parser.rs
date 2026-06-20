use clap::{Args, Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(version, about, subcommand_required = true, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Convert genome graphs between formats
    Convert(ConvertArgs),

    /// Convert variation graph to De Bruijn graph
    Ddb(DeBruijnArgs),

    /// Show basic information about a graph file
    Info(InfoArgs),

    /// Show detailed statistics about a graph file
    Stats(StatsArgs),

    /// List supported graph formats
    Format,
}

#[derive(Args, Debug)]
pub struct ConvertArgs {
    /// Input file path (format inferred from suffix)
    #[arg(short = 'i', long)]
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
    /// k-mer size
    #[arg(short = 'k', long, default_value_t = 31)]
    pub kmer_size: usize,

    /// Colored DBG
    #[arg(short = 'c', long, default_value_t = false)]
    pub colored: bool,

    // Use all topological walks to create edges ( otherwise use only haplotype paths)
    #[arg(short = 'f', long = "full-topology", default_value_t = false)]
    pub full_topology: bool,

    /// Input file path (format inferred from suffix)
    #[arg(short = 'i', long)]
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
