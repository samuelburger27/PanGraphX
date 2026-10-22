mod cli;
mod graph_io;
mod handle_convert;
mod handle_dbg;
mod handle_formats;
mod handle_info;
mod handle_stats;
mod utils;

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, error::ErrorKind};
use cli::args_parser::{Cli, Commands};
use colored::Colorize;
use handle_convert::handle_convert;
use handle_dbg::handle_dbg;
use handle_formats::handle_formats;
use handle_info::handle_info;
use handle_stats::handle_stats;
use log::{Level, debug};
use std::io::{IsTerminal, Write};

fn main() {
    if let Err(err) = run() {
        // A closed downstream pipe (e.g. `| head`) is not an error.
        if is_broken_pipe(&err) {
            std::process::exit(0);
        }
        print_error(&err);
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = parse_or_exit();
    init_logging(&cli);

    if let Some(threads) = cli.threads {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build_global()
            .context("failed to set up the thread pool")?;
    }
    debug!("Using {} worker threads", rayon::current_num_threads());

    match &cli.command {
        Commands::Convert(args) => handle_convert(args),
        Commands::Dbg(args) => handle_dbg(args),
        Commands::Info(args) => handle_info(args),
        Commands::Stats(args) => handle_stats(args),
        Commands::Format => {
            handle_formats();
            Ok(())
        }
    }
}

/// Logs go to stderr, so stdout carries only data. Progress is logged at
/// `info`, shown by default; `RUST_LOG` overrides `-v` / `-q`.
fn init_logging(cli: &Cli) {
    let level = if cli.verbose {
        "debug"
    } else if cli.quiet {
        "warn"
    } else {
        "info"
    };
    let default = format!("warn,pangraphx_cli={level},pangraphx_core={level}");
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(default))
        .format(|buf, record| {
            if record.level() == Level::Info {
                return writeln!(buf, "{}", record.args());
            }
            let style = buf.default_level_style(record.level()).bold();
            let level = record.level().as_str().to_lowercase();
            writeln!(buf, "{style}{level}{style:#}: {}", record.args())
        })
        .init();
}

/// Parse arguments; on a usage error print the message followed by the help of
/// the subcommand the user was addressing, then exit with code 2.
fn parse_or_exit() -> Cli {
    match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => {
            if matches!(
                err.kind(),
                ErrorKind::DisplayHelp
                    | ErrorKind::DisplayVersion
                    | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
            ) {
                err.exit();
            }
            let _ = err.print();
            let mut cmd = Cli::command();
            cmd.build();
            let sub = std::env::args()
                .skip(1)
                .find_map(|arg| cmd.find_subcommand(&arg).cloned());
            let mut target = sub.unwrap_or(cmd);
            let help = target.render_help();
            // Keep clap's bold/underline styling, as for `--help`, when stderr is a terminal.
            if stderr_styled() {
                eprintln!("\n{}", help.ansi());
            } else {
                eprintln!("\n{help}");
            }
            std::process::exit(2);
        }
    }
}

fn stderr_styled() -> bool {
    std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none()
}

fn is_broken_pipe(err: &anyhow::Error) -> bool {
    err.chain()
        .filter_map(|cause| cause.downcast_ref::<std::io::Error>())
        .any(|io| io.kind() == std::io::ErrorKind::BrokenPipe)
}

fn print_error(err: &anyhow::Error) {
    // `colored` decides by stdout, but errors go to stderr.
    if !stderr_styled() {
        colored::control::set_override(false);
    }
    eprintln!("{} {}", "error:".red().bold(), err.to_string().bold());

    let mut causes = err.chain().skip(1).peekable();
    if causes.peek().is_some() {
        eprintln!("{}", "Caused by:".yellow().bold());
        for (idx, cause) in causes.enumerate() {
            eprintln!("  {} {}", format!("{}.", idx + 1).yellow(), cause);
        }
    }
    eprintln!(
        "{} {}",
        "hint:".cyan().bold(),
        "run with -v for diagnostic logs".dimmed()
    );
}
