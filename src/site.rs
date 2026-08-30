use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteConfig {
    pub id: String,
    pub hosts: Vec<String>,
    #[serde(default)]
    pub path_prefixes: Vec<String>,
    pub content_selector: Option<String>,
    #[serde(default)]
    pub remove_selectors: Vec<String>,
}

impl SiteConfig {
    pub fn matches(&self, url: &Url) -> bool {
        let Some(host) = url.host_str() else {
            return false;
        };
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        let host_matches = self.hosts.iter().any(|configured| {
            let configured = configured.trim_end_matches('.').to_ascii_lowercase();
            host == configured || host.ends_with(&format!(".{configured}"))
        });
        host_matches
            && (self.path_prefixes.is_empty()
                || self
                    .path_prefixes
                    .iter()
                    .any(|prefix| url.path().starts_with(prefix)))
    }

    pub fn validate(&self) -> Result<(), SiteConfigError> {
        if self.id.trim().is_empty() {
            return Err(SiteConfigError::Invalid(
                "site id must not be empty".to_string(),
            ));
        }
        if self.hosts.is_empty() {
            return Err(SiteConfigError::Invalid(format!(
                "site {} must declare at least one host",
                self.id
            )));
        }
        for host in &self.hosts {
            if host.is_empty() || host.contains("//") || host.contains('/') || host.contains(':') {
                return Err(SiteConfigError::Invalid(format!(
                    "site {} has invalid host {host:?}",
                    self.id
                )));
            }
        }
        if let Some(selector) = &self.content_selector {
            validate_selector(&self.id, selector)?;
        }
        for selector in &self.remove_selectors {
            validate_selector(&self.id, selector)?;
        }
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum SiteConfigError {
    #[error("invalid site configuration JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid site configuration: {0}")]
    Invalid(String),
}

#[derive(Deserialize)]
#[serde(untagged)]
enum SiteConfigDocument {
    One(SiteConfig),
    Many(Vec<SiteConfig>),
    Wrapped { sites: Vec<SiteConfig> },
}

pub fn parse_site_configs(json: &str) -> Result<Vec<SiteConfig>, SiteConfigError> {
    let configs = match serde_json::from_str(json)? {
        SiteConfigDocument::One(config) => vec![config],
        SiteConfigDocument::Many(configs) | SiteConfigDocument::Wrapped { sites: configs } => {
            configs
        }
    };
    for config in &configs {
        config.validate()?;
    }
    Ok(configs)
}

pub(crate) fn builtins() -> Vec<SiteConfig> {
    vec![
        SiteConfig {
            id: "medium".to_string(),
            hosts: vec!["medium.com".to_string()],
            path_prefixes: Vec::new(),
            content_selector: Some("article".to_string()),
            remove_selectors: vec![
                "[data-testid=\"post-preview\"]".to_string(),
                "[data-testid*=\"Clap\"]".to_string(),
                "[data-testid*=\"Bookmark\"]".to_string(),
                "[data-testid*=\"Share\"]".to_string(),
                "[data-testid*=\"Response\"]".to_string(),
                "a[href*=\"medium.com/plans\"]".to_string(),
            ],
        },
        SiteConfig {
            id: "wikipedia".to_string(),
            hosts: vec!["wikipedia.org".to_string()],
            path_prefixes: vec!["/wiki/".to_string()],
            content_selector: Some("#mw-content-text".to_string()),
            remove_selectors: vec![
                ".mw-editsection".to_string(),
                ".navbox".to_string(),
                ".vertical-navbox".to_string(),
                ".metadata".to_string(),
                ".mw-jump-link".to_string(),
                ".printfooter".to_string(),
                ".catlinks".to_string(),
            ],
        },
        SiteConfig {
            id: "mdn".to_string(),
            hosts: vec!["developer.mozilla.org".to_string()],
            path_prefixes: vec!["/en-US/docs/".to_string()],
            content_selector: Some("main".to_string()),
            remove_selectors: vec![
                ".top-navigation-main".to_string(),
                ".sidebar".to_string(),
                ".page-footer".to_string(),
                ".article-footer".to_string(),
                ".prev-next".to_string(),
                ".survey".to_string(),
            ],
        },
    ]
}

pub(crate) fn find<'a>(configs: &'a [SiteConfig], url: Option<&Url>) -> Option<&'a SiteConfig> {
    let url = url?;
    configs.iter().find(|config| config.matches(url))
}

