use super::records::parse_overlap;
use super::steps::{PATH_PIECE_SIZE, split_at_commas, split_at_orientations};
use super::*;
use crate::core::core_types::Orientation;
use std::fmt::Write as _;
use std::io::Cursor;

fn parse_str(text: &str) -> PanResult<CoreGraphDTO> {
    parse(&mut Cursor::new(text.as_bytes()))
}

fn err(text: &str) -> String {
    parse_str(text).unwrap_err().to_string()
}

fn step_names(graph: &CoreGraphDTO, path: usize) -> Vec<(String, Orientation)> {
    graph.paths[path]
        .steps
        .iter()
        .map(|s| (graph.get_name_from_id(s.node_id()), s.orientation()))
        .collect()
}

const SMALL: &str = "H\tVN:Z:1.0\n\
    S\t1\tACGT\n\
    S\t2\tGG\tLN:i:2\n\
    S\t3\t*\n\
    L\t1\t+\t2\t-\t3M\n\
    L\t2\t-\t3\t+\t*\n\
    P\tp1\t1+,2-,3+\t3M,0M\n\
    P\tp2\t3-\t*\n";

#[test]
fn parses_small_graph() {
    let g = parse_str(SMALL).unwrap();
    assert_eq!(g.nodes.len(), 3);
    assert_eq!(g.nodes[0].sequence, b"ACGT");
    assert_eq!(g.nodes[2].sequence, b"*");
    assert_eq!(g.get_name_from_id(1), "2");
    assert_eq!(g.edges.len(), 2);
    assert_eq!(g.edges[0].overlap, 3);
    assert_eq!(g.edges[0].to_orient, Orientation::Reverse);
    assert_eq!(g.edges[1].overlap, 0);
    assert_eq!(
        step_names(&g, 0),
        vec![
            ("1".into(), Orientation::Forward),
            ("2".into(), Orientation::Reverse),
            ("3".into(), Orientation::Forward)
        ]
    );
    assert_eq!(g.paths[0].overlaps, vec![3, 0]);
    assert_eq!(g.paths[1].overlaps, Vec::<u32>::new());
}

/// Every vg naming case: reference (`RS`), haplotype and generic walks,
/// with and without subranges, between `P` lines.
const WALKS: &str = "H\tVN:Z:1.1\tRS:Z:GRCh38  CHM13\n\
    S\t1\tACGT\n\
    S\t2\tGG\n\
    P\tfirst\t1+,2+\t*\n\
    W\tGRCh38\t0\tchr1\t0\t6\t>1>2\n\
    W\tCHM13\t2\tchr1\t100\t106\t>1>2\n\
    W\tHG1\t1\tctgA\t0\t6\t<2<1\n\
    W\tHG1\t2\tctgB\t5\t*\t>2\tSR:i:0\n\
    W\tHG2\t0\tctgC\t*\t*\t>1<2>1\n\
    W\t*\t0\tunplaced\t7\t11\t>1\n\
    P\tlast\t2-\t*\n";

#[test]
fn parses_walks_as_named_paths() {
    let g = parse_str(WALKS).unwrap();
    let names: Vec<String> = g.paths.iter().map(|p| lossy(&p.name).into()).collect();
    assert_eq!(
        names,
        [
            "first",
            "GRCh38#0#chr1",
            "CHM13#2#chr1[100-106]",
            "HG1#1#ctgA#0",
            "HG1#2#ctgB#0[5]",
            "HG2#0#ctgC#0",
            "unplaced[7-11]",
            "last"
        ]
    );
    assert_eq!(
        step_names(&g, 3),
        vec![
            ("2".into(), Orientation::Reverse),
            ("1".into(), Orientation::Reverse)
        ]
    );
    assert_eq!(
        step_names(&g, 5),
        vec![
            ("1".into(), Orientation::Forward),
            ("2".into(), Orientation::Reverse),
            ("1".into(), Orientation::Forward)
        ]
    );
    assert!(g.paths.iter().all(|p| p.overlaps.is_empty()));
}

#[test]
fn walks_without_reference_samples_are_haplotypes() {
    let g = parse_str("S\ta\tA\nW\tGRCh38\t0\tchr1\t0\t1\t>a\n").unwrap();
    assert_eq!(g.paths[0].name, b"GRCh38#0#chr1#0");
}

