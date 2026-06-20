mod cli;
mod handle_convert;
mod handle_de_bruijn;
mod handle_formats;
mod handle_info;
use anyhow::Result;
use clap::Parser;
use cli::args_parser::{Cli, Commands};
use colored::Colorize;
use handle_convert::handle_conversion;
use handle_de_bruijn::handle_de_bruijn;
use handle_formats::handle_formats;
use handle_info::handle_info;

fn main() {
    if let Err(err) = run() {
        print_error(&err);
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
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
        Commands::Format => {
            handle_formats();
            Ok(())
        }
    }
}

fn print_error(err: &anyhow::Error) {
    eprintln!();
    eprintln!("{} {}", "Error:".red().bold(), err.to_string().bold());

    let mut causes = err.chain().skip(1).peekable();
    if causes.peek().is_some() {
        eprintln!("{}", "Caused by:".yellow().bold());
        for (idx, cause) in causes.enumerate() {
            eprintln!("  {} {}", format!("{}.", idx + 1).yellow(), cause);
        }
    }

    eprintln!();
    eprintln!(
        "{} {}",
        "Hint:".cyan().bold(),
        "run with RUST_LOG=debug for diagnostic logs".dimmed()
    );
}
