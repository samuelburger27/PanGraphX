use clap::{Args, Parser, Subcommand, ValueEnum};
use pangraphx_core::GraphFormat;
use std::fmt;

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

    /// Number of worker threads [default: all cores]
    #[arg(
        long,
        global = true,
        display_order = 100,
        value_name = "N",
        value_parser = clap::builder::RangedU64ValueParser::<usize>::new().range(1..)
    )]
    pub threads: Option<usize>,

    #[arg(
        short = 'v',
        long,
        global = true,
        display_order = 101,
        conflicts_with = "quiet",
        help = "Log debug messages to stderr (RUST_LOG overrides)"
    )]
    pub verbose: bool,

    #[arg(
        short = 'q',
        long,
        global = true,
        display_order = 102,
        help = "Only log warnings and errors to stderr (RUST_LOG overrides)"
    )]
    pub quiet: bool,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Convert genome graphs between formats
    #[command(
        arg_required_else_help = true,
        after_help = "Examples:\n  pangraphx-cli convert graph.gfa -o graph.gbz\n  pangraphx-cli convert graph.dat -o graph.vg --from gfa\n  zcat graph.gfa.gz | pangraphx-cli convert - --from gfa -o - --to vg > graph.vg"
    )]
    Convert(ConvertArgs),

    /// Build a de Bruijn graph from a variation graph
    #[command(
        arg_required_else_help = true,
        after_help = "Output is GFA, or FASTA for unitigs (-u); other graph formats cannot store the\nk-1 overlaps of a de Bruijn graph.\n\nExamples:\n  pangraphx-cli dbg graph.vg -k 31 -o dbg.gfa\n  pangraphx-cli dbg graph.gfa -k 21 -c --compact -o unitigs.gfa\n  pangraphx-cli dbg graph.gfa -k 31 -u -o unitigs.fa"
    )]
    Dbg(DbgArgs),

    /// Show basic information about a graph file
    #[command(arg_required_else_help = true)]
    Info(InputArgs),

    /// Show detailed statistics about a graph file
    #[command(arg_required_else_help = true)]
    Stats(InputArgs),

    /// List supported graph formats
    #[command(visible_alias = "formats")]
    Format,
}

/// Graph to read; `-` reads standard input.
#[derive(Args, Debug)]
pub struct InputArgs {
    /// Input file, or - for stdin (format inferred from the extension)
    #[arg(value_name = "INPUT")]
    pub input: String,

    /// Input format (gfa, gbz, vg, fastg, og); required for stdin
    #[arg(short = 'f', long, alias = "format", value_name = "FORMAT", value_parser = parse_format)]
    pub from: Option<GraphFormat>,
}

/// Graph to write; `-` writes standard output.
#[derive(Args, Debug)]
pub struct OutputArgs {
    /// Output file, or - for stdout (format inferred from the extension)
    #[arg(short = 'o', long, value_name = "OUTPUT")]
    pub output: String,

    /// Output format (gfa, gbz, vg, fastg, og); required for stdout
    #[arg(long, value_name = "FORMAT", value_parser = parse_format)]
    pub to: Option<GraphFormat>,
}

#[derive(Args, Debug)]
pub struct ConvertArgs {
    #[command(flatten)]
    pub input: InputArgs,

    #[command(flatten)]
    pub output: OutputArgs,
}

/// Output formats of a de Bruijn graph.
#[derive(Clone, Copy, PartialEq, Eq, Debug, ValueEnum)]
pub enum DbgFormat {
    /// GFA, one segment per k-mer or unitig
    Gfa,
    /// FASTA, one record per unitig (needs --compact)
    Fasta,
}

impl fmt::Display for DbgFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Gfa => "GFA",
            Self::Fasta => "FASTA",
        })
    }
}

/// Where to write a de Bruijn graph; `-` writes standard output.
#[derive(Args, Debug)]
pub struct DbgOutputArgs {
    /// Output file, or - for stdout (.gfa, or .fa/.fasta/.fna for unitigs)
    #[arg(short = 'o', long, value_name = "OUTPUT")]
    pub output: String,

    /// Output format; required for stdout
    #[arg(long, value_name = "FORMAT", value_enum)]
    pub to: Option<DbgFormat>,
}

#[derive(Args, Debug)]
pub struct DbgArgs {
    #[command(flatten)]
    pub input: InputArgs,

    #[command(flatten)]
    pub output: DbgOutputArgs,

    /// k-mer size (1-63)
    #[arg(
        short = 'k',
        long,
        default_value_t = 31,
        value_parser = clap::builder::RangedU64ValueParser::<usize>::new().range(1..=63)
    )]
    pub kmer_size: usize,

    /// Colored DBG: keep every input path as a walk through the k-mers
    #[arg(short = 'c', long)]
    pub colored: bool,

    /// Use every walk of the bidirected graph, not only the paths
    #[arg(short = 't', long = "full-topology", conflicts_with = "colored")]
    pub full_topology: bool,

    /// Merge non-branching chains of k-mers into unitig nodes
    #[arg(short = 'u', long)]
    pub compact: bool,
}

/// Parses a format given by its extension (`og`) or its name (`odgi`), ignoring case.
fn parse_format(value: &str) -> Result<GraphFormat, String> {
    GraphFormat::from_extension(value)
        .ok()
        .or_else(|| GraphFormat::iter().find(|f| f.to_string().eq_ignore_ascii_case(value)))
        .ok_or_else(|| {
            let known: Vec<_> = GraphFormat::iter()
                .map(|f| f.get_extension().to_string())
                .collect();
            format!("unknown format (expected one of: {})", known.join(", "))
        })
}
