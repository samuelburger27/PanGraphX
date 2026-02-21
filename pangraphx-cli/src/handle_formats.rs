use anyhow::{Ok, Result};
use pangraphx_core::GraphFormat;

pub fn handle_formats() -> Result<()> {
    println!("Supported formats: ");
    for format in GraphFormat::iter() {
        println!("\t{} format, file extension: {}", format, format.get_extension());
    }
    Ok(())
}
