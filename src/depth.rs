//! Lexical nesting-depth screen for HTML conversion (dsh tool-web port).
//!
//! DOM construction and markdown conversion walk the element tree, and their
//! cost grows superlinearly with nesting depth — measured in dsh: depth 512
//! ≈ 0.15s, 20,000 ≈ 5s of synchronous work. The screen runs BEFORE any
//! parser sees the input (the `engine::extract` entry), so a deeply nested
//! bomb fails fast with a structured error instead of burning the budget in
//! the tree builder.

/// Conversion-depth ceiling: real pages nest a few dozen levels; 512 is far
/// above content and far below weaponizable. A robustness invariant, not a
/// tunable.
pub(crate) const MAX_CONVERSION_DEPTH: usize = 512;

/// Elements that never take a closing tag, so they do not grow the lexical
/// stack.
const VOID_ELEMENTS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

/// Elements whose contents HTML parses as text until their matching end tag.
const RAW_TEXT_ELEMENTS: &[&str] = &["script", "style", "noscript"];

/// Conservatively reject HTML whose lexical element stack crosses the
/// conversion-depth ceiling. The single pass skips raw-text bodies, respects
/// quoted `>` characters, and only accepts a closing tag for the current
/// element. Opening tags inside comments still count and unterminated tags
/// accumulate — malformed input over-counts rather than hiding nesting.
pub(crate) fn exceeds_depth(html: &str) -> bool {
    let bytes = html.as_bytes();
    // Names are compared lowercased (like dsh's `lowerHtml`); ASCII
    // lowercasing preserves every index, so both views share positions.
    let lower = html.to_ascii_lowercase();
    let lower_bytes = lower.as_bytes();
    let mut open_elements: Vec<&str> = Vec::new();
    let mut offset = 0usize;
    let mut in_comment = false;

    while offset < bytes.len() {
        let start = html[offset..].find('<').map(|relative| offset + relative);
        if in_comment {
            if let Some(end) = html[offset..].find("-->").map(|relative| offset + relative) {
                if start.is_none_or(|start| end < start) {
                    in_comment = false;
                    offset = end + 3;
                    continue;
                }
            }
        }
        let Some(start) = start else { break };
        if !in_comment && lower_bytes[start..].starts_with(b"<!--") {
            in_comment = true;
            offset = start + 4;
            continue;
        }

        let mut cursor = start + 1;
        let closing = bytes.get(cursor) == Some(&b'/');
        if closing {
            cursor += 1;
        }
        let name_start = cursor;
        while bytes
            .get(cursor)
            .is_some_and(|&b| b.is_ascii_alphanumeric() || b == b'-')
        {
            cursor += 1;
        }
        if cursor == name_start || !bytes[name_start].is_ascii_alphabetic() {
            offset = start + 1;
            continue;
        }

        let name = &lower[name_start..cursor];
        let mut quote: Option<u8> = None;
        while cursor < bytes.len() {
            let byte = bytes[cursor];
            cursor += 1;
            match quote {
                Some(q) if byte == q => quote = None,
                None if byte == b'"' || byte == b'\'' => quote = Some(byte),
                None if byte == b'>' => break,
                _ => {}
            }
        }
        if bytes.get(cursor - 1) != Some(&b'>') {
            break;
        }

        if closing {
            if !in_comment && open_elements.last() == Some(&name) {
                open_elements.pop();
            }
        } else {
            // The name's alphabetic tail bounds this walk, so `last` cannot
            // slide past the tag's start.
            let mut last = cursor - 2;
            while bytes.get(last).is_some_and(|&b| b.is_ascii_whitespace()) {
                last -= 1;
            }
            let self_closing = bytes.get(last) == Some(&b'/');
            if !VOID_ELEMENTS.contains(&name) && !self_closing {
                open_elements.push(name);
                if open_elements.len() > MAX_CONVERSION_DEPTH {
                    return true;
                }
                if !in_comment && RAW_TEXT_ELEMENTS.contains(&name) {
                    match find_raw_text_end(&lower, name, cursor) {
                        Some(end) => {
                            offset = end;
                            continue;
                        }
                        None => break,
                    }
                }
            }
        }
        offset = cursor;
    }
    false
}

/// Whether a byte can follow a raw-text end-tag name and still terminate it.
fn is_tag_boundary(byte: Option<&u8>) -> bool {
    match byte {
        None | Some(b'>' | b'/') => true,
        Some(&b) => b.is_ascii_whitespace(),
    }
}

