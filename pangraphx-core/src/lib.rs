pub mod core;
pub mod error;
pub mod formats;
pub mod traits;

pub use core::graph::CoreGraph;
pub use error::PanResult;
use std::fmt::Display;

use crate::traits::{GraphParser, GraphSerializer};

/// Supported graph formats
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphFormat {
    GFA,
    VG,
}

impl Display for GraphFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GraphFormat::GFA => write!(f, "GFA"),
            GraphFormat::VG => write!(f, "VG"),
        }
    }
}

impl GraphFormat {
    pub fn iter() -> impl Iterator<Item = Self> {
        [Self::GFA, Self::VG].iter().copied()
    }

    pub fn get_extension(&self) -> &str {
        match self {
            GraphFormat::GFA => "gfa",
            GraphFormat::VG => "vg",
        }
    }

    pub fn from_extension(extension: &str) -> Option<Self> {
        match extension.to_lowercase().as_str() {
            "gfa" => Some(GraphFormat::GFA),
            "vg" => Some(GraphFormat::VG),
            _ => None,
        }
    }

    pub fn get_parser(&self) -> Box<dyn GraphParser> {
        match self {
            GraphFormat::GFA => Box::new(formats::gfa_format::GFACodec),
            GraphFormat::VG => unimplemented!("VG format parser not implemented yet"),
        }
    }

    pub fn get_serializer(&self) -> Box<dyn GraphSerializer> {
        match self {
            // TODO
            // GraphFormat::GFA => Box::new(formats::gfa_format::GFASerialization),
            GraphFormat::GFA => Box::new(formats::gfa_format::GFACodec),
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
