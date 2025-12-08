use crate::core::graph::CoreGraph;
use crate::error::PanResult;
use crate::traits::{GraphParser, GraphSerializer};
use std::io::{Read, Seek};

pub struct VGCodec;

impl<R: Read + Seek> GraphParser<R> for VGCodec {
    fn parse(&self, _reader: &mut R) -> PanResult<CoreGraph> {
        todo!("VG format parser not implemented yet")
    }
}

impl GraphSerializer for VGCodec {
    fn serialize(&self, _graph: &CoreGraph, _writer: &mut dyn std::io::Write) -> PanResult<()> {
        todo!("VG format serializer not implemented yet")
    }
}