/// Find the matching raw-text end tag without interpreting markup-like body
/// text. Returns the index of the `</name` start, or `None` when unterminated.
fn find_raw_text_end(lower: &str, name: &str, from: usize) -> Option<usize> {
    let bytes = lower.as_bytes();
    let mut search = from;
    while let Some(relative) = lower[search..].find("</") {
        let candidate = search + relative;
        if bytes[candidate + 2..].starts_with(name.as_bytes())
            && is_tag_boundary(bytes.get(candidate + 2 + name.len()))
        {
            return Some(candidate);
        }
        search = candidate + 2;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nested(depth: usize) -> String {
        "<div>".repeat(depth)
    }

    #[test]
    fn flat_document_is_within_budget() {
        assert!(!exceeds_depth(
            "<html><body><p>hi</p><p>there</p></body></html>"
        ));
    }

    #[test]
    fn depth_at_the_ceiling_passes_and_one_more_fails() {
        assert!(!exceeds_depth(&nested(MAX_CONVERSION_DEPTH)));
        assert!(exceeds_depth(&nested(MAX_CONVERSION_DEPTH + 1)));
    }

    #[test]
    fn unterminated_tag_scan_stops_cleanly() {
        assert!(!exceeds_depth("<div><p>never closed"));
        assert!(!exceeds_depth("<div attr=\"unterminated"));
    }

    #[test]
    fn plain_comments_close_properly() {
        assert!(!exceeds_depth("<!-- a plain comment --><p>ok</p>"));
    }

    #[test]
    fn opening_tags_inside_comments_still_over_count() {
        // The pass only ignores CLOSING tags inside comments; opening tags
        // still push, so hostile nesting cannot hide behind a comment
        // (malformed input over-counts rather than under-counts).
        let html = format!("<!-- {} --><p>real content</p>", nested(600));
        assert!(exceeds_depth(&html));
    }

    #[test]
    fn markup_inside_raw_text_bodies_is_ignored() {
        // 600 nested divs inside a script body never reach the stack.
        let html = format!("<script>{}</script><p>ok</p>", nested(600));
        assert!(!exceeds_depth(&html));
        // Unterminated raw text ends the scan at its open tag.
        assert!(!exceeds_depth(&format!("<style>{}", nested(600))));
        // Markup-like body text does not fool the end-tag search.
        assert!(!exceeds_depth("<script>if (a</script-like) {}</script>"));
    }

    #[test]
    fn quoted_greater_than_does_not_end_the_tag() {
        // Without quote handling the `>` inside the attribute would split one
        // open tag into many phantom ones.
        let html = format!(r#"<div data-x="a>b<c">{}</div>"#, nested(100));
        assert!(!exceeds_depth(&html));
    }

    #[test]
    fn void_and_self_closing_elements_do_not_grow_the_stack() {
        assert!(!exceeds_depth(&"<br>".repeat(10_000)));
        assert!(!exceeds_depth(&"<img/>".repeat(10_000)));
        assert!(!exceeds_depth(&"<div />".repeat(10_000)));
    }

    #[test]
    fn mismatched_closing_tags_are_ignored() {
        // `</div>` while span is open does not pop; only a matching close pops.
        assert!(!exceeds_depth("<div><span></div></span>"));
        // Even repeated mismatched closes cannot deflate a real stack.
        let html = format!("{}{}</div>", nested(600), "</span>".repeat(600));
        assert!(exceeds_depth(&html));
    }

    #[test]
    fn case_insensitive_names_pair_correctly() {
        assert!(!exceeds_depth("<DIV><SPAN></span></div>"));
    }

    #[test]
    fn end_tag_boundaries_are_respected() {
        // `</scriptx>` is not a script terminator; the scan stops at EOF.
        assert!(!exceeds_depth("<script></scriptx>"));
        // `</script data-x>` is a valid terminator (whitespace boundary).
        assert!(!exceeds_depth("<script>body</script data-x><p>ok</p>"));
    }

    #[test]
    fn non_tag_brackets_are_skipped() {
        assert!(!exceeds_depth("3 < 5 and 7 > 2 <p>ok</p>"));
        assert!(!exceeds_depth("<!DOCTYPE html><p>ok</p>"));
    }
}
