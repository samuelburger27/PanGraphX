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

    /// Extract a subgraph (path, neighborhood, or component)
    Extract(ExtractArgs),

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

#[derive(Args, Debug)]
pub struct ExtractArgs {
    #[command(subcommand)]
    pub mode: ExtractMode,
}

#[derive(Subcommand, Debug)]
pub enum ExtractMode {
    /// Extract a named path into a new graph
    Path(ExtractPathArgs),

    /// Extract the k-hop neighborhood of a node into a new graph
    Neighborhood(ExtractNeighborhoodArgs),

    /// Extract the connected component containing a node into a new graph
    Component(ExtractComponentArgs),
}

#[derive(Args, Debug)]
pub struct ExtractIoArgs {
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
pub struct ExtractPathArgs {
    /// Name of the path to extract
    #[arg(short = 'p', long)]
    pub path: String,

    #[command(flatten)]
    pub io: ExtractIoArgs,
}

#[derive(Args, Debug)]
pub struct ExtractNeighborhoodArgs {
    /// Node to center the neighborhood on (numeric ID or node name)
    #[arg(short = 'n', long)]
    pub node: String,

    /// Radius of the neighborhood in hops
    #[arg(short = 'r', long, default_value_t = 1)]
    pub radius: usize,

    #[command(flatten)]
    pub io: ExtractIoArgs,
}

#[derive(Args, Debug)]
pub struct ExtractComponentArgs {
    /// Node whose connected component to extract (numeric ID or node name)
    #[arg(short = 'n', long)]
    pub node: String,

    #[command(flatten)]
    pub io: ExtractIoArgs,
}