#[test]
fn crlf_and_missing_final_newline() {
    let text = SMALL.replace('\n', "\r\n");
    let g = parse_str(text.trim_end()).unwrap();
    assert_eq!(g, parse_str(SMALL).unwrap());
}

#[test]
fn records_before_their_segments() {
    let text = "P\tp\tb+,a-\t*\nL\ta\t+\tb\t+\t0M\nS\ta\tA\nS\tb\tC\n";
    let g = parse_str(text).unwrap();
    assert_eq!(g.get_name_from_id(0), "a");
    assert_eq!(
        step_names(&g, 0),
        vec![
            ("b".into(), Orientation::Forward),
            ("a".into(), Orientation::Reverse)
        ]
    );
    assert_eq!((g.edges[0].from_node, g.edges[0].to_node), (0, 1));
}

#[test]
fn non_canonical_numbers_are_names() {
    // Mixed names fall back to hashing; `007` and `7` are different segments.
    let g = parse_str("S\t7\tA\nS\t007\tC\nP\tp\t007+,7+\t*\n").unwrap();
    assert_eq!(step_names(&g, 0)[0].0, "007");
    assert_eq!(g.paths[0].steps[0].node_id(), 1);
    // With numeric names, a non-canonical reference does not resolve.
    assert!(err("S\t7\tA\nP\tp\t007+\t*\n").contains("undefined segment 007"));
}

#[test]
fn skips_other_record_types() {
    let text = "# comment\nH\tVN:Z:1.2\nS\t1\tA\n\nC\t1\t+\t1\t+\t0\t1M\nJ\t1\t+\t1\t+\t*\n";
    let g = parse_str(text).unwrap();
    assert_eq!(g.nodes.len(), 1);
    assert!(g.edges.is_empty() && g.paths.is_empty());
}

#[test]
fn empty_input() {
    let g = parse_str("").unwrap();
    assert!(g.nodes.is_empty() && g.edges.is_empty() && g.paths.is_empty());
}

#[test]
fn reports_errors_with_line_numbers() {
    let cases = [
        ("S\t1\tA\nS\t1\tC\n", "line 2: duplicate segment name 1"),
        ("S\ta\tA\nS\ta\tC\n", "line 2: duplicate segment name a"),
        (
            "S\ta\tA\nL\ta\t+\ta\t+\t0M\n\nS\tb\tC\nS\ta\tG\n",
            "line 5: duplicate segment name a",
        ),
        (
            "S\t1\tA\nS\t2\tA\nH\nS\t2\tC\n",
            "line 4: duplicate segment name 2",
        ),
        ("S\t1\tAC GT\n", "line 1: segment 1 has an invalid sequence"),
        ("S\t1\n", "line 1: segment 1 has no sequence field"),
        (
            "S\t1\tA\nL\t1\t+\t2\t+\t0M\n",
            "line 2: reference to undefined segment 2",
        ),
        (
            "S\t1\tA\nL\t1\tx\t1\t+\t0M\n",
            "line 2: invalid orientation",
        ),
        (
            "S\t1\tA\nL\t1\t+\t1\t+\t5Q\n",
            "line 2: invalid CIGAR overlap",
        ),
        (
            "S\t1\tA\nL\t1\t+\t1\t+\n",
            "line 2: L line without an overlap",
        ),
        (
            "S\t1\tA\n\nP\tp\t1\t*\n",
            "line 3: path p: step \"1\" has no orientation",
        ),
        ("S\t1\tA\nP\tp\t1+,,1+\t*\n", "line 2: path p: empty step"),
        ("S\t1\tA\nP\tp\t1+\n", "line 2: path p has no overlap field"),
        (
            "S\t1\tA\nW\ts\t1\tc\t0\t1\n",
            "line 2: W line without a walk",
        ),
        (
            "S\t1\tA\nW\ts\tx\tc\t0\t1\t>1\n",
            "line 2: invalid haplotype index \"x\"",
        ),
        (
            "S\t1\tA\nW\ts\t1\tc\t-1\t1\t>1\n",
            "line 2: invalid start position \"-1\"",
        ),
        (
            "S\t1\tA\nW\ts\t1\tc\t0\t99999999999999999999\t>1\n",
            "line 2: invalid end position",
        ),
        (
            "S\t1\tA\nW\t\t1\tc\t0\t1\t>1\n",
            "line 2: empty sample name",
        ),
        (
            "S\t1\tA\nW\ts\t1\tc\t0\t1\t*\n",
            "line 2: walk s#1#c#0: walk does not start with > or <",
        ),
        (
            "S\t1\tA\nW\ts\t1\tc\t0\t1\t>1<\n",
            "line 2: walk s#1#c#0: empty walk step",
        ),
        (
            "S\t1\tA\nW\ts\t1\tc\t0\t1\t\n",
            "line 2: walk s#1#c#0: empty walk",
        ),
        (
            "S\t1\tA\nW\ts\t1\tc\t0\t1\t>1>2\n",
            "line 2: walk s#1#c#0: reference to undefined segment 2",
        ),
    ];
    for (text, expected) in cases {
        let message = err(text);
        assert!(message.contains(expected), "{text:?}: {message}");
    }
}

