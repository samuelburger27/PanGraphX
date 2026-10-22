use assert_cmd::Command;
use predicates::prelude::*;

fn cli() -> Command {
    Command::cargo_bin("pangraphx-cli").expect("binary builds")
}

fn sample_gfa() -> String {
    format!(
        "{}/../pangraphx-core/tests/test_files/gfa/tiny.gfa",
        env!("CARGO_MANIFEST_DIR")
    )
}

#[test]
fn convert_takes_positional_input() {
    let dir = tempfile::tempdir().expect("temp dir");
    let out = dir.path().join("out.gbz");
    cli()
        .args(["convert", &sample_gfa(), "-o"])
        .arg(&out)
        .assert()
        .success();
    assert!(out.exists());
}

#[test]
fn convert_stdout_has_only_data() {
    cli()
        .args(["convert", &sample_gfa(), "-o", "-", "--to", "gfa"])
        .assert()
        .success()
        .stdout(predicate::str::starts_with("H\t"))
        .stderr(
            predicate::str::contains("Loading").and(predicate::str::contains("Writing stdout")),
        );
}

#[test]
fn convert_stdin_to_stdout() {
    let gfa = std::fs::read(sample_gfa()).expect("fixture");
    let vg = cli()
        .args(["convert", "-", "--from", "gfa", "-o", "-", "--to", "vg"])
        .write_stdin(gfa)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    cli()
        .args(["info", "-", "--from", "vg"])
        .write_stdin(vg)
        .assert()
        .success()
        .stdout(predicate::str::contains("Nodes:"));
}

#[test]
fn stream_needs_explicit_format() {
    cli()
        .args(["info", "-"])
        .write_stdin("")
        .assert()
        .code(1)
        .stderr(predicate::str::contains("--from"));
    cli()
        .args(["convert", &sample_gfa(), "-o", "-"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("--to"));
}

#[test]
fn quiet_silences_progress() {
    cli()
        .args(["-q", "convert", &sample_gfa(), "-o", "-", "--to", "gfa"])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
}

#[test]
fn threads_option_is_global() {
    cli()
        .args(["--threads", "1", "stats", &sample_gfa()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Graph Statistics"));
    cli()
        .args(["stats", &sample_gfa(), "--threads", "0"])
        .assert()
        .code(2);
}

#[test]
fn dbg_writes_graph() {
    for flags in [&[][..], &["-c"], &["-t", "-u"], &["-c", "--compact"]] {
        cli()
            .args(["dbg", &sample_gfa(), "-k", "5", "-o", "-", "--to", "gfa"])
            .args(flags)
            .assert()
            .success()
            .stdout(predicate::str::contains("S\t"));
    }
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
fn convert_rejects_unknown_format() {
    cli()
        .args(["convert", "in.gfa", "-o", "out.x", "--to", "xyz"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("expected one of"));
}

#[test]
fn dbg_bad_kmer_shows_dbg_help() {
    cli()
        .args([
            "--threads",
            "2",
            "dbg",
            "in.gfa",
            "-k",
            "99",
            "-o",
            "out.gfa",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--kmer-size").and(predicate::str::contains("--compact")));
}

#[test]
fn dbg_colored_conflicts_with_full_topology() {
    cli()
        .args(["dbg", "in.gfa", "-c", "-t", "-o", "out.gfa"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("cannot be used with"));
}

#[test]
fn format_and_formats_alias() {
    cli().arg("format").assert().success();
    cli().arg("formats").assert().success();
}
