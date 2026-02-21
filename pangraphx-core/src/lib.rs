//! Core library for PanGraphX.
//!
//! `pangraphx-core` provides a unified API for parsing, converting, and manipulating
//! pangenome graph formats.
//!
//! # What this crate provides
//!
//! - A common graph DTO: [`CoreGraphDTO`]
//! - A mutable graph type with lookup/manipulation helpers: [`CoreGraph`]
//! - Format dispatch through [`GraphFormat`]
//! - Parser/serializer traits: [`traits::GraphParser`] and [`traits::GraphSerializer`]
//! - de Bruijn graph utilities: [`DeBruijn`] and related types
//!
//! # Supported formats
//!
//! See [`GraphFormat`] for currently supported formats and extension mapping.
//!
//! # Quick start
//!
//! Parse a graph and serialize it to another format:
//!
//! ```no_run
//! use std::fs::File;
//! use pangraphx_core::GraphFormat;
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let mut input = File::open("graph.gfa")?;
//!     let parser = GraphFormat::GFA.get_parser();
//!     let graph = parser.parse(&mut input)?;
//!
//!     let serializer = GraphFormat::GBZ.get_serializer();
//!     let mut output = File::create("graph.gbz")?;
//!     serializer.serialize(&graph, &mut output)?;
//!     Ok(())
//! }
//! ```
//!
//! Build a de Bruijn graph from a parsed directed graph:
//!
//! ```no_run
//! use std::fs::File;
//! use pangraphx_core::{CoreGraphDTO, DeBruijn, GraphFormat};
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let mut input = File::open("graph.vg")?;
//!     let parser = GraphFormat::VG.get_parser();
//!     let graph: CoreGraphDTO = parser.parse(&mut input)?;
//!
//!     let dbg = DeBruijn::from_directed_graph(graph, 31);
//!     let _as_core: CoreGraphDTO = dbg.into();
//!     Ok(())
//! }
//! ```
//!
//! # Errors
//!
//! Library APIs typically return [`PanResult<T>`](PanResult), an alias around
//! [`PanGraphXError`]

pub mod core;
pub mod de_bruijn_conversion;
pub mod error;
pub mod formats;
#[cfg(test)]
pub mod test_helpers;
pub mod traits;

/// Common graph primitive types used throughout the crate.
pub use core::core_types::{
    Edge, Node, NodeId, NodeName, Orientation, Path, PathName, Sequence, Step,
};
/// Internal graph model with efficient lookup and manipulation utilities.
pub use core::{graph::CoreGraph, graph_dto::CoreGraphDTO};
/// de Bruijn graph data structures and helpers.
pub use de_bruijn_conversion::{colored_dbg::ColoredDBG, de_bruijn_graph::DeBruijn, k_mers::Kmer};
/// Standard result type for public APIs in this crate.
pub use error::PanResult;
use std::fmt::Display;

use crate::error::PanGraphXError;
use crate::formats::{FastgCodec, GBZCodec, GFACodec, ODGICodec, VGCodec};
use crate::traits::{GraphParser, GraphSerializer};
use std::io::{Read, Seek};

/// Supported graph formats
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphFormat {
    /// GBZ compressed graph format.
    GBZ,
    /// Graphical Fragment Assembly format.
    GFA,
    /// Variation Graph format.
    VG,
    /// FASTG graph format.
    FASTG,
    /// ODGI binary graph format.
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
    /// Returns an iterator over all supported formats.
    pub fn iter() -> impl Iterator<Item = Self> {
        [Self::GBZ, Self::GFA, Self::VG, Self::FASTG, Self::ODGI]
            .iter()
            .copied()
    }

    /// Returns the conventional filename extension for this format.
    pub fn get_extension(&self) -> &str {
        match self {
            GraphFormat::GBZ => "gbz",
            GraphFormat::GFA => "gfa",
            GraphFormat::VG => "vg",
            GraphFormat::FASTG => "fastg",
            GraphFormat::ODGI => "odgi",
        }
    }

    /// Resolves a format from a filename extension.
    ///
    /// Matching is case-insensitive.
    pub fn from_extension(extension: &str) -> PanResult<Self> {
        match extension.to_lowercase().as_str() {
            "gfa" => Ok(GraphFormat::GFA),
            "vg" => Ok(GraphFormat::VG),
            "gbz" => Ok(GraphFormat::GBZ),
            "fastg" => Ok(GraphFormat::FASTG),
            "odgi" => Ok(GraphFormat::ODGI),
            _ => Err(PanGraphXError::UnsupportedFormat),
        }
    }

    /// Returns a parser implementation for the selected format.
    pub fn get_parser<R: Read + Seek>(&self) -> Box<dyn GraphParser<R>> {
        match self {
            GraphFormat::GFA => Box::new(GFACodec),
            GraphFormat::VG => Box::new(VGCodec),
            GraphFormat::GBZ => Box::new(GBZCodec),
            GraphFormat::FASTG => Box::new(FastgCodec),
            GraphFormat::ODGI => Box::new(ODGICodec),
        }
    }

    /// Returns a serializer implementation for the selected format.
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
