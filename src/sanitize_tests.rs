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
