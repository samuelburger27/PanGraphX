use crate::core::core_types::{Edge, Node, NodeId, NodeName, Nodes, Orientation, Path, Step};
use crate::core::graph_dto::CoreGraphDTO;
use crate::error::{PanGraphXError, PanResult};
use crate::traits::{GraphParser, GraphSerializer};
use bstr::io::BufReadExt;
use gfa::cigar::CIGAR;
use gfa::gfa::GFA;
use gfa::gfa::orientation::Orientation as GFAOrientation;
use gfa::parser::GFAParserBuilder;
use gfa::parser::error::ParserTolerance;
use log::debug;
use rayon::prelude::*;
use std::borrow::Cow;
use std::collections::HashMap;
use std::io::{BufReader, Read, Seek, Write};

pub struct GFACodec;

impl From<GFAOrientation> for Orientation {
    fn from(orientation: GFAOrientation) -> Self {
        match orientation {
            GFAOrientation::Forward => Self::Forward,
            GFAOrientation::Backward => Self::Reverse,
        }
    }
}
/// Converts CIGAR to overlap length
/// For now ignore everything except matches
#[inline]
fn cigar_to_overlap(cigar: CIGAR) -> u32 {
    let mut result: u32 = 0;
    for pair in cigar.0 {
        let (count, op) = pair.into_pair();
        if op == gfa::cigar::CIGAROp::M {
            result += count;
        } else {
            debug!("Non-match CIGAR op encountered: {op:?} with count {count}");
            debug!("Ignoring non-match operations for overlap calculation");
        }
    }
    result
}

// Can be made faster by avoiding parsing CIGAR completely
// For now, this seems sufficient
#[inline]
fn parse_overlap(cigar_str: &[u8]) -> u32 {
    CIGAR::from_bytestring(cigar_str).map_or(0, cigar_to_overlap)
}

/// Implementation of `GraphParser` for GFA format
/// Uses gfa crate to parse GFA files and converts to `CoreGraph`
impl<R: Read + Seek> GraphParser<R> for GFACodec {
    fn parse(&self, reader: &mut R) -> PanResult<CoreGraphDTO> {
        // Containments are parsed but never used, so skip them entirely.
        let parser = GFAParserBuilder {
            segments: true,
            links: true,
            containments: false,
            paths: true,
            tolerance: ParserTolerance::default(),
        }
        .build::<Vec<u8>, ()>();

        // Stream lines into the gfa crate's `GFA` struct instead of collecting
        // the whole file into a `Vec<String>` first. `byte_lines` yields owned
        // buffers per line (no UTF-8 validation, handles `\r\n`), so the raw
        // file text is never fully resident in memory.
        let buf_reader = BufReader::new(reader);
        let mut gfa: GFA<Vec<u8>, ()> = GFA::new();
        for line in buf_reader.byte_lines() {
            let line = line?;
            match parser.parse_gfa_line(&line) {
                Ok(parsed) => gfa.insert_line(parsed),
                Err(err) if err.can_safely_continue(&ParserTolerance::Safe) => {}
                Err(err) => return Err(err.into()),
            }
        }

        // Phase 1: Create nodes sequentially (maintaining order) and build the
        // name -> id lookup, moving names and sequences out of the segments
        // instead of cloning them.
        let mut name_to_id: HashMap<NodeName, NodeId> = HashMap::with_capacity(gfa.segments.len());
        let mut nodes = Nodes::new();
        for (i, segment) in gfa.segments.into_iter().enumerate() {
            name_to_id.insert(segment.name, i);
            nodes.push(segment.sequence);
        }

        // Phase 2: Parallelize edge creation from links.
        let edges = gfa
            .links
            .into_par_iter()
            .map(|link| {
                PanResult::Ok(Edge {
                    from_node: *name_to_id.get(&link.from_segment[..]).ok_or_else(|| {
                        PanGraphXError::Parse("Failed to map from_segment to node ID".into())
                    })?,
                    from_orient: link.from_orient.into(),
                    to_node: *name_to_id.get(&link.to_segment[..]).ok_or_else(|| {
                        PanGraphXError::Parse("Failed to map to_segment to node ID".into())
                    })?,
                    to_orient: link.to_orient.into(),
                    overlap: parse_overlap(&link.overlap),
                })
            })
            .collect::<PanResult<Vec<Edge>>>()?;

        // Phase 3: Parallelize path creation from paths.
        let paths: Vec<Path> = gfa
            .paths
            .into_par_iter()
            .map(|p| {
                let steps: Vec<Step> = p
                    .iter()
                    .map(|(id, orient)| {
                        let a: &[u8] = id.iter().as_slice();
                        let o = *name_to_id.get(a).ok_or_else(|| {
                            PanGraphXError::Parse("Failed to map segment ID to node ID".into())
                        })?;
                        Step::new(o, orient.into())
                    })
                    .collect::<PanResult<Vec<Step>>>()?;

                let overlaps: Vec<u32> = p
                    .overlaps
                    .into_iter()
                    .map(|opt_cigar| opt_cigar.map_or(0, cigar_to_overlap))
                    .collect();
                PanResult::Ok(Path {
                    name: p.path_name,
                    steps,
                    overlaps,
                })
            })
            .collect::<PanResult<Vec<Path>>>()?;

        // Invert the name -> id lookup into the id -> name map the DTO stores.
        let node_name_map = name_to_id
            .into_iter()
            .map(|(name, id)| (id, name))
            .collect();

        Ok(CoreGraphDTO {
            nodes,
            edges,
            paths,
            node_name_map: Some(node_name_map),
        })
    }
}

