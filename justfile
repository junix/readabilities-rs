# justfile for Rust project

default:
    @just --list

# Build the crate
build:
    cargo build

# Build with release optimizations
build-release:
    cargo build --release

# Run tests
test:
    cargo test

# Run tests with output
test-verbose:
    cargo test -- --nocapture

# Check code without building
check:
    cargo check

# Format code
fmt:
    cargo fmt

# Format code and check
fmt-check:
    cargo fmt -- --check

# Run linter
clippy:
    cargo clippy -- -D warnings

# Run linter with fixes
clippy-fix:
    cargo clippy --fix --allow-dirty --allow-staged

# Run the complete local release gate.
check-all:
    cargo fmt --all -- --check
    cargo clippy --all-targets --all-features -- -D warnings
    cargo test --locked
    cargo test --locked --no-default-features
    cargo test --locked --all-features
    cargo build --locked
    cargo build --locked --no-default-features
    cargo build --locked --all-features

# Clean build artifacts
clean:
    cargo clean

# Update dependencies
update:
    cargo update

# Run cargo doc with open
doc:
    cargo doc --open

# Run with watch
watch:
    cargo watch -x check -x test -x run

# Install dev tools
install-tools:
    cargo install cargo-watch cargo-edit cargo-audit

# Benchmark wall time, output bytes, stage timings, debug overhead, and peak RSS.
benchmark iterations="20":
    #!/usr/bin/env bash
    set -euo pipefail
    cargo build --release --locked --example extraction_benchmark
    if [[ "$(uname -s)" == "Darwin" ]]; then
      /usr/bin/time -l ./target/release/examples/extraction_benchmark --iterations "{{ iterations }}"
    else
      /usr/bin/time -v ./target/release/examples/extraction_benchmark --iterations "{{ iterations }}"
    fi

bench iterations="20":
    just benchmark "{{ iterations }}"

# Install the release binary into the shared per-platform bin directory.
install:
    #!/usr/bin/env bash
    set -euo pipefail
    case "$(uname -s)" in
      Darwin) os_name=macos ;;
      Linux) os_name=linux ;;
      *) echo "unsupported OS" >&2; exit 1 ;;
    esac
    case "$(uname -m)" in
      arm64|aarch64) arch_name=arm64 ;;
      x86_64|amd64) arch_name=x86 ;;
      *) echo "unsupported architecture" >&2; exit 1 ;;
    esac
    install_dir="${SYNC_BIN_DIR:-${HOME}/sync/${os_name}-${arch_name}-bin}"
    cargo build --release --locked --all-features
    mkdir -p "$install_dir"
    cp target/release/readabilities-rs "$install_dir/readabilities-rs"
    echo "Installed $install_dir/readabilities-rs"

# Generate coverage
coverage:
    cargo tarpaulin --out Html

# Show dependency tree
deps:
    cargo tree

# Show outdated dependencies
outdated:
    cargo outdated
