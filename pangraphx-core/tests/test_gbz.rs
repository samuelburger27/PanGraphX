use pangraphx_core::*;
use std::fs;
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
        //let graph = graph.unwrap();
        let temp_file = NamedTempFile::new().unwrap();
        // Save to GFA format, since GBZ serialization is currently not supported
        graph
            .save_to_file(temp_file.path().to_str().unwrap(), GraphFormat::GFA)
            .unwrap();
        assert!(temp_file.path().exists());

        let reloaded_graph =
            CoreGraphDTO::load_from_file(temp_file.path().to_str().unwrap(), GraphFormat::GFA)
                .unwrap();

        assert!(
            graph.isomorphic(&reloaded_graph),
            "Graphs do not match after save/load cycle for file: {}",
            path_string
        );
    }
}
