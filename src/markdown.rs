use std::rc::Rc;

use htmd::element_handler::{HandlerResult, Handlers};
use htmd::{Element, HtmlToMarkdown};
use markup5ever_rcdom::{Node, NodeData};

pub(crate) fn render(html: &str) -> std::io::Result<String> {
    let converter = HtmlToMarkdown::builder()
        .add_handler(vec!["code"], handle_code)
        .add_handler(vec!["math"], handle_math)
        .add_handler(vec!["sup", "sub"], handle_script)
        .add_handler(vec!["span", "div"], handle_semantic_container)
        .add_handler(vec!["a"], handle_anchor)
        .add_handler(vec!["img"], handle_image)
        .build();
    converter
        .convert(html)
        .map(|markdown| collapse_newlines(&clean_bare_bullets(&markdown)))
}

fn handle_code(handlers: &dyn Handlers, element: Element<'_>) -> Option<HandlerResult> {
    if parent_tag(element.node).as_deref() != Some("pre") {
        return handlers.fallback(element);
    }
    let language = attribute(&element, "data-lang")
        .or_else(|| language_class(&element))
        .map(|value| safe_info_string(&value))
        .filter(|value| !value.is_empty());
    let Some(language) = language else {
        return handlers.fallback(element);
    };

    let content = handlers.walk_children(element.node).content;
    let content = content.strip_suffix('\n').unwrap_or(&content);
    let fence = "`".repeat(longest_run(content, '`').saturating_add(1).max(3));
    Some(format!("{fence}{language}\n{content}\n{fence}").into())
}

#[expect(clippy::needless_pass_by_value, clippy::unnecessary_wraps)]
fn handle_math(_handlers: &dyn Handlers, element: Element<'_>) -> Option<HandlerResult> {
    let latex = attribute(&element, "data-latex")
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| mathml_to_latex(element.node));
    let latex = normalize_math(&latex);
    if latex.is_empty() {
        return Some(String::new().into());
    }
    let inline = parent_tag(element.node).is_some_and(|tag| {
        matches!(
            tag.as_str(),
            "p" | "span" | "a" | "li" | "td" | "th" | "figcaption"
        )
    });
    if inline {
        Some(format!("${latex}$").into())
    } else {
        Some(format!("\n\n$$\n{latex}\n$$\n\n").into())
    }
}

#[expect(clippy::needless_pass_by_value, clippy::unnecessary_wraps)]
fn handle_script(handlers: &dyn Handlers, element: Element<'_>) -> Option<HandlerResult> {
    if element.tag == "sup"
        && let Some(reference) = attribute(&element, "id")
            .and_then(|id| id.strip_prefix("fnref:").map(ToOwned::to_owned))
    {
        let base = reference.split('-').next().unwrap_or(&reference);
        return Some(format!("[^{base}]").into());
    }
    let content = handlers.walk_children(element.node).content;
    Some(format!("<{}>{}</{}>", element.tag, content.trim(), element.tag).into())
}

fn handle_semantic_container(
    handlers: &dyn Handlers,
    element: Element<'_>,
) -> Option<HandlerResult> {
    if let Some(latex) = attribute(&element, "data-latex")
        && !latex.trim().is_empty()
    {
        return if element.tag == "div" {
            Some(format!("\n\n$$\n{}\n$$\n\n", normalize_math(&latex)).into())
        } else {
            Some(format!("${}$", normalize_math(&latex)).into())
        };
    }

    if element.tag == "div" {
        if let Some(reference) =
            attribute(&element, "id").and_then(|id| id.strip_prefix("fn:").map(ToOwned::to_owned))
        {
            let base = reference.split('-').next().unwrap_or(&reference);
            let content = handlers.walk_children(element.node).content;
            return Some(format!("\n[^{base}]: {}\n", content.trim()).into());
        }
        if attribute(&element, "id").as_deref() == Some("footnotes") {
            return Some(handlers.walk_children(element.node));
        }
    }
    handlers.fallback(element)
}

fn handle_anchor(handlers: &dyn Handlers, element: Element<'_>) -> Option<HandlerResult> {
    if attribute(&element, "class").is_some_and(|classes| {
        classes
            .split_whitespace()
            .any(|class| class.contains("footnote-backref"))
    }) {
        return Some(String::new().into());
    }
    handlers.fallback(element)
}

fn handle_image(handlers: &dyn Handlers, element: Element<'_>) -> Option<HandlerResult> {
    let mut result = handlers.fallback(element)?;
    if !result.content.is_empty() && !result.content.ends_with(char::is_whitespace) {
        result.content.push(' ');
    }
    Some(result)
}

