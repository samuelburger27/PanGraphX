pub mod core;
pub mod error;
pub mod formats;
pub mod traits;

pub use core::graph::CoreGraph;
pub use error::PanResult;

use crate::traits::{GraphParser, GraphSerializer};

/// Supported graph formats
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphFormat {
    GFA,
    VG,
}

impl GraphFormat {
    pub fn from_extension(extension: &str) -> Option<Self> {
        match extension.to_lowercase().as_str() {
            "gfa" => Some(GraphFormat::GFA),
            "vg" => Some(GraphFormat::VG),
            _ => None,
        }
    }

    pub fn get_parser(&self) -> Box<dyn GraphParser> {
        match self {
            GraphFormat::GFA => Box::new(formats::gfa_format::GFASerialization),
            GraphFormat::VG => unimplemented!("VG format parser not implemented yet"),
        }
    }

    pub fn get_serializer(&self) -> Box<dyn GraphSerializer> {
        match self {
            // TODO
            // GraphFormat::GFA => Box::new(formats::gfa_format::GFASerialization),
            GraphFormat::GFA => unimplemented!("GFA format serializer not implemented yet"),
            GraphFormat::VG => unimplemented!("VG format serializer not implemented yet"),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}
