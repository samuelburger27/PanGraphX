mod cli;

use anyhow::Result;
use clap::Parser;
use cli::args_parser::{Cli, Commands};

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Convert(args) => {
            // Handle convert command
            println!("Converting from {:?} to {:?}", args.from, args.to);

            
        }
        Commands::Info(args) => {
            // Handle info command
            println!("Getting info for {:?}", args.file);
        }
        Commands::Validate(args) => {
            // Handle validate command
            println!("Validating {:?}", args.file);
        }
        Commands::Formats => {
            // Handle formats command
            println!("Listing supported formats");
        }
    }

    Ok(())
}
