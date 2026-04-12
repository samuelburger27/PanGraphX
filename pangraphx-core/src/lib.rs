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
pub mod proto_gen;
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
#[cfg(feature = "odgi")]
use crate::formats::ODGICodec;
use crate::formats::{FastgCodec, GBZCodec, GFACodec, VGCodec};
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
    /// ODGI binary graph format (requires `odgi` feature, Linux only).
    #[cfg(feature = "odgi")]
    ODGI,
}

impl Display for GraphFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GraphFormat::GBZ => write!(f, "GBZ"),
            GraphFormat::GFA => write!(f, "GFA"),
            GraphFormat::VG => write!(f, "VG"),
            GraphFormat::FASTG => write!(f, "FASTG"),
            #[cfg(feature = "odgi")]
            GraphFormat::ODGI => write!(f, "ODGI"),
        }
    }
}

impl GraphFormat {
    /// Returns an iterator over all supported formats.
    pub fn iter() -> impl Iterator<Item = Self> {
        let base = [Self::GBZ, Self::GFA, Self::VG, Self::FASTG];
        #[cfg(feature = "odgi")]
        let extra = [Self::ODGI];
        #[cfg(not(feature = "odgi"))]
        let extra: [Self; 0] = [];
        base.into_iter().chain(extra)
    }

    /// Returns the conventional filename extension for this format.
    pub fn get_extension(&self) -> &str {
        match self {
            GraphFormat::GBZ => "gbz",
            GraphFormat::GFA => "gfa",
            GraphFormat::VG => "vg",
            GraphFormat::FASTG => "fastg",
            #[cfg(feature = "odgi")]
            GraphFormat::ODGI => "og",
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
            #[cfg(feature = "odgi")]
            "og" => Ok(GraphFormat::ODGI),
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
            #[cfg(feature = "odgi")]
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
            #[cfg(feature = "odgi")]
            GraphFormat::ODGI => Box::new(ODGICodec),
        }
    }

    pub fn get_description(&self) -> &str {
        match self {
            GraphFormat::GFA => {
                "Graphical Fragment Assembly (v1.0): An interoperable, text-based standard for representing pangenome nodes, edges, and paths."
            }
            GraphFormat::VG => {
                "VG Protobuf: A serialized binary format used by the vg toolkit; supports rich metadata but can be storage-intensive."
            }
            GraphFormat::GBZ => {
                "GBZ: A highly compressed, read-only format for large pangenomes. Note: Currently, only deserialization (loading) is supported."
            }
            GraphFormat::FASTG => {
                "FASTG: A text-based format designed to capture assembly ambiguities; largely superseded by GFA in modern workflows."
            }
            #[cfg(feature = "odgi")]
            GraphFormat::ODGI => {
                "ODGI: A dynamic, memory-efficient binary format optimized for large-scale pangenome analysis and graph manipulation."
            }
        }
    }
}
