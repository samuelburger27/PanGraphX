use pangraphx_core::*;
use std::{fs, path};
use tempfile::NamedTempFile;

#[test]
fn test_gbz_format() {
    let format = GraphFormat::GBZ;
    let test_files = fs::read_dir("tests/test_files/gbz").unwrap();
    for entry in test_files {
        let path = entry.unwrap().path();
        let path_string = path.to_str().unwrap();
        println!("Testing GBZ file: {}... ", path_string);
        let graph = CoreGraphDTO::load_from_file(path_string, format).unwrap();
        //assert!(graph.is_ok(), "Failed to parse GBZ file: {}", path_string);
        //let graph = graph.unwrap();
        let temp_file = NamedTempFile::new().unwrap();
        // Save to GFA fromat, since GBZ serialization is currently not supported

        graph
            .save_to_file(temp_file.path().to_str().unwrap(), GraphFormat::GFA)
            .unwrap();
        assert!(temp_file.path().exists());

        let reloaded_graph =
            CoreGraphDTO::load_from_file(temp_file.path().to_str().unwrap(), GraphFormat::GFA)
                .unwrap();

        assert_eq!(
            graph.nodes, reloaded_graph.nodes,
            "Graphs do not match after save/load cycle for file: {}",
            path_string
        );
        assert_eq!(
            graph.edges, reloaded_graph.edges,
            "Graphs do not match after save/load cycle for file: {}",
            path_string
        );
        // assert_eq!(
        //     graph.paths, reloaded_graph.paths,
        //     "Graphs do not match after save/load cycle for file: {}",
        //     path_string
        // );
    }
}