pub(crate) fn remove_configured_noise(
    html: &str,
    config: &SiteConfig,
) -> Result<String, SiteConfigError> {
    config.validate()?;
    if config.remove_selectors.is_empty() {
        return Ok(html.to_string());
    }
    let mut document = Html::parse_document(html);
    for raw_selector in &config.remove_selectors {
        let selector = Selector::parse(raw_selector).map_err(|error| {
            SiteConfigError::Invalid(format!(
                "site {} selector {raw_selector:?}: {error}",
                config.id
            ))
        })?;
        let ids = document
            .select(&selector)
            .map(|element| element.id())
            .collect::<Vec<_>>();
        for id in ids {
            if let Some(mut node) = document.tree.get_mut(id) {
                node.detach();
            }
        }
    }
    Ok(document.html())
}

fn validate_selector(site: &str, selector: &str) -> Result<(), SiteConfigError> {
    Selector::parse(selector).map(|_| ()).map_err(|error| {
        SiteConfigError::Invalid(format!("site {site} selector {selector:?}: {error}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_matching_is_boundary_aware() {
        let config = &builtins()[0];
        assert!(config.matches(&Url::parse("https://medium.com/post").unwrap()));
        assert!(config.matches(&Url::parse("https://team.medium.com/post").unwrap()));
        assert!(!config.matches(&Url::parse("https://notmedium.com/post").unwrap()));
    }

    #[test]
    fn configured_noise_is_removed_before_generic_extraction() {
        let config = &builtins()[0];
        let output = remove_configured_noise(
            r#"<article><p>keep</p><div data-testid="post-preview">remove</div></article>"#,
            config,
        )
        .unwrap();
        assert!(output.contains("keep"));
        assert!(!output.contains("remove"));
    }

    fn base_config() -> SiteConfig {
        SiteConfig {
            id: "demo".to_string(),
            hosts: vec!["example.test".to_string()],
            path_prefixes: Vec::new(),
            content_selector: None,
            remove_selectors: Vec::new(),
        }
    }

    #[test]
    fn invalid_site_configs_are_rejected_with_specific_messages() {
        for (config, expected) in [
            (
                SiteConfig {
                    id: "  ".to_string(),
                    ..base_config()
                },
                "site id must not be empty",
            ),
            (
                SiteConfig {
                    hosts: Vec::new(),
                    ..base_config()
                },
                "site demo must declare at least one host",
            ),
            (
                SiteConfig {
                    hosts: vec![String::new()],
                    ..base_config()
                },
                "site demo has invalid host \"\"",
            ),
            (
                SiteConfig {
                    hosts: vec!["example.test/path".to_string()],
                    ..base_config()
                },
                "site demo has invalid host \"example.test/path\"",
            ),
            (
                SiteConfig {
                    hosts: vec!["example.test:8080".to_string()],
                    ..base_config()
                },
                "site demo has invalid host \"example.test:8080\"",
            ),
            (
                SiteConfig {
                    hosts: vec!["https://example.test".to_string()],
                    ..base_config()
                },
                "site demo has invalid host \"https://example.test\"",
            ),
            (
                SiteConfig {
                    content_selector: Some("main[".to_string()),
                    ..base_config()
                },
                "site demo selector \"main[\"",
            ),
            (
                SiteConfig {
                    remove_selectors: vec!["div[".to_string()],
                    ..base_config()
                },
                "site demo selector \"div[\"",
            ),
        ] {
            let error = config
                .validate()
                .err()
                .unwrap_or_else(|| panic!("config must be rejected: {expected}"));
            let SiteConfigError::Invalid(message) = &error else {
                panic!("expected Invalid, got {error}");
            };
            assert!(message.contains(expected), "{expected} not in {message}");
        }
        // The baseline config stays valid.
        base_config().validate().unwrap();
    }

    #[test]
    fn site_config_documents_accept_single_many_and_wrapped_shapes() {
        for json in [
            r#"{"id":"a","hosts":["a.test"]}"#,
            r#"[{"id":"a","hosts":["a.test"]}]"#,
            r#"{"sites":[{"id":"a","hosts":["a.test"]}]}"#,
        ] {
            let configs = parse_site_configs(json).unwrap();
            assert_eq!(configs.len(), 1, "{json}");
            assert_eq!(configs[0].id, "a", "{json}");
            assert_eq!(configs[0].hosts, ["a.test"], "{json}");
        }
        // Malformed JSON and structurally-valid-but-invalid configs are both
        // rejected before any reader is built.
        assert!(matches!(
            parse_site_configs("not json"),
            Err(SiteConfigError::Json(_))
        ));
        assert!(matches!(
            parse_site_configs(r#"{"id":"a","hosts":[]}"#),
            Err(SiteConfigError::Invalid(_))
        ));
    }
}