fn attribute(element: &Element<'_>, name: &str) -> Option<String> {
    element
        .attrs
        .iter()
        .find(|attribute| attribute.name.local.as_ref() == name)
        .map(|attribute| attribute.value.to_string())
}

fn language_class(element: &Element<'_>) -> Option<String> {
    attribute(element, "class").and_then(|classes| {
        classes
            .split_whitespace()
            .find_map(|class| class.strip_prefix("language-").map(ToOwned::to_owned))
    })
}

fn safe_info_string(value: &str) -> String {
    value
        .chars()
        .filter(|character| {
            character.is_alphanumeric() || matches!(character, '+' | '-' | '_' | '.')
        })
        .collect()
}

fn longest_run(value: &str, needle: char) -> usize {
    let mut longest = 0;
    let mut current = 0;
    for character in value.chars() {
        if character == needle {
            current += 1;
            longest = longest.max(current);
        } else {
            current = 0;
        }
    }
    longest
}

fn parent_tag(node: &Rc<Node>) -> Option<String> {
    let parent = node.parent.take();
    let upgraded = parent.as_ref().and_then(std::rc::Weak::upgrade);
    node.parent.set(parent);
    upgraded.as_ref().and_then(node_tag).map(ToOwned::to_owned)
}

fn node_tag(node: &Rc<Node>) -> Option<&str> {
    match &node.data {
        NodeData::Element { name, .. } => Some(name.local.as_ref()),
        _ => None,
    }
}

fn mathml_to_latex(node: &Rc<Node>) -> String {
    match &node.data {
        NodeData::Text { contents } => contents.borrow().trim().to_string(),
        NodeData::Element { name, attrs, .. } => {
            let tag = name.local.as_ref();
            if tag == "annotation"
                && !attrs.borrow().iter().any(|attribute| {
                    attribute.name.local.as_ref() == "encoding"
                        && attribute.value.to_ascii_lowercase().contains("tex")
                })
            {
                return String::new();
            }
            let children = node.children.borrow();
            let rendered = children
                .iter()
                .map(mathml_to_latex)
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>();
            match tag {
                "msup" if rendered.len() >= 2 => {
                    format!("{}^{{{}}}", rendered[0], rendered[1])
                }
                "msub" if rendered.len() >= 2 => {
                    format!("{}_{{{}}}", rendered[0], rendered[1])
                }
                "mfrac" if rendered.len() >= 2 => {
                    format!("\\frac{{{}}}{{{}}}", rendered[0], rendered[1])
                }
                "msqrt" => format!("\\sqrt{{{}}}", rendered.join(" ")),
                "mo" => rendered.join("").replace('−', "-").replace('×', "\\times"),
                "mi" | "mn" | "annotation" => rendered.join(""),
                _ => rendered.join(" "),
            }
        }
        _ => String::new(),
    }
}

fn normalize_math(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn clean_bare_bullets(markdown: &str) -> String {
    let lines = markdown.lines().collect::<Vec<_>>();
    let mut output = Vec::with_capacity(lines.len());
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        let next = lines.get(index + 1).map(|line| line.trim());
        if matches!(trimmed, "-" | "+" | "*")
            && (next.is_none()
                || next == Some("")
                || next.is_some_and(|line| matches!(line, "-" | "+" | "*")))
        {
            continue;
        }
        output.push(*line);
    }
    output.join("\n")
}

fn collapse_newlines(markdown: &str) -> String {
    let mut output = Vec::new();
    let mut in_fence = false;
    let mut blank_run = 0;
    for line in markdown.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
        }
        if line.is_empty() {
            blank_run += 1;
            if in_fence || blank_run <= 1 {
                output.push(line);
            }
        } else {
            blank_run = 0;
            output.push(line.trim_end());
        }
    }
    output.join("\n").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_language_math_scripts_and_image_boundaries() {
        let html = r#"<article><pre><code data-lang="rust">fn main() {}</code></pre>
            <math><mrow><msup><mi>a</mi><mn>2</mn></msup><mo>+</mo><mn>1</mn></mrow></math>
            <p><img src="https://example.test/x.png" alt="x">After</p>
            <p><sup>2</sup> note</p></article>"#;
        let markdown = render(html).unwrap();
        assert!(
            markdown.contains("```rust\nfn main() {}\n```"),
            "{markdown}"
        );
        assert!(markdown.contains("$$\na^{2} + 1\n$$"), "{markdown}");
        assert!(
            markdown.contains("](https://example.test/x.png) After"),
            "{markdown}"
        );
        assert!(markdown.contains("<sup>2</sup> note"), "{markdown}");
    }
}
