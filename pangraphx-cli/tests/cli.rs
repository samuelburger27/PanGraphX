use assert_cmd::Command;
use predicates::prelude::*;

fn cli() -> Command {
    Command::cargo_bin("pangraphx-cli").expect("binary builds")
}

fn sample_gfa() -> String {
    format!(
        "{}/../pangraphx-core/tests/test_files/gfa/merged.gfa",
        env!("CARGO_MANIFEST_DIR")
    )
}

#[test]
fn convert_takes_positional_input() {
    let dir = std::env::temp_dir().join("pangraphx_cli_positional");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let out = dir.join("out.gfa");
    cli()
        .args(["convert", &sample_gfa(), "-o"])
        .arg(&out)
        .assert()
        .success();
    assert!(out.exists());
}

#[test]
fn convert_without_args_shows_help() {
    cli()
        .arg("convert")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("Usage:").and(predicate::str::contains("--output")));
}

#[test]
fn convert_missing_output_shows_error_and_help() {
    cli().args(["convert", "in.gfa"]).assert().code(2).stderr(
        predicate::str::contains("required arguments")
            .and(predicate::str::contains("Options:"))
            .and(predicate::str::contains("--from")),
    );
}

#[test]
fn convert_rejects_old_input_flag() {
    cli()
        .args(["convert", "-i", "in.gfa", "-o", "out.gbz"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("Options:"));
}

#[test]
fn ddb_bad_kmer_shows_ddb_help() {
    cli()
        .args(["ddb", "in.gfa", "-k", "99", "-o", "out.gfa"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--kmer-size").and(predicate::str::contains("--compact")));
}

#[test]
fn format_and_formats_alias() {
    cli().arg("format").assert().success();
    cli().arg("formats").assert().success();
}

fn test_file(rel: &str) -> String {
    format!(
        "{}/../pangraphx-core/tests/test_files/{rel}",
        env!("CARGO_MANIFEST_DIR")
    )
}

#[test]
fn convert_writes_packed_graph() {
    let dir = std::env::temp_dir().join("pangraphx_cli_packed_graph");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let out = dir.join("out.pg");
    cli()
        .args(["convert", &sample_gfa(), "-o"])
        .arg(&out)
        .assert()
        .success();
    cli()
        .arg("info")
        .arg(&out)
        .assert()
        .success()
        .stdout(predicate::str::contains("PackedGraph"));
}

/// Current vg writes `PackedGraph` into `.vg` files by default; `.hg` is `HashGraph`.
#[test]
fn info_reads_vg_handle_graphs() {
    let dir = std::env::temp_dir().join("pangraphx_cli_vg_handle_graphs");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let renamed = dir.join("random.vg");
    std::fs::copy(test_file("pg/random.pg"), &renamed).expect("copy fixture");
    for file in [
        test_file("pg/random.pg"),
        test_file("hg/random.hg"),
        renamed.to_string_lossy().into_owned(),
    ] {
        cli()
            .args(["info", &file])
            .assert()
            .success()
            .stdout(predicate::str::contains("Nodes").and(predicate::str::contains("100")));
    }
}
