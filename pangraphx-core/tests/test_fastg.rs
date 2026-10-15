//! FASTG round-trip tests are a TODO.
//!
//! FASTG does not carry path or overlap information, so a `GFA -> FASTG -> GFA`
//! round-trip is inherently lossy. Any test here should therefore compare node
//! sequences and edges only, not use `CoreGraphDTO::isomorphic`.
