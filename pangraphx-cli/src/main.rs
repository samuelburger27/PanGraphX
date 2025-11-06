mod cli;
mod convert;
mod handle_formats;
mod handle_info;
use anyhow::Result;
use clap::Parser;
use cli::args_parser::{Cli, Commands};
use convert::handle_conversion;
use handle_formats::handle_formats;
use log::{debug};

fn main() -> Result<()> {
    // TODO production logging configuration
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
        Commands::Formats => handle_formats(),
    }
}
