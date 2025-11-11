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

    /// Show basic info about a graph file
    Info(InfoArgs),

    /// Validate graph file structure and format
    Validate(ValidateArgs),

    /// List supported graph formats
    Formats,
}

#[derive(Args, Debug)]
pub struct ConvertArgs {
    /// Input file path (format inferred from suffix)
    #[arg(short = 'i', long)]
    pub input: String,

    /// Output file path (format inferred from suffix)
    #[arg(short = 'o', long)]
    pub output: String,

    /// Override input format (e.g., gfa, vg, dbg)
    #[arg(long)]
    pub from: Option<String>,

    /// Override output format (e.g., gfa, vg, dbg)
    #[arg(long)]
    pub to: Option<String>,

    /// Number of threads to use
    #[arg(short = 't', long, default_value_t = 1)]
    pub threads: usize,
}

#[derive(Args, Debug)]
pub struct InfoArgs {
    #[arg(help = "Graph file to inspect")]
    pub file: String,
}

#[derive(Args, Debug)]
pub struct ValidateArgs {
    #[arg(help = "Graph file to validate")]
    pub file: String,
}
