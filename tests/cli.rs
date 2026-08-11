use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_readabilities-rs"))
}

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/READ-001-noisy-article.html")
}

#[test]
fn help_version_and_doctor_are_public() {
    for args in [["--help"].as_slice(), ["--version"].as_slice()] {
        let output = binary().args(args).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let output = binary().args(["doctor", "--json"]).output().unwrap();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["core"], true);
    assert_eq!(value["schema_version"], 1);
}

#[test]
fn file_and_stdin_extraction_share_the_contract() {
    let file_output = binary()
        .args([
            "extract",
            fixture().to_str().unwrap(),
            "--url",
            "https://example.test/article",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(file_output.status.success());
    let file_json: serde_json::Value = serde_json::from_slice(&file_output.stdout).unwrap();
    assert!(
        file_json["content"]
            .as_str()
            .unwrap()
            .contains("REQUIRED-CENTRAL-IDEA")
    );

    let html = std::fs::read(fixture()).unwrap();
    let mut child = binary()
        .args(["extract", "-", "--format", "text"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(&html).unwrap();
    let stdin_output = child.wait_with_output().unwrap();
    assert!(stdin_output.status.success());
    let text = String::from_utf8_lossy(&stdin_output.stdout);
    assert!(text.contains("REQUIRED-CENTRAL-IDEA"));
    assert!(!text.contains("FORBIDDEN-FOOTER"));
}

#[test]
fn empty_input_is_a_typed_nonzero_failure() {
    let mut child = binary()
        .args(["extract", "-", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdin.take());
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["kind"], "invalid_input");
    assert_eq!(error["stage"], "validate");
}

#[cfg(feature = "http")]
#[test]
fn debug_output_contains_bounded_execution_evidence() {
    let output = binary()
        .args(["extract", fixture().to_str().unwrap(), "--debug"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["outcome"]["status"], "success");
    assert!(
        value["attempts"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );
    assert!(value["removals"].as_array().is_some());
}
