use std::fs;
use std::io::{self, Read};
use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
#[cfg(feature = "http")]
use readabilities_rs::{Acquisition, ExecutionOutcome, ReadRequest, UrlPolicy};
use readabilities_rs::{
    ExtractionMode, ExtractionOptions, OutputFormat, ReadError, Reader, SiteConfig, VERSION,
    parse_site_configs,
};
use serde::Serialize;
use url::Url;

#[derive(Debug, Parser)]
#[command(
    name = "readabilities-rs",
    version,
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
        #[arg(long, conflicts_with = "provider")]
        browser: bool,
        #[arg(long, value_enum, conflicts_with = "browser")]
        provider: Option<CliProvider>,
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

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliProvider {
    Jina,
    Firecrawl,
    Yxt,
}

#[derive(Debug, Serialize)]
struct DoctorReport {
    schema_version: u32,
    version: &'static str,
    core: bool,
    http: bool,
    ensemble: bool,
    providers: bool,
    browser: bool,
    browser_executable: Option<String>,
}

#[derive(Debug, Serialize)]
struct VersionReport {
    schema_version: u32,
    name: &'static str,
    version: &'static str,
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
            browser,
            provider,
            site_configs,
            no_site_configs,
        } => {
            let mut extraction = extraction_options(mode, false, false);
            extraction.diagnostics = debug;
            let mut policy = UrlPolicy::default();
            match configure_acquisition(&mut policy, browser, provider) {
                Ok(()) => {}
                Err(message) => {
                    eprintln!("{message}");
                    return 2;
                }
            }
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

#[cfg(feature = "http")]
fn configure_acquisition(
    policy: &mut UrlPolicy,
    browser: bool,
    provider: Option<CliProvider>,
) -> Result<(), String> {
    if browser {
        #[cfg(feature = "browser")]
        {
            policy.acquisition = Acquisition::Browser(readabilities_rs::BrowserPolicy::default());
            return Ok(());
        }
        #[cfg(not(feature = "browser"))]
        return Err("--browser requires a build with the `browser` feature".to_string());
    }

    let Some(provider) = provider else {
        policy.acquisition = Acquisition::Origin;
        return Ok(());
    };
    #[cfg(feature = "providers")]
    {
        use readabilities_rs::{FirecrawlConfig, JinaConfig, ManagedProvider, YxtConfig};
        use secrecy::SecretString;
        use std::time::Duration;

        policy.acquisition = Acquisition::Managed(match provider {
            CliProvider::Jina => {
                let config = JinaConfig {
                    api_key: std::env::var("JINA_API_KEY").ok().map(SecretString::from),
                    ..JinaConfig::default()
                };
                ManagedProvider::Jina(config)
            }
            CliProvider::Firecrawl => {
                let key = std::env::var("FIRECRAWL_API_KEY")
                    .map_err(|_| "FIRECRAWL_API_KEY is required".to_string())?;
                ManagedProvider::Firecrawl(FirecrawlConfig::new(SecretString::from(key)))
            }
            CliProvider::Yxt => {
                let endpoint = std::env::var("YXT_ENDPOINT")
                    .map_err(|_| "YXT_ENDPOINT is required".to_string())?;
                let authorization = std::env::var("YXT_AUTHORIZATION")
                    .map_err(|_| "YXT_AUTHORIZATION is required".to_string())?;
                ManagedProvider::Yxt(YxtConfig {
                    endpoint: Url::parse(&endpoint)
                        .map_err(|error| format!("invalid YXT_ENDPOINT: {error}"))?,
                    authorization: SecretString::from(authorization),
                    client: std::env::var("YXT_CLIENT").unwrap_or_else(|_| "AI_DIGGER".to_string()),
                    poll_interval: Duration::from_secs(10),
                })
            }
        });
        Ok(())
    }
    #[cfg(not(feature = "providers"))]
    {
        let _ = provider;
        Err("--provider requires a build with the `providers` feature".to_string())
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
    let report = DoctorReport {
        schema_version: readabilities_rs::SCHEMA_VERSION,
        version: VERSION,
        core: true,
        http: cfg!(feature = "http"),
        ensemble: cfg!(feature = "ensemble"),
        providers: cfg!(feature = "providers"),
        browser: cfg!(feature = "browser"),
        browser_executable: find_browser_executable(),
    };
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).unwrap_or_default()
        );
    } else {
        println!("readabilities-rs {VERSION}");
        println!("core: ok");
        println!("http: {}", report.http);
        println!("ensemble: {}", report.ensemble);
        println!("providers: {}", report.providers);
        println!("browser feature: {}", report.browser);
        println!(
            "browser executable: {}",
            report.browser_executable.as_deref().unwrap_or("not found")
        );
    }
    0
}

fn print_version(json: bool) -> i32 {
    if json {
        let report = VersionReport {
            schema_version: readabilities_rs::SCHEMA_VERSION,
            name: "readabilities-rs",
            version: VERSION,
        };
        println!("{}", serde_json::to_string(&report).unwrap_or_default());
    } else {
        println!("readabilities-rs {VERSION}");
    }
    0
}

fn find_browser_executable() -> Option<String> {
    let candidates = [
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
        "/usr/bin/google-chrome",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
    ];
    candidates
        .iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
        .map(|path| path.display().to_string())
}
