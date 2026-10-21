mod binio;
pub mod fastg_format;
pub mod gbz_format;
pub mod gfa_format;
mod gfa_parser;
pub mod hash_graph_format;
mod id_map;
pub mod odgi_format;
pub mod packed_graph_format;
pub mod vg_format;

pub use fastg_format::FastgCodec;
pub use gbz_format::GBZCodec;
pub use gfa_format::GFACodec;
pub use odgi_format::ODGICodec;
pub use packed_graph_format::PackedGraphCodec;
pub use vg_format::VGCodec;