#[test]
fn cigar_overlaps() {
    assert_eq!(parse_overlap(b"*"), Ok(0));
    assert_eq!(parse_overlap(b"10M"), Ok(10));
    assert_eq!(parse_overlap(b"3M2I4M"), Ok(7));
    assert!(parse_overlap(b"").is_err());
    assert!(parse_overlap(b"M").is_err());
    assert!(parse_overlap(b"12").is_err());
    assert!(parse_overlap(b"99999999999M").is_err());
}

#[test]
fn tiny_blocks_match_default_blocks() {
    // Every block boundary falls inside a line; some lines exceed a block.
    for text in [SMALL, WALKS] {
        let reference = parse_str(text).unwrap();
        for block_size in [1, 2, 7, 16] {
            let g = parse_with_block_size(&mut Cursor::new(text.as_bytes()), block_size).unwrap();
            assert_eq!(g, reference, "block size {block_size}");
        }
    }
}

#[test]
fn long_step_lists_are_split_in_pieces() {
    let list: Vec<u8> = (0..100)
        .flat_map(|i| format!("{i}+,").into_bytes())
        .collect();
    let list = &list[..list.len() - 1];
    for piece_size in [1, 5, 64, list.len()] {
        let pieces = split_at_commas(list, piece_size);
        assert_eq!(pieces.join(&b','), list, "piece size {piece_size}");
    }

    let mut text = String::new();
    for i in 0..300_000 {
        writeln!(text, "S\t{i}\tA").unwrap();
    }
    text.push_str("P\tp\t");
    let steps: Vec<String> = (0..300_000).map(|i| format!("{i}-")).collect();
    text.push_str(&steps.join(","));
    text.push_str("\t*\n");
    let g = parse_str(&text).unwrap();
    assert!(steps.join(",").len() > PATH_PIECE_SIZE);
    assert_eq!(g.paths[0].steps.len(), 300_000);
    assert!(
        g.paths[0]
            .steps
            .iter()
            .enumerate()
            .all(|(i, s)| s.node_id() == i && s.is_reverse())
    );
}

#[test]
fn long_walks_are_split_in_pieces() {
    let walk: Vec<u8> = (0..100)
        .flat_map(|i| format!("{}{i}", if i % 3 == 0 { '<' } else { '>' }).into_bytes())
        .collect();
    for piece_size in [1, 5, 64, walk.len()] {
        let pieces = split_at_orientations(&walk, piece_size);
        assert_eq!(pieces.concat(), walk, "piece size {piece_size}");
        assert!(
            pieces
                .iter()
                .all(|p| matches!(p.first(), Some(b'>' | b'<'))),
            "piece size {piece_size}"
        );
    }

    let mut text = String::new();
    for i in 0..300_000 {
        writeln!(text, "S\t{i}\tA").unwrap();
    }
    text.push_str("W\ts\t1\tc\t0\t*\t");
    for i in 0..300_000 {
        write!(text, "<{i}").unwrap();
    }
    text.push('\n');
    let g = parse_str(&text).unwrap();
    assert!(text.lines().last().unwrap().len() > PATH_PIECE_SIZE);
    assert_eq!(g.paths[0].steps.len(), 300_000);
    assert!(
        g.paths[0]
            .steps
            .iter()
            .enumerate()
            .all(|(i, s)| s.node_id() == i && s.is_reverse())
    );
}

#[test]
fn reads_from_current_position() {
    let mut cursor = Cursor::new(format!("ignored\n{SMALL}").into_bytes());
    cursor.set_position(8);
    assert_eq!(parse(&mut cursor).unwrap(), parse_str(SMALL).unwrap());
}
