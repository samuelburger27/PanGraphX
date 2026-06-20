use thiserror::Error;

#[derive(Error, Debug)]
pub enum PanGraphXError {
    #[error("I/O Error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Parse Error: {0}")]
    Parse(String),

    #[error("Serialization Error: {0}")]
    Serialize(String),

    #[error("Format not supported")]
    UnsupportedFormat,

    #[error("Other Error: {0}")]
    Other(String),
}

pub type PanResult<T> = std::result::Result<T, PanGraphXError>;

// Convert gfa parser errors into our library Parse error variant
impl From<gfa::parser::error::ParseError> for PanGraphXError {
    fn from(err: gfa::parser::error::ParseError) -> Self {
        Self::Parse(err.to_string())
    }
}