/// Number of S/L records formatted per parallel task.
const RECORDS_PER_CHUNK: usize = 1 << 14;

/// Builds the id -> name table once, so formatting never hashes or allocates per step.
/// Nodes without an entry in `node_name_map` are named by their numeric id.
fn node_names(graph: &CoreGraphDTO) -> Vec<Cow<'_, [u8]>> {
    (0..graph.nodes.len())
        .into_par_iter()
        .map(|id| {
            graph
                .node_name_map
                .as_ref()
                .and_then(|map| map.get(&id))
                .map_or_else(
                    || Cow::Owned(itoa::Buffer::new().format(id).as_bytes().to_vec()),
                    |name| Cow::Borrowed(name.as_slice()),
                )
        })
        .collect()
}

#[inline]
const fn orientation_byte(orientation: Orientation) -> u8 {
    match orientation {
        Orientation::Forward => b'+',
        Orientation::Reverse => b'-',
    }
}

/// Formats `items` in parallel and writes the result in order.
///
/// Only one window of chunks (`RECORDS_PER_CHUNK` items per thread) is held in
/// memory at a time, so the output text is never fully resident.
fn write_records<T, F>(
    writer: &mut dyn Write,
    items: &[T],
    per_chunk: usize,
    format: F,
) -> PanResult<()>
where
    T: Sync,
    F: Fn(&T, &mut Vec<u8>) + Sync,
{
    let window = per_chunk * rayon::current_num_threads();
    for window_items in items.chunks(window) {
        let buffers: Vec<Vec<u8>> = window_items
            .par_chunks(per_chunk)
            .map(|chunk| {
                let mut buf = Vec::new();
                for item in chunk {
                    format(item, &mut buf);
                }
                buf
            })
            .collect();
        for buf in buffers {
            writer.write_all(&buf)?;
        }
    }
    Ok(())
}

fn format_segment(names: &[Cow<'_, [u8]>], node: &Node, buf: &mut Vec<u8>) {
    buf.extend_from_slice(b"S\t");
    buf.extend_from_slice(&names[node.id]);
    buf.push(b'\t');
    buf.extend_from_slice(&node.sequence);
    buf.push(b'\n');
}

fn format_link(names: &[Cow<'_, [u8]>], edge: &Edge, buf: &mut Vec<u8>) {
    buf.extend_from_slice(b"L\t");
    buf.extend_from_slice(&names[edge.from_node]);
    buf.push(b'\t');
    buf.push(orientation_byte(edge.from_orient));
    buf.push(b'\t');
    buf.extend_from_slice(&names[edge.to_node]);
    buf.push(b'\t');
    buf.push(orientation_byte(edge.to_orient));
    buf.push(b'\t');
    buf.extend_from_slice(itoa::Buffer::new().format(edge.overlap).as_bytes());
    buf.extend_from_slice(b"M\n");
}

fn format_path(names: &[Cow<'_, [u8]>], path: &Path, buf: &mut Vec<u8>) {
    buf.extend_from_slice(b"P\t");
    buf.extend_from_slice(&path.name);
    buf.push(b'\t');
    for (i, step) in path.steps.iter().enumerate() {
        if i > 0 {
            buf.push(b',');
        }
        buf.extend_from_slice(&names[step.node_id()]);
        buf.push(orientation_byte(step.orientation()));
    }
    buf.push(b'\t');
    // All-zero overlaps carry no information, so they are written as `*`.
    if path.overlaps.iter().all(|&overlap| overlap == 0) {
        buf.push(b'*');
    } else {
        let mut itoa_buf = itoa::Buffer::new();
        for (i, &overlap) in path.overlaps.iter().enumerate() {
            if i > 0 {
                buf.push(b',');
            }
            buf.extend_from_slice(itoa_buf.format(overlap).as_bytes());
            buf.push(b'M');
        }
    }
    buf.push(b'\n');
}

impl GraphSerializer for GFACodec {
    fn serialize(&self, graph: &CoreGraphDTO, writer: &mut dyn Write) -> PanResult<()> {
        let names = node_names(graph);

        writer.write_all(b"H\tVN:Z:1.0\n")?;
        write_records(
            writer,
            graph.nodes.as_slice(),
            RECORDS_PER_CHUNK,
            |node, buf| {
                format_segment(&names, node, buf);
            },
        )?;
        write_records(writer, &graph.edges, RECORDS_PER_CHUNK, |edge, buf| {
            format_link(&names, edge, buf);
        })?;
        // A single path line can be hundreds of MB, so each path is its own task.
        write_records(writer, &graph.paths, 1, |path, buf| {
            format_path(&names, path, buf);
        })?;
        Ok(())
    }
}
