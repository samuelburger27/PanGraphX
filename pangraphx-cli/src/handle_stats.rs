use crate::cli::args_parser::InputArgs;
use crate::graph_io::load_graph;
use crate::utils::group;
use anyhow::Result;
use colored::Colorize;
use pangraphx_core::{CoreGraph, GraphStats};

pub fn handle_stats(args: &InputArgs) -> Result<()> {
    let graph = CoreGraph::new(load_graph(args)?);
    print_graph_stats(&graph.compute_stats());
    Ok(())
}

fn print_graph_stats(stats: &GraphStats) {
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
