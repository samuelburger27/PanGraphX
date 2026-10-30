/// Generated protobuf types for the VG (Variation Graph) format.
pub mod vg {
    #![allow(clippy::derive_partial_eq_without_eq)]
    include!(concat!(env!("OUT_DIR"), "/vg.rs"));
}

/// The subset of [`vg`] the VG parser decodes (see `proto/vg_read.proto`).
pub mod vg_read {
    #![allow(clippy::derive_partial_eq_without_eq)]
    include!(concat!(env!("OUT_DIR"), "/vg_read.rs"));
}
