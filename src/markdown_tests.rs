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
