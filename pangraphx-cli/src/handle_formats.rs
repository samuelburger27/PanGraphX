use anyhow::Result;
use colored::Colorize;
use pangraphx_core::GraphFormat;
use textwrap::{Options, fill};

pub fn handle_formats() -> Result<()> {
    println!("{}", "Supported Graph Formats:".bold().bright_cyan());
    println!("{}", format!("{:-<80}", "").dimmed()); // Horizontal separator

    for format in GraphFormat::iter() {
        let name = format.to_string(); // e.g., "GFA"
        let ext = format.get_extension(); // e.g., "gfa"
        let desc = format.get_description();

        // Bold the format name and extension for visual hierarchy
        println!(" * {} [.{}]", name.bright_green().bold(), ext.cyan());

        // Wrap the description at 70 characters and indent it (before coloring)
        let wrapped = fill(
            desc,
            Options::new(70)
                .initial_indent("       ")
                .subsequent_indent("       "),
        );

        // Highlight "Note" in yellow after wrapping for visibility
        let colored_desc = wrapped.replace("Note", &format!("{}", "Note".yellow().bold()));
        println!("{}", colored_desc);
        println!();
    }
    Ok(())
}
