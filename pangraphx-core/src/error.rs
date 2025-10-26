use thiserror::Error;

#[derive(Error, Debug)]
pub enum PanGraphXError {
    #[error("I/O Error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Parse Error: {0}")]
    Parse(String),

    #[error("Format not supported")]
    UnsupportedFormat,
}

pub type PanResult<T> = std::result::Result<T, PanGraphXError>;

// Convert gfa parser errors into our library Parse error variant
impl From<gfa::parser::error::ParseError> for PanGraphXError {
    fn from(err: gfa::parser::error::ParseError) -> Self {
        PanGraphXError::Parse(err.to_string())
    }
}
