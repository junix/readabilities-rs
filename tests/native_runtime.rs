use std::fs;
use std::path::{Path, PathBuf};

const FORBIDDEN_MANIFEST_DEPENDENCIES: &[&str] =
    &["chromiumoxide", "decruft", "pyo3", "secrecy", "trafilatura"];

const FORBIDDEN_PRODUCTION_TOKENS: &[&str] = &[
    "Command::new(",
    "std::process::Command",
    "tokio::process",
    "api.firecrawl.dev",
    "r.jina.ai",
    "trafilatura",
    "decruft",
];

#[test]
fn manifest_has_no_delegated_extraction_runtime() {
    let manifest = include_str!("../Cargo.toml").to_ascii_lowercase();
    for dependency in FORBIDDEN_MANIFEST_DEPENDENCIES {
        assert!(
            !manifest.contains(dependency),
            "delegated runtime dependency `{dependency}` is forbidden"
        );
    }
}

#[test]
fn production_sources_do_not_launch_or_call_delegated_extractors() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    for path in rust_sources(&root) {
        let source = fs::read_to_string(&path).unwrap();
        let lowercase = source.to_ascii_lowercase();
        for token in FORBIDDEN_PRODUCTION_TOKENS {
            assert!(
                !lowercase.contains(&token.to_ascii_lowercase()),
                "{} contains forbidden delegated-runtime token `{token}`",
                path.display()
            );
        }
    }
}

fn rust_sources(root: &Path) -> Vec<PathBuf> {
    let mut sources = Vec::new();
    collect_rust_sources(root, &mut sources);
    sources.sort();
    sources
}

fn collect_rust_sources(directory: &Path, sources: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect_rust_sources(&path, sources);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            sources.push(path);
        }
    }
}
