mod cli;
mod handle_convert;
mod handle_de_bruijn;
mod handle_formats;
mod handle_info;
use anyhow::Result;
use clap::Parser;
use cli::args_parser::{Cli, Commands};
use handle_convert::handle_conversion;
use handle_de_bruijn::handle_de_bruijn;
use handle_formats::handle_formats;
use handle_info::handle_info;

fn main() -> Result<()> {
    // production logging configuration
    // builder()
    //     .target(Target::Stderr)
    //     .format(|buf, record| {
    //         writeln!(buf, "{}", record.args())
    //     })
    //     .init();
    env_logger::init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Convert(args) => handle_conversion(&args),
        Commands::Ddb(args) => handle_de_bruijn(&args),
        Commands::Info(args) => handle_info(&args),
        Commands::Format => handle_formats(),
    }
}
