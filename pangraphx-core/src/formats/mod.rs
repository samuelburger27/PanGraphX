pub(crate) mod common;
pub mod fastg_format;
pub mod gbz_format;
pub mod gfa;
pub mod odgi_format;
pub mod vg_format;

pub use fastg_format::FastgCodec;
pub use gbz_format::GBZCodec;
pub use gfa::GFACodec;
pub use odgi_format::ODGICodec;
pub use vg_format::VGCodec;
