use pangraphx_core::*;
use std::fs;
use tempfile::NamedTempFile;

#[test]
fn test_gbz_format() {
    // TODO finish this , it is 
    // Test all GFA files in the test_files/gfa directory
    // Read each file, parse it, serialize it back, and compare the contents
    let format = GraphFormat::GBZ;
    let test_files = fs::read_dir("tests/test_files/gfa").unwrap();
    for entry in test_files {
        let path = entry.unwrap().path();
        let path_string = path.to_str().unwrap();
        let graph = CoreGraph::load_from_file(path_string, format);
        assert!(graph.is_ok(), "Failed to parse GFA file: {}", path_string);
        let graph = graph.unwrap();
        let temp_file = NamedTempFile::new().unwrap();
        graph
            .save_to_file(temp_file.path().to_str().unwrap(), format)
            .unwrap();
        assert!(temp_file.path().exists());

        let reloaded_graph =
            CoreGraph::load_from_file(temp_file.path().to_str().unwrap(), format).unwrap();
        assert_eq!(
            graph, reloaded_graph,
            "Graphs do not match after save/load cycle for file: {}",
            path_string
        );

        // Compare file contents
        let original_content = fs::read_to_string(path_string).unwrap();
        let saved_content = fs::read_to_string(temp_file.path()).unwrap();
        assert_eq!(original_content, saved_content);
    }

    // Your test code here
}
