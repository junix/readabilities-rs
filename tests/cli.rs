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
    // The version handshake is machine-parsed downstream: pin it exactly.
    // `version()` carries the git build stamp (ADR-1168) when present.
    let version = binary().args(["--version"]).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&version.stdout),
        format!("readabilities-rs {}\n", readabilities_rs::version())
    );
    let version = binary().args(["version", "--json"]).output().unwrap();
    assert!(version.status.success());
    let version: serde_json::Value = serde_json::from_slice(&version.stdout).unwrap();
    assert_eq!(version["schema_version"], 1);
    assert_eq!(version["name"], "readabilities-rs");
    assert_eq!(version["version"], readabilities_rs::version());

    let output = binary().args(["doctor", "--json"]).output().unwrap();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["core"], true);
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["providers"], false);
    assert_eq!(value["external_processes"], false);

    // The human doctor report keeps its feature-independent lines stable.
    let human = binary().args(["doctor"]).output().unwrap();
    assert!(human.status.success());
    let report = String::from_utf8_lossy(&human.stdout);
    for line in [
        format!("readabilities-rs {}", readabilities_rs::version()).as_str(),
        "core: ok",
        "providers: false",
        "external processes: false",
    ] {
        assert!(
            report.lines().any(|rendered| rendered == line),
            "doctor report must keep {line:?}: {report}"
        );
    }
}

#[cfg(feature = "http")]
#[test]
fn vendor_provider_flags_are_not_part_of_the_cli() {
    let output = binary()
        .args(["read", "https://example.test/article", "--provider", "jina"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("unexpected argument '--provider'"));
}

#[cfg(feature = "http")]
#[test]
fn external_browser_flags_are_not_part_of_the_cli() {
    let output = binary()
        .args(["read", "https://example.test/article", "--browser"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("unexpected argument '--browser'"));
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

    // The same bytes with the same base URL must produce the identical
    // article: the input transport may not leak into extraction results.
    let mut child = binary()
        .args([
            "extract",
            "-",
            "--url",
            "https://example.test/article",
            "--json",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(&html).unwrap();
    let stdin_output = child.wait_with_output().unwrap();
    assert!(stdin_output.status.success());
    let stdin_json: serde_json::Value = serde_json::from_slice(&stdin_output.stdout).unwrap();
    assert_eq!(
        stdin_json["content"], file_json["content"],
        "stdin and file extraction must share the contract"
    );
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
    assert_eq!(output.status.code(), Some(1));
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["kind"], "invalid_input");
    assert_eq!(error["stage"], "validate");
    assert_eq!(error["backend"], "local");
    assert_eq!(error["message"], "HTML input is empty");
    assert_eq!(error["retry"], "never");
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
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["outcome"]["status"], "success");
    let attempts = value["attempts"].as_array().unwrap();
    assert!(!attempts.is_empty());
    assert!(
        attempts
            .iter()
            .all(|attempt| attempt["engine"] == "readabilities-rs")
    );
    // Exactly one attempt is the selected extraction.
    assert_eq!(
        attempts
            .iter()
            .filter(|attempt| attempt["selected"] == true)
            .count(),
        1
    );
    // Local extraction leaves the network ledger untouched.
    assert_eq!(value["cost"]["origin_requests"], 0);
    assert_eq!(value["cost"]["downloaded_bytes"], 0);
    // The pipeline records validate -> parse -> sanitize, all in-process.
    let stages = value["stages"].as_array().unwrap();
    assert_eq!(
        stages
            .iter()
            .map(|stage| stage["stage"].clone())
            .collect::<Vec<_>>(),
        vec!["validate", "parse", "sanitize"]
    );
    assert!(stages.iter().all(|stage| stage["backend"] == "local"));
    assert!(value["removals"].as_array().is_some());
}
