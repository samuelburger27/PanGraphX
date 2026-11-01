//! Error helpers for the line-based text readers (GFA, FASTG).

use crate::error::PanGraphXError;
use std::borrow::Cow;

/// A parse error located at line `line_no` of a `format` file.
pub fn line_error(format: &str, line_no: usize, message: &str) -> PanGraphXError {
    PanGraphXError::Parse(format!("{format} line {line_no}: {message}"))
}

/// `bytes` as text for an error message, with invalid UTF-8 replaced.
pub fn lossy(bytes: &[u8]) -> Cow<'_, str> {
    String::from_utf8_lossy(bytes)
}
