//! Core library for `PanGraphX`.
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
//!     let dbg = DeBruijn::from_directed_graph(graph, 31)?;
//!     // One node per k-mer, or `dbg.into_compacted_dto()?` for unitigs.
//!     let _as_core = CoreGraphDTO::try_from(dbg)?;
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
pub mod traits;

/// Common graph primitive types used throughout the crate.
pub use core::core_types::{
    Edge, Node, NodeId, NodeName, Orientation, Path, PathName, Sequence, Step,
};
/// Graph statistics summary types produced by [`CoreGraph::compute_stats`].
pub use core::stats::{DegreeStats, GraphStats, LenStats, PathStats};
/// Internal graph model with efficient lookup and manipulation utilities.
pub use core::{
    graph::CoreGraph,
    graph_dto::{CoreGraphDTO, GraphParts},
};
/// de Bruijn graph data structures and helpers.
pub use de_bruijn_conversion::{
    UnitigSequences, colored_dbg::ColoredDBG, de_bruijn_graph::DeBruijn, k_mers::Kmer,
};
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
            Self::GBZ => write!(f, "GBZ"),
            Self::GFA => write!(f, "GFA"),
            Self::VG => write!(f, "VG"),
            Self::FASTG => write!(f, "FASTG"),
            Self::ODGI => write!(f, "ODGI"),
        }
    }
}

impl GraphFormat {
    /// Returns an iterator over all supported formats.
    pub fn iter() -> impl Iterator<Item = Self> {
        [Self::GFA, Self::GBZ, Self::VG, Self::FASTG, Self::ODGI].into_iter()
    }

    /// Returns the conventional filename extension for this format.
    #[must_use]
    pub const fn get_extension(&self) -> &str {
        match self {
            Self::GBZ => "gbz",
            Self::GFA => "gfa",
            Self::VG => "vg",
            Self::FASTG => "fastg",
            Self::ODGI => "og",
        }
    }

    /// Resolves a format from a filename extension.
    ///
    /// Matching is case-insensitive.
    ///
    /// # Errors
    /// Returns an error if the extension does not correspond to a supported format.
    pub fn from_extension(extension: &str) -> PanResult<Self> {
        match extension.to_lowercase().as_str() {
            "gfa" => Ok(Self::GFA),
            "vg" => Ok(Self::VG),
            "gbz" => Ok(Self::GBZ),
            "fastg" => Ok(Self::FASTG),
            "og" => Ok(Self::ODGI),
            _ => Err(PanGraphXError::UnsupportedFormat),
        }
    }

    /// Returns a parser implementation for the selected format.
    #[must_use]
    pub fn get_parser<R: Read + Seek>(&self) -> Box<dyn GraphParser<R>> {
        match self {
            Self::GFA => Box::new(GFACodec),
            Self::VG => Box::new(VGCodec),
            Self::GBZ => Box::new(GBZCodec),
            Self::FASTG => Box::new(FastgCodec),
            Self::ODGI => Box::new(ODGICodec),
        }
    }

    /// Returns a serializer implementation for the selected format.
    #[must_use]
    pub fn get_serializer(&self) -> Box<dyn GraphSerializer> {
        match self {
            Self::GFA => Box::new(GFACodec),
            Self::VG => Box::new(VGCodec),
            Self::GBZ => Box::new(GBZCodec),
            Self::FASTG => Box::new(FastgCodec),
            Self::ODGI => Box::new(ODGICodec),
        }
    }

    #[must_use]
    pub const fn get_description(&self) -> &str {
        match self {
            Self::GFA => {
                "Graphical Fragment Assembly (v1.0/v1.1): An interoperable, text-based standard for representing pangenome nodes, edges, and paths."
            }
            Self::VG => {
                "VG Protobuf: A serialized binary format used by the vg toolkit; supports rich metadata but can be storage-intensive."
            }
            Self::GBZ => "GBZ: A highly compressed format for large pangenomes.",
            Self::FASTG => {
                "FASTG: A text-based format designed to capture assembly ambiguities; largely superseded by GFA in modern workflows."
            }
            Self::ODGI => {
                "ODGI: A dynamic, memory-efficient binary format optimized for large-scale pangenome analysis and graph manipulation."
            }
        }
    }
}
