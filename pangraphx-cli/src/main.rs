mod cli;
mod handle_convert;
mod handle_de_bruijn;
mod handle_formats;
mod handle_info;
mod handle_stats;
use anyhow::Result;
use clap::{CommandFactory, Parser, error::ErrorKind};
use cli::args_parser::{Cli, Commands};
use colored::Colorize;
use handle_convert::handle_conversion;
use handle_de_bruijn::handle_de_bruijn;
use handle_formats::handle_formats;
use handle_info::handle_info;
use handle_stats::handle_stats;

// Graph loading makes millions of small allocations (sequences, names, path
// steps). mimalloc is faster for this but holds on to more memory, hence opt-in.
#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

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

    let cli = parse_or_exit();

    match cli.command {
        Commands::Convert(args) => handle_conversion(&args),
        Commands::Ddb(args) => handle_de_bruijn(&args),
        Commands::Info(args) => handle_info(&args),
        Commands::Stats(args) => handle_stats(&args),
        Commands::Format => {
            handle_formats();
            Ok(())
        }
    }
}

/// Parse arguments; on a usage error print the message followed by the help of
/// the subcommand the user was addressing, then exit with code 2.
fn parse_or_exit() -> Cli {
    match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => {
            if matches!(
                err.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) {
                err.exit();
            }
            let _ = err.print();
            let mut cmd = Cli::command();
            cmd.build();
            let name = std::env::args().skip(1).find(|a| !a.starts_with('-'));
            let sub = name.and_then(|n| cmd.find_subcommand(&n).cloned());
            let mut target = sub.unwrap_or(cmd);
            eprintln!();
            let _ = target.write_help(&mut std::io::stderr());
            eprintln!();
            std::process::exit(2);
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
