//! Benchmarks for OxiBrowser core operations.
//!
//! Run with: cargo bench

use criterion::{Criterion, criterion_group, criterion_main};
use oxibrowser_webapi::Document;

fn bench_html_parsing(c: &mut Criterion) {
    let simple_html =
        r#"<html><head><title>Test</title></head><body><h1>Hello</h1><p>World</p></body></html>"#;
    let complex_html = include_str!("../benches/fixtures/complex.html");

    let mut group = c.benchmark_group("html_parsing");
    group.bench_function("simple", |b| b.iter(|| Document::parse(simple_html)));
    group.bench_function("complex", |b| b.iter(|| Document::parse(complex_html)));
    group.finish();
}

fn bench_dom_queries(c: &mut Criterion) {
    let html = r#"
    <html><body>
        <div id="main" class="container">
            <h1>Title</h1>
            <p class="text">Paragraph 1</p>
            <p class="text">Paragraph 2</p>
            <a href="https://example.com">Link</a>
            <ul><li>Item 1</li><li>Item 2</li><li>Item 3</li></ul>
        </div>
    </body></html>"#;

    let doc = Document::parse(html);

    let mut group = c.benchmark_group("dom_queries");
    group.bench_function("query_selector_id", |b| {
        b.iter(|| doc.query_selector("#main"))
    });
    group.bench_function("query_selector_tag", |b| {
        b.iter(|| doc.query_selector("h1"))
    });
    group.bench_function("query_selector_class", |b| {
        b.iter(|| doc.query_selector(".text"))
    });
    group.bench_function("query_selector_all_p", |b| {
        b.iter(|| doc.query_selector_all("p"))
    });
    group.bench_function("query_text", |b| b.iter(|| doc.query_text("h1")));
    group.finish();
}

fn bench_to_markdown(c: &mut Criterion) {
    let html = r#"
    <html><body>
        <article>
            <h1>Main Title</h1>
            <h2>Section 1</h2>
            <p>This is a paragraph with <strong>bold</strong> and <em>italic</em> text.</p>
            <ul>
                <li>Item 1</li>
                <li>Item 2</li>
                <li>Item 3</li>
            </ul>
            <h2>Section 2</h2>
            <p>Another paragraph with a <a href="https://example.com">link</a>.</p>
            <code>let x = 42;</code>
        </article>
    </body></html>"#;

    let doc = Document::parse(html);

    c.bench_function("to_markdown", |b| b.iter(|| doc.to_markdown()));
}

criterion_group!(
    benches,
    bench_html_parsing,
    bench_dom_queries,
    bench_to_markdown
);
criterion_main!(benches);
