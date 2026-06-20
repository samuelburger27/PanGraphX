use anyhow::{Context, Result};
use colored::Colorize;
use log::debug;
use pangraphx_core::{CoreGraph, CoreGraphDTO, GraphStats};

use crate::{cli::args_parser::StatsArgs, handle_convert::infer_graph_format};

pub fn handle_stats(args: &StatsArgs) -> Result<()> {
    debug!("Arguments for stats: {args:#?}");
    let format = infer_graph_format(&args.file, args.format.as_ref()).ok_or_else(|| {
        anyhow::anyhow!(
            "Input graph format is not supported or couldn't be inferred: {}",
            args.file
        )
    })?;
    println!(
        "{} {}",
        "📂".bright_cyan(),
        format!("Loading file: {}", args.file).bold()
    );
    println!("   {} {}", "Format:".dimmed(), format.to_string().green());

    let dto = CoreGraphDTO::load_from_file(&args.file, format).with_context(|| {
        format!(
            "failed to load '{}' as {}",
            args.file,
            format.to_string().to_uppercase()
        )
    })?;
    let graph = CoreGraph::new(dto);
    let stats = graph.compute_stats();

    print_graph_stats(&stats);
    Ok(())
}

fn print_graph_stats(stats: &GraphStats) {
    println!();
    println!("{}", "Graph Statistics:".bold().bright_cyan());
    println!("{}", format!("{:-<50}", "").dimmed());

    print_section("Sequence");
    print_stat("Nodes:", &group(stats.node_count));
    print_stat(
        "Total length:",
        &format!("{} bp", group(stats.node_len.total)),
    );
    print_stat(
        "Node length:",
        &format!(
            "min {} / avg {:.1} / max {} bp",
            group(stats.node_len.min),
            stats.node_len.mean,
            group(stats.node_len.max)
        ),
    );
    print_stat("N50:", &format!("{} bp", group(stats.node_len.n50)));

    print_section("Topology");
    print_stat("Edges:", &group(stats.edge_count));
    print_stat(
        "Components:",
        &format!(
            "{}   (largest {} nodes)",
            group(stats.component_count),
            group(stats.largest_component)
        ),
    );
    print_stat("Isolated nodes:", &group(stats.isolated_nodes));
    print_stat(
        "In-degree:",
        &format!(
            "min {} / avg {:.2} / max {}",
            stats.in_degree.min, stats.in_degree.mean, stats.in_degree.max
        ),
    );
    print_stat(
        "Out-degree:",
        &format!(
            "min {} / avg {:.2} / max {}",
            stats.out_degree.min, stats.out_degree.mean, stats.out_degree.max
        ),
    );
    print_stat(
        "Degree histogram:",
        &format_degree_hist(&stats.total_degree_hist),
    );

    print_section("Paths");
    print_stat("Paths:", &group(stats.path_count));
    if stats.path_len.count > 0 {
        print_stat(
            "Path length:",
            &format!(
                "min {} / avg {:.1} / max {} bp   (total {} bp)",
                group(stats.path_len.bp_min),
                stats.path_len.bp_mean,
                group(stats.path_len.bp_max),
                group(stats.path_len.bp_total)
            ),
        );
        print_stat(
            "Path steps:",
            &format!(
                "min {} / avg {:.1} / max {}",
                group(stats.path_len.steps_min),
                stats.path_len.steps_mean,
                group(stats.path_len.steps_max)
            ),
        );
    }
}

fn print_section(title: &str) {
    println!(" {}", title.bold().bright_cyan());
}

fn print_stat(label: &str, value: &str) {
    println!(
        "   {} {}",
        format!("{label:<18}").yellow(),
        value.bright_white().bold()
    );
}

/// Formats a number with thousands separators, e.g. `45310` -> `45,310`.
fn group(n: usize) -> String {
    let digits = n.to_string();
    let len = digits.len();
    let mut out = String::with_capacity(len + len / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// Renders the total-degree histogram, grouping all degrees `>= CAP` into one bucket
/// to keep the line readable for high-degree graphs.
fn format_degree_hist(hist: &[(usize, usize)]) -> String {
    const CAP: usize = 10;
    let mut parts: Vec<String> = Vec::new();
    let mut overflow = 0usize;
    for &(degree, count) in hist {
        if degree < CAP {
            parts.push(format!("{degree}:{count}"));
        } else {
            overflow += count;
        }
    }
    if overflow > 0 {
        parts.push(format!("{CAP}+:{overflow}"));
    }
    if parts.is_empty() {
        "-".to_string()
    } else {
        parts.join("  ")
    }
}
