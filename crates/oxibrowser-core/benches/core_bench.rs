//! Benchmarks for OxiBrowser core operations.
//!
//! Run with: cargo bench

use criterion::{Criterion, criterion_group, criterion_main};
use oxibrowser_core::frame::Frame;
use oxibrowser_core::js::dom_snapshot::DomSnapshot;

fn bench_html_parsing(c: &mut Criterion) {
    let simple_html =
        r#"<html><head><title>Test</title></head><body><h1>Hello</h1><p>World</p></body></html>"#;
    let complex_html = include_str!("../benches/fixtures/complex.html");

    let mut group = c.benchmark_group("html_parsing");
    let url = url::Url::parse("https://example.com/").unwrap();
    group.bench_function("simple", |b| {
        b.iter(|| {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(Frame::from_html(url.clone(), simple_html)).unwrap()
        })
    });
    group.bench_function("complex", |b| {
        b.iter(|| {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(Frame::from_html(url.clone(), complex_html)).unwrap()
        })
    });
    group.finish();
}

fn build_snapshot(html: &str) -> (Frame, DomSnapshot) {
    let url = url::Url::parse("https://example.com/").unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let frame = rt.block_on(Frame::from_html(url, html)).unwrap();
    let snap = frame.document().clone();
    (frame, snap)
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

    let (_frame, snap) = build_snapshot(html);

    let mut group = c.benchmark_group("dom_queries");
    group.bench_function("query_selector_id", |b| b.iter(|| snap.query_selector("#main")));
    group.bench_function("query_selector_tag", |b| b.iter(|| snap.query_selector("h1")));
    group.bench_function("query_selector_class", |b| {
        b.iter(|| snap.query_selector(".text"))
    });
    group.bench_function("query_selector_all_p", |b| {
        b.iter(|| snap.query_selector_all("p"))
    });
    group.bench_function("query_text", |b| b.iter(|| snap.query_selector("h1")));
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

    let (_frame, snap) = build_snapshot(html);

    c.bench_function("to_markdown", |b| {
        b.iter(|| oxibrowser_core::css::render_to_markdown(&snap))
    });
}

criterion_group!(
    benches,
    bench_html_parsing,
    bench_dom_queries,
    bench_to_markdown
);
criterion_main!(benches);
