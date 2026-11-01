pub(crate) mod common;
pub mod fastg_format;
pub mod gbz;
pub mod gfa;
pub mod odgi;
pub mod vg;

pub use fastg_format::FastgCodec;
pub use gbz::GBZCodec;
pub use gfa::GFACodec;
pub use odgi::ODGICodec;
pub use vg::VGCodec;
