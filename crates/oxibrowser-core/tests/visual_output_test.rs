#[tokio::test]
async fn save_visual_output_for_inspection() {
    use oxibrowser_core::frame::Frame;
    use oxibrowser_core::js::dom_snapshot::DomSnapshot;
    use oxibrowser_core::css::{render_accessibility_tree, render_box_model_png};

    let html = r##"<html><head><title>Test Page</title></head><body>
        <h1>Welcome to My Site</h1>
        <p style="color:#333; font-size:14px">This is a description paragraph with some content.</p>
        <div style="width:400px; height:60px; background-color:#1a73e8; padding:10px">
            <button style="background-color:white; color:#1a73e8">Sign Up</button>
            <button disabled style="background-color:#ccc">Login</button>
        </div>
        <a href="/about" style="color:#1a73e8">Learn more about us</a>
        <img src="hero.jpg" alt="Hero image showing our product">
        <ul>
            <li>Feature 1</li>
            <li>Feature 2</li>
        </ul>
        <p style="display:none">This is hidden</p>
    </body></html>"##;

    let frame = Frame::from_html(
        url::Url::parse("http://test/").unwrap(),
        html,
    ).await.unwrap();

    let snapshot = DomSnapshot::from_frame(&frame);

    let tree = render_accessibility_tree(&snapshot);
    eprintln!("{}", tree);

    let png = render_box_model_png(&snapshot, 800).unwrap();
    std::fs::write("/tmp/oxibrowser_visual_test.png", &png).unwrap();
    eprintln!("PNG saved: {} bytes", png.len());
    assert!(png.len() > 500, "PNG should have meaningful content");
}
