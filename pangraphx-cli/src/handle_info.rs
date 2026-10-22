use crate::cli::args_parser::InputArgs;
use crate::graph_io::load_graph;
use crate::utils::group;
use anyhow::Result;
use colored::Colorize;

pub fn handle_info(args: &InputArgs) -> Result<()> {
    let graph = load_graph(args)?;

    println!("{}", "Graph Summary:".bold().bright_cyan());
    println!("{}", format!("{:-<50}", "").dimmed());
    print_count("Nodes:", graph.nodes.len());
    print_count("Edges:", graph.edges.len());
    print_count("Paths:", graph.paths.len());
    let names = if graph.node_name_map.is_some() {
        "Available".green()
    } else {
        "Not available (node IDs only)".dimmed()
    };
    println!("   {} {names}", "Node names:".cyan());
    Ok(())
}

fn print_count(label: &str, count: usize) {
    println!(
        "   {} {}",
        label.yellow(),
        group(count).bright_white().bold()
    );
}
