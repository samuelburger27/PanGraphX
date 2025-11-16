pub mod core;
pub mod error;
pub mod formats;
pub mod traits;

pub use core::graph::CoreGraph;
pub use error::PanResult;
use std::fmt::Display;

use crate::formats::{GBZCodec, GFACodec, VGCodec, FastgCodec};
use crate::traits::{GraphParser, GraphSerializer};
use std::io::{Read, Seek};

/// Supported graph formats
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphFormat {
    GBZ,
    GFA,
    VG,
    FASTG,
}

impl Display for GraphFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GraphFormat::GBZ => write!(f, "GBZ"),
            GraphFormat::GFA => write!(f, "GFA"),
            GraphFormat::VG => write!(f, "VG"),
            GraphFormat::FASTG => write!(f, "FASTG"),
        }
    }
}

impl GraphFormat {
    pub fn iter() -> impl Iterator<Item = Self> {
        [Self::GFA, Self::VG, Self::GBZ].iter().copied()
    }

    pub fn get_extension(&self) -> &str {
        match self {
            GraphFormat::GBZ => "gbz",
            GraphFormat::GFA => "gfa",
            GraphFormat::VG => "vg",
            GraphFormat::FASTG => "fastg",
        }
    }

    pub fn from_extension(extension: &str) -> Option<Self> {
        match extension.to_lowercase().as_str() {
            "gfa" => Some(GraphFormat::GFA),
            "vg" => Some(GraphFormat::VG),
            "gbz" => Some(GraphFormat::GBZ),
            "fastg" => Some(GraphFormat::FASTG),
            _ => None,
        }
    }

    pub fn get_parser<R: Read + Seek>(&self) -> Box<dyn GraphParser<R>> {
        match self {
            GraphFormat::GFA => Box::new(GFACodec),
            GraphFormat::VG => Box::new(VGCodec),
            GraphFormat::GBZ => Box::new(GBZCodec),
            GraphFormat::FASTG => Box::new(FastgCodec),
        }
    }

    pub fn get_serializer(&self) -> Box<dyn GraphSerializer> {
        match self {
            GraphFormat::GFA => Box::new(GFACodec),
            GraphFormat::VG => Box::new(VGCodec),
            GraphFormat::GBZ => Box::new(GBZCodec),
            GraphFormat::FASTG => Box::new(FastgCodec),
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
