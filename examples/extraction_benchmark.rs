use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fs;
use std::hint::black_box;
use std::path::PathBuf;
use std::time::Instant;

use readabilities_rs::{ExecutionOutcome, ExtractionOptions, ReadRequest, Reader, Stage};
use serde::Serialize;

const FIXTURES: &[&str] = &[
    "READ-001-noisy-article.html",
    "READ-002-short-note.html",
    "READ-003-chinese-article.html",
    "READ-004-technical-doc.html",
    "READ-005-news.html",
    "READ-006-blog.html",
    "READ-007-forum.html",
    "READ-008-math-footnotes.html",
    "READ-009-images.html",
    "READ-010-security.html",
];

#[derive(Serialize)]
struct Pass {
    diagnostics: bool,
    documents: usize,
    wall_ms: f64,
    average_us_per_document: f64,
    average_output_bytes: usize,
    removal_records: usize,
    recorded_stage_ms: BTreeMap<String, u64>,
}

#[derive(Serialize)]
struct Report {
    schema_version: u32,
    iterations: usize,
    samples: usize,
    corpus_documents: usize,
    normal: Pass,
    diagnostics: Pass,
    normal_wall_samples_ms: Vec<f64>,
    diagnostics_wall_samples_ms: Vec<f64>,
    diagnostics_wall_overhead_percent: f64,
    memory_note: &'static str,
}

#[tokio::main]
async fn main() {
    let (iterations, samples) = parse_options();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let fixtures = FIXTURES
        .iter()
        .map(|name| fs::read_to_string(root.join(name)).expect("fixture must be readable"))
        .collect::<Vec<_>>();

    let _ = run_pass(&fixtures, 1, false).await;
    let _ = run_pass(&fixtures, 1, true).await;
    let mut normal_samples = Vec::with_capacity(samples);
    let mut diagnostics_samples = Vec::with_capacity(samples);
    for sample in 0..samples {
        if sample % 2 == 0 {
            normal_samples.push(run_pass(&fixtures, iterations, false).await);
            diagnostics_samples.push(run_pass(&fixtures, iterations, true).await);
        } else {
            diagnostics_samples.push(run_pass(&fixtures, iterations, true).await);
            normal_samples.push(run_pass(&fixtures, iterations, false).await);
        }
    }
    let normal_wall_samples_ms = normal_samples.iter().map(|pass| pass.wall_ms).collect();
    let diagnostics_wall_samples_ms = diagnostics_samples
        .iter()
        .map(|pass| pass.wall_ms)
        .collect();
    let normal = median_pass(normal_samples);
    let diagnostics = median_pass(diagnostics_samples);
    let overhead = if normal.wall_ms > 0.0 {
        (diagnostics.wall_ms / normal.wall_ms - 1.0) * 100.0
    } else {
        0.0
    };
    let report = Report {
        schema_version: 1,
        iterations,
        samples,
        corpus_documents: fixtures.len(),
        normal,
        diagnostics,
        normal_wall_samples_ms,
        diagnostics_wall_samples_ms,
        diagnostics_wall_overhead_percent: overhead,
        memory_note: "Use `just benchmark` to add OS-reported peak resident bytes for the benchmark process.",
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&report).expect("benchmark report must serialize")
    );
}

async fn run_pass(fixtures: &[String], iterations: usize, diagnostics: bool) -> Pass {
    let extraction = ExtractionOptions {
        diagnostics,
        ..ExtractionOptions::default()
    };
    let reader = Reader::new();
    let started = Instant::now();
    let mut output_bytes = 0_usize;
    let mut removal_records = 0_usize;
    let mut recorded_stage_ms = BTreeMap::<String, u64>::new();

    for _ in 0..iterations {
        for html in fixtures {
            let mut request = ReadRequest::html(black_box(html.clone()), None);
            request.extraction = extraction.clone();
            let execution = reader.execute(request).await;
            let article = match execution.outcome {
                ExecutionOutcome::Success(article) => article,
                ExecutionOutcome::Failure(error) => panic!("benchmark extraction failed: {error}"),
            };
            output_bytes = output_bytes.saturating_add(black_box(article.content.len()));
            removal_records = removal_records.saturating_add(execution.removals.len());
            for stage in execution.stages {
                let key = stage_name(stage.stage).to_string();
                *recorded_stage_ms.entry(key).or_default() += stage.elapsed_ms;
            }
        }
    }
    let elapsed = started.elapsed();
    let documents = iterations * fixtures.len();
    let documents_for_average =
        u32::try_from(documents).expect("benchmark document count exceeds u32");
    Pass {
        diagnostics,
        documents,
        wall_ms: elapsed.as_secs_f64() * 1_000.0,
        average_us_per_document: elapsed.as_secs_f64() * 1_000_000.0
            / f64::from(documents_for_average),
        average_output_bytes: output_bytes / documents,
        removal_records,
        recorded_stage_ms,
    }
}

fn stage_name(stage: Stage) -> &'static str {
    match stage {
        Stage::Validate => "validate",
        Stage::Acquire => "acquire",
        Stage::Parse => "parse_locate_clean_normalize",
        Stage::Locate => "locate",
        Stage::Clean => "clean",
        Stage::Normalize => "normalize",
        Stage::Sanitize => "sanitize",
        Stage::Render => "render",
        Stage::RemoteSubmit => "remote_submit",
        Stage::RemotePoll => "remote_poll",
    }
}

fn median_pass(mut passes: Vec<Pass>) -> Pass {
    passes.sort_by(|left, right| {
        left.wall_ms
            .partial_cmp(&right.wall_ms)
            .unwrap_or(Ordering::Equal)
    });
    passes.swap_remove(passes.len() / 2)
}

fn parse_options() -> (usize, usize) {
    let mut args = std::env::args().skip(1);
    let mut iterations = 20_usize;
    let mut samples = 5_usize;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--iterations" => {
                iterations = args
                    .next()
                    .expect("--iterations requires a value")
                    .parse()
                    .expect("iterations must be a positive integer");
            }
            "--samples" => {
                samples = args
                    .next()
                    .expect("--samples requires a value")
                    .parse()
                    .expect("samples must be a positive integer");
            }
            _ => {}
        }
    }
    assert!(iterations > 0, "iterations must be positive");
    assert!(samples > 0, "samples must be positive");
    (iterations, samples)
}
