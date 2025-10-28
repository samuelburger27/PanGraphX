use crate::CoreGraph;
use crate::GraphFormat;
use crate::PanResult;
use std::io::{BufRead, BufReader};
use std::fs::File;

impl CoreGraph {
    pub fn load_from_file(path: &str, format: GraphFormat) -> PanResult<CoreGraph> {
        let file = File::open(path)?;
        let mut reader = BufReader::new(file);
        Self::load(& mut reader, format)
    }

    pub fn load(reader: & mut dyn BufRead, format: GraphFormat) -> PanResult<CoreGraph> {
        format.get_parser().parse(reader)
    }

    pub fn save_to_file(&self, path: &str, format: GraphFormat) -> PanResult<()> {
        let file = File::create(path)?;
        let mut writer = std::io::BufWriter::new(file);
        self.save(& mut writer, format)
    }

    pub fn save(&self, writer: & mut dyn std::io::Write, format: GraphFormat) -> PanResult<()> {
        format.get_serializer().serialize(self, writer)
    }
}
