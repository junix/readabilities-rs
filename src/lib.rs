//! Local-first readable-content extraction.
//!
//! The extraction pipeline selects and sanitizes clean HTML first. Markdown
//! and text are renderings of that canonical article, never inputs to content
//! selection.

#[cfg(feature = "http")]
mod charset;
mod engine;
mod error;
#[cfg(feature = "http")]
mod http;
mod markdown;
mod model;
mod native;
mod reader;
mod sanitize;
mod site;

pub use error::{ErrorKind, ReadError, Result, RetryAdvice};
pub use model::*;
pub use reader::Reader;
pub use site::{SiteConfig, SiteConfigError, parse_site_configs};

/// Library version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
