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
use log::debug;

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
        Commands::Info(args) => {
            // TODO
            debug!("Arguments for info: {:?}", args);
            // Handle info command
            println!("Getting info for {:?}", args.file);
            todo!("TODO");
        }
        Commands::Validate(args) => {
            // TODO
            // Handle validate command
            debug!("Arguments for validation: {:?}", args);
            println!("Validating {:?}", args.file);
            todo!("TODO");
        }
        Commands::Format => handle_formats(),
    }
}
