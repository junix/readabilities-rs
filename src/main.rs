use std::fs;
use std::io::{self, Read};
use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
#[cfg(feature = "http")]
use readabilities_rs::{ExecutionOutcome, ReadRequest, UrlPolicy};
use readabilities_rs::{
    ExtractionMode, ExtractionOptions, OutputFormat, ReadError, Reader, SiteConfig,
    parse_site_configs, version,
};
use serde::Serialize;
use url::Url;

#[derive(Debug, Parser)]
#[command(
    name = "readabilities-rs",
    version = version(),
    about = "Extract readable content; clean HTML first, render Markdown later"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Extract from an HTML file or stdin without network access.
    Extract {
        /// HTML path, or - for stdin.
        #[arg(default_value = "-")]
        input: PathBuf,
        /// Base URL used only to resolve relative links and metadata.
        #[arg(long)]
        url: Option<Url>,
        #[arg(long, value_enum, default_value = "json")]
        format: CliFormat,
        /// Shorthand for --format json.
        #[arg(long)]
        json: bool,
        #[arg(long, value_enum, default_value = "balanced")]
        mode: CliMode,
        /// Emit the complete Execution report instead of only the Article.
        #[arg(long)]
        debug: bool,
        #[arg(long)]
        no_images: bool,
        #[arg(long)]
        no_replies: bool,
        /// Load one or more declarative site configurations from JSON.
        #[arg(
            long = "site-config",
            value_name = "PATH",
            conflicts_with = "no_site_configs"
        )]
        site_configs: Vec<PathBuf>,
        /// Disable built-in site configurations and use only generic extraction.
        #[arg(long)]
        no_site_configs: bool,
    },
    /// Acquire a URL using an explicitly selected backend, then extract.
    Read {
        url: Url,
        #[arg(long, value_enum, default_value = "json")]
        format: CliFormat,
        #[arg(long)]
        json: bool,
        #[arg(long, value_enum, default_value = "balanced")]
        mode: CliMode,
        #[arg(long)]
        debug: bool,
        /// Load one or more declarative site configurations from JSON.
        #[arg(
            long = "site-config",
            value_name = "PATH",
            conflicts_with = "no_site_configs"
        )]
        site_configs: Vec<PathBuf>,
        /// Disable built-in site configurations and use only generic extraction.
        #[arg(long)]
        no_site_configs: bool,
    },
    /// Report compiled capabilities and local runtime prerequisites.
    Doctor {
        #[arg(long)]
        json: bool,
    },
    /// Print version and JSON schema version.
    Version {
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliFormat {
    Json,
    Html,
    Markdown,
    Text,
}

impl From<CliFormat> for OutputFormat {
    fn from(value: CliFormat) -> Self {
        match value {
            CliFormat::Json => Self::Json,
            CliFormat::Html => Self::Html,
            CliFormat::Markdown => Self::Markdown,
            CliFormat::Text => Self::Text,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliMode {
    Balanced,
    Conservative,
    Aggressive,
    Ensemble,
}

impl From<CliMode> for ExtractionMode {
    fn from(value: CliMode) -> Self {
        match value {
            CliMode::Balanced => Self::Balanced,
            CliMode::Conservative => Self::Conservative,
            CliMode::Aggressive => Self::Aggressive,
            CliMode::Ensemble => Self::Ensemble,
        }
    }
}

#[derive(Debug, Serialize)]
struct DoctorReport {
    schema_version: u32,
    version: String,
    core: bool,
    http: bool,
    ensemble: bool,
    providers: bool,
    external_processes: bool,
}

#[derive(Debug, Serialize)]
struct VersionReport {
    schema_version: u32,
    name: &'static str,
    version: String,
}

#[cfg(feature = "http")]
#[tokio::main]
async fn main() {
    let code = run_async(Cli::parse()).await;
    if code != 0 {
        std::process::exit(code);
    }
}

#[cfg(not(feature = "http"))]
fn main() {
    let code = run_without_http(Cli::parse());
    if code != 0 {
        std::process::exit(code);
    }
}

#[cfg(feature = "http")]
async fn run_async(cli: Cli) -> i32 {
    match cli.command {
        Command::Extract {
            input,
            url,
            format,
            json,
            mode,
            debug,
            no_images,
            no_replies,
            site_configs,
            no_site_configs,
        } => {
            let html = match read_input(&input) {
                Ok(html) => html,
                Err(error) => return print_io_error(&error),
            };
            let mut extraction = extraction_options(mode, no_images, no_replies);
            extraction.diagnostics = debug;
            let reader = match make_reader(extraction.clone(), &site_configs, no_site_configs) {
                Ok(reader) => reader,
                Err(message) => {
                    eprintln!("{message}");
                    return 2;
                }
            };
            if debug {
                let mut request = ReadRequest::html(html, url);
                request.extraction = extraction;
                print_execution(&reader.execute(request).await)
            } else {
                let output = if json {
                    OutputFormat::Json
                } else {
                    format.into()
                };
                match reader.extract_html(&html, url.as_ref()) {
                    Ok(article) => print_article(&article, output),
                    Err(error) => print_read_error(&error),
                }
            }
        }
        Command::Read {
            url,
            format,
            json,
            mode,
            debug,
            site_configs,
            no_site_configs,
        } => {
            let mut extraction = extraction_options(mode, false, false);
            extraction.diagnostics = debug;
            let policy = UrlPolicy::default();
            let reader = match make_reader(extraction.clone(), &site_configs, no_site_configs) {
                Ok(reader) => reader,
                Err(message) => {
                    eprintln!("{message}");
                    return 2;
                }
            };
            if debug {
                let mut request = ReadRequest::url(url);
                request.extraction = extraction;
                request.url_policy = policy;
                print_execution(&reader.execute(request).await)
            } else {
                let output = if json {
                    OutputFormat::Json
                } else {
                    format.into()
                };
                match reader.read_url(&url, &policy).await {
                    Ok(article) => print_article(&article, output),
                    Err(error) => print_read_error(&error),
                }
            }
        }
        Command::Doctor { json } => print_doctor(json),
        Command::Version { json } => print_version(json),
    }
}

#[cfg(not(feature = "http"))]
fn run_without_http(cli: Cli) -> i32 {
    match cli.command {
        Command::Extract {
            input,
            url,
            format,
            json,
            mode,
            debug,
            no_images,
            no_replies,
            site_configs,
            no_site_configs,
        } => {
            if debug {
                eprintln!("--debug requires the default `http` feature build");
                return 2;
            }
            let html = match read_input(&input) {
                Ok(html) => html,
                Err(error) => return print_io_error(&error),
            };
            let reader = match make_reader(
                extraction_options(mode, no_images, no_replies),
                &site_configs,
                no_site_configs,
            ) {
                Ok(reader) => reader,
                Err(message) => {
                    eprintln!("{message}");
                    return 2;
                }
            };
            let output = if json {
                OutputFormat::Json
            } else {
                format.into()
            };
            match reader.extract_html(&html, url.as_ref()) {
                Ok(article) => print_article(&article, output),
                Err(error) => print_read_error(&error),
            }
        }
        Command::Read { .. } => {
            eprintln!("read requires the `http` Cargo feature");
            2
        }
        Command::Doctor { json } => print_doctor(json),
        Command::Version { json } => print_version(json),
    }
}

fn extraction_options(mode: CliMode, no_images: bool, no_replies: bool) -> ExtractionOptions {
    ExtractionOptions {
        mode: mode.into(),
        include_images: !no_images,
        include_replies: !no_replies,
        ..ExtractionOptions::default()
    }
}

fn make_reader(
    options: ExtractionOptions,
    paths: &[PathBuf],
    no_site_configs: bool,
) -> Result<Reader, String> {
    if no_site_configs {
        return Ok(Reader::without_site_configs(options));
    }
    let mut configs = Vec::<SiteConfig>::new();
    for path in paths {
        let json = fs::read_to_string(path)
            .map_err(|error| format!("failed to read site config {}: {error}", path.display()))?;
        let mut loaded = parse_site_configs(&json)
            .map_err(|error| format!("failed to load site config {}: {error}", path.display()))?;
        configs.append(&mut loaded);
    }
    if configs.is_empty() {
        Ok(Reader::with_options(options))
    } else {
        Reader::with_options_and_site_configs(options, configs)
            .map_err(|error| format!("failed to configure reader: {error}"))
    }
}

fn read_input(path: &PathBuf) -> io::Result<String> {
    if path.as_os_str() == "-" {
        let mut input = String::new();
        io::stdin().read_to_string(&mut input)?;
        Ok(input)
    } else {
        fs::read_to_string(path)
    }
}

fn print_article(article: &readabilities_rs::Article, format: OutputFormat) -> i32 {
    match article.render(format) {
        Ok(output) => {
            println!("{output}");
            0
        }
        Err(error) => print_read_error(&error),
    }
}

#[cfg(feature = "http")]
fn print_execution(execution: &readabilities_rs::Execution) -> i32 {
    let failed = matches!(execution.outcome, ExecutionOutcome::Failure(_));
    match serde_json::to_string_pretty(&execution) {
        Ok(output) => println!("{output}"),
        Err(error) => {
            eprintln!("failed to serialize execution: {error}");
            return 1;
        }
    }
    i32::from(failed)
}

fn print_read_error(error: &ReadError) -> i32 {
    match serde_json::to_string(error) {
        Ok(output) => eprintln!("{output}"),
        Err(_) => eprintln!("{error}"),
    }
    1
}

fn print_io_error(error: &io::Error) -> i32 {
    eprintln!("failed to read HTML input: {error}");
    1
}

fn print_doctor(json: bool) -> i32 {
    let version = version();
    let report = DoctorReport {
        schema_version: readabilities_rs::SCHEMA_VERSION,
        version: version.clone(),
        core: true,
        http: cfg!(feature = "http"),
        ensemble: cfg!(feature = "ensemble"),
        providers: false,
        external_processes: false,
    };
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).unwrap_or_default()
        );
    } else {
        println!("readabilities-rs {version}");
        println!("core: ok");
        println!("http: {}", report.http);
        println!("ensemble: {}", report.ensemble);
        println!("providers: {}", report.providers);
        println!("external processes: {}", report.external_processes);
    }
    0
}

fn print_version(json: bool) -> i32 {
    if json {
        let report = VersionReport {
            schema_version: readabilities_rs::SCHEMA_VERSION,
            name: "readabilities-rs",
            version: version(),
        };
        println!("{}", serde_json::to_string(&report).unwrap_or_default());
    } else {
        println!("readabilities-rs {}", version());
    }
    0
}
