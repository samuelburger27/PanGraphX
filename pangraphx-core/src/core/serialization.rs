use crate::CoreGraphDTO;
use crate::GraphFormat;
use crate::PanResult;
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, Write};

pub trait GraphReader: BufRead + Seek {}

impl<T: BufRead + Seek> GraphReader for T {}

impl CoreGraphDTO {
    /// # Errors
    /// Returns an error if the file cannot be opened or the graph cannot be parsed.
    pub fn load_from_file(path: &str, format: GraphFormat) -> PanResult<Self> {
        let file = File::open(path)?;
        let mut reader = BufReader::new(file);
        Self::load(&mut reader, format)
    }

    /// # Errors
    /// Returns an error if the graph cannot be parsed from the reader.
    pub fn load<R: GraphReader>(reader: &mut R, format: GraphFormat) -> PanResult<Self> {
        format.get_parser::<R>().parse(reader)
    }

    /// # Errors
    /// Returns an error if the file cannot be created or the graph cannot be serialized.
    pub fn save_to_file(&self, path: &str, format: GraphFormat) -> PanResult<()> {
        let file = File::create(path)?;
        let mut writer = std::io::BufWriter::new(file);
        self.save(&mut writer, format)
    }

    /// # Errors
    /// Returns an error if the graph cannot be serialized to the writer.
    pub fn save(&self, writer: &mut dyn Write, format: GraphFormat) -> PanResult<()> {
        format.get_serializer().serialize(self, writer)
    }
}
