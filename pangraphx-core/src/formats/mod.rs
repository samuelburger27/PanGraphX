pub mod fastg_format;
pub mod gbz_format;
pub mod gfa_format;
pub mod odgi_format;
pub mod vg_format;

pub use fastg_format::FastgCodec;
pub use gbz_format::GBZCodec;
pub use gfa_format::GFACodec;
pub use vg_format::VGCodec;
pub use odgi_format::ODGICodec;