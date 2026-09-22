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
