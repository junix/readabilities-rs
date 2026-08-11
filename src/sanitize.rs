use std::collections::HashSet;

use ammonia::Builder;

pub(crate) fn clean(html: &str) -> String {
    let mut builder = Builder::default();
    builder
        .add_tags(&[
            "main",
            "address",
            "math",
            "semantics",
            "annotation",
            "annotation-xml",
            "mrow",
            "mi",
            "mn",
            "mo",
            "msup",
            "msub",
            "mfrac",
            "msqrt",
        ])
        .add_generic_attributes(&[
            "class",
            "id",
            "lang",
            "dir",
            "role",
            "aria-label",
            "data-lang",
            "data-latex",
        ])
        .add_tag_attributes("a", &["href", "title", "name"])
        .add_tag_attributes(
            "img",
            &["src", "srcset", "sizes", "alt", "title", "width", "height"],
        )
        .add_tag_attributes("code", &["class", "data-lang"])
        .add_tag_attributes("pre", &["class"])
        .add_tag_attributes("time", &["datetime"])
        .add_tag_attributes("ol", &["start", "reversed", "type"])
        .add_tag_attributes("li", &["value"])
        .add_tag_attributes("td", &["colspan", "rowspan", "headers"])
        .add_tag_attributes("th", &["colspan", "rowspan", "headers", "scope"])
        .add_tag_attributes("annotation", &["encoding"])
        .add_tag_attributes("math", &["display", "alttext", "data-latex"])
        .url_schemes(HashSet::from(["http", "https", "mailto", "tel"]))
        .link_rel(Some("noopener noreferrer"));
    builder.clean(html).to_string()
}

pub(crate) fn violations(html: &str) -> Vec<&'static str> {
    let lowercase = html.to_ascii_lowercase();
    let mut found = Vec::new();
    if lowercase.contains("<script") {
        found.push("script_element");
    }
    if contains_event_handler(&lowercase) {
        found.push("event_handler_attribute");
    }
    if contains_dangerous_uri_in_tag(&lowercase) {
        found.push("dangerous_uri");
    }
    found
}

fn contains_dangerous_uri_in_tag(html: &str) -> bool {
    let mut remaining = html;
    while let Some(start) = remaining.find('<') {
        remaining = &remaining[start + 1..];
        let Some(end) = remaining.find('>') else {
            break;
        };
        let tag = &remaining[..end];
        if tag.contains("javascript:") || tag.contains("data:text/html") {
            return true;
        }
        remaining = &remaining[end + 1..];
    }
    false
}

fn contains_event_handler(html: &str) -> bool {
    [
        " onclick=",
        " onerror=",
        " onload=",
        " onmouseover=",
        " onfocus=",
        " onsubmit=",
    ]
    .iter()
    .any(|needle| html.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizer_removes_active_content_but_preserves_structure() {
        let input = r#"<article onclick="steal()"><h1>Title</h1><script>bad()</script>
            <a href="javascript:bad()">bad</a><img src="https://e.test/x.png" onerror="bad()">
            <p>JavaScript: this literal prose is safe.</p>
            <pre><code class="language-rust">fn main() {}</code></pre></article>"#;
        let output = clean(input);
        assert!(violations(&output).is_empty(), "{output}");
        assert!(output.contains("<h1>Title</h1>"));
        assert!(output.contains("language-rust"));
        assert!(output.contains("https://e.test/x.png"));
        assert!(output.contains("JavaScript: this literal prose is safe."));
    }
}
