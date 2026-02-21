pub mod core;
pub mod de_bruijn_conversion;
pub mod error;
pub mod formats;
#[cfg(test)]
pub mod test_helpers;
pub mod traits;

pub use core::{graph_dto::CoreGraphDTO, graph::CoreGraph};
pub use de_bruijn_conversion::{colored_dbg::ColoredDBG, de_bruijn_graph::DeBruijn, k_mers::Kmer};
pub use error::PanResult;
use std::fmt::Display;

use crate::formats::{FastgCodec, GBZCodec, GFACodec, VGCodec, ODGICodec};
use crate::traits::{GraphParser, GraphSerializer};
use std::io::{Read, Seek};

/// Supported graph formats
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphFormat {
    GBZ,
    GFA,
    VG,
    FASTG,
    ODGI,
}

impl Display for GraphFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GraphFormat::GBZ => write!(f, "GBZ"),
            GraphFormat::GFA => write!(f, "GFA"),
            GraphFormat::VG => write!(f, "VG"),
            GraphFormat::FASTG => write!(f, "FASTG"),
            GraphFormat::ODGI => write!(f, "ODGI"),
        }
    }
}

impl GraphFormat {
    pub fn iter() -> impl Iterator<Item = Self> {
        [Self::GBZ, Self::GFA, Self::VG, Self::FASTG, Self::ODGI]
            .iter()
            .copied()
    }

    pub fn get_extension(&self) -> &str {
        match self {
            GraphFormat::GBZ => "gbz",
            GraphFormat::GFA => "gfa",
            GraphFormat::VG => "vg",
            GraphFormat::FASTG => "fastg",
            GraphFormat::ODGI => "odgi",
        }
    }

    pub fn from_extension(extension: &str) -> Option<Self> {
        match extension.to_lowercase().as_str() {
            "gfa" => Some(GraphFormat::GFA),
            "vg" => Some(GraphFormat::VG),
            "gbz" => Some(GraphFormat::GBZ),
            "fastg" => Some(GraphFormat::FASTG),
            "odgi" => Some(GraphFormat::ODGI),
            _ => None,
        }
    }

    pub fn get_parser<R: Read + Seek>(&self) -> Box<dyn GraphParser<R>> {
        match self {
            GraphFormat::GFA => Box::new(GFACodec),
            GraphFormat::VG => Box::new(VGCodec),
            GraphFormat::GBZ => Box::new(GBZCodec),
            GraphFormat::FASTG => Box::new(FastgCodec),
            GraphFormat::ODGI => Box::new(ODGICodec),
        }
    }

    pub fn get_serializer(&self) -> Box<dyn GraphSerializer> {
        match self {
            GraphFormat::GFA => Box::new(GFACodec),
            GraphFormat::VG => Box::new(VGCodec),
            GraphFormat::GBZ => Box::new(GBZCodec),
            GraphFormat::FASTG => Box::new(FastgCodec),
            GraphFormat::ODGI => Box::new(ODGICodec),
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
