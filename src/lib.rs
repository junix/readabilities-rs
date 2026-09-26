//! Local-first readable-content extraction.
//!
//! The extraction pipeline selects and sanitizes clean HTML first. Markdown
//! and text are renderings of that canonical article, never inputs to content
//! selection.

#[cfg(feature = "charset")]
mod charset;
mod depth;
mod engine;
mod error;
#[cfg(feature = "http")]
mod http;
mod markdown;
mod model;
mod native;
mod page;
mod reader;
mod sanitize;
mod site;

pub use error::{ErrorKind, ReadError, Result, RetryAdvice};
pub use model::*;
pub use page::{DiscoveredLink, MetaRobots, PageAnalysis, PageSnapshot};
pub use reader::Reader;
pub use site::{SiteConfig, SiteConfigError, parse_site_configs};

/// Library version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Version stamped with the git build sha (ADR-1168): `<semver>+g<sha>` when the
/// justfile build provides `PM_BUILD_SHA` (`.dirty` appended for dirty trees),
/// bare semver otherwise.
pub fn version() -> String {
    match option_env!("PM_BUILD_SHA") {
        Some(sha) => format!("{VERSION}+{sha}"),
        None => VERSION.to_string(),
    }
}
