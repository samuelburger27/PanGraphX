//! External reference tools (`vg`, `odgi`) for the cross-validation tests.
//!
//! A tool is taken from `PANGRAPHX_VG` / `PANGRAPHX_ODGI`, else from `PATH`,
//! and must answer `<tool> version`. A missing tool skips the test with a
//! `SKIPPED` line on stderr, or fails it when `PANGRAPHX_REQUIRE_EXTERNAL=1`
//! so that CI cannot pass without checking anything.

use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

pub struct Tool {
    pub name: &'static str,
    pub program: PathBuf,
}

impl Tool {
    fn find(name: &'static str) -> Option<Self> {
        let var = format!("PANGRAPHX_{}", name.to_ascii_uppercase());
        let program = std::env::var_os(&var).map_or_else(|| PathBuf::from(name), PathBuf::from);
        let answers = Command::new(&program)
            .arg("version")
            .output()
            .is_ok_and(|out| out.status.success());
        answers.then_some(Self { name, program })
    }

    /// Runs the tool with the arguments `args` adds and returns its stdout;
    /// panics with stderr on failure.
    pub fn run(&self, args: impl FnOnce(&mut Command) -> &mut Command) -> Vec<u8> {
        let mut command = Command::new(&self.program);
        args(&mut command);
        let shown: Vec<String> = command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let out = command
            .output()
            .unwrap_or_else(|e| panic!("cannot run {}: {e}", self.name));
        assert!(
            out.status.success(),
            "`{} {}` failed ({}):\n{}",
            self.name,
            shown.join(" "),
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        out.stdout
    }
}

fn cached(name: &'static str) -> Option<&'static Tool> {
    static VG: OnceLock<Option<Tool>> = OnceLock::new();
    static ODGI: OnceLock<Option<Tool>> = OnceLock::new();
    let cell = match name {
        "vg" => &VG,
        "odgi" => &ODGI,
        other => panic!("unknown tool {other}"),
    };
    cell.get_or_init(|| Tool::find(name)).as_ref()
}

/// Prints `message` even for a passing test: libtest captures `eprintln!`,
/// but not writes to the stderr handle itself.
pub fn notice(message: &str) {
    let _ = writeln!(std::io::stderr(), "{message}");
}

/// The tool, or `None` after reporting the skip.
///
/// # Panics
/// When the tool is missing and `PANGRAPHX_REQUIRE_EXTERNAL=1`.
pub fn require_tool(name: &'static str, test: &str) -> Option<&'static Tool> {
    let tool = cached(name);
    if tool.is_none() {
        let var = format!("PANGRAPHX_{}", name.to_ascii_uppercase());
        assert!(
            std::env::var("PANGRAPHX_REQUIRE_EXTERNAL").as_deref() != Ok("1"),
            "{name} not found (set {var} or add it to PATH); \
             PANGRAPHX_REQUIRE_EXTERNAL=1 forbids skipping {test}"
        );
        notice(&format!(
            "SKIPPED {test}: {name} not found (set {var} or add it to PATH)"
        ));
    }
    tool
}
