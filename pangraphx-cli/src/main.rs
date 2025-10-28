mod cli;
mod convert;
use anyhow::Result;
use clap::{Parser, error};
use cli::args_parser::{Cli, Commands};
use convert::handle_conversion;
use env_logger::{Target, builder};
use log::{debug, error, info, warn};
use pangraphx_core::{CoreGraph, GraphFormat, PanResult};
use std::io::Write;

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
            info!("Getting info for {:?}", args.file);
            Ok(())
        }
        Commands::Validate(args) => {
            // TODO
            // Handle validate command
            debug!("Arguments for validation: {:?}", args);
            info!("Validating {:?}", args.file);
            Ok(())
        }
        Commands::Formats => {
            // TODO
            // Handle formats command
            info!("Listing supported formats");
            Ok(())
        }
    }
}
