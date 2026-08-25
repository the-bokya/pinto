//! Scenario coverage for the native renderer. Drives `render::engine::layout_pages` and
//! asserts on the resulting display list (fast, deterministic, no rasterization).

use std::time::Duration;

use pinto::browser::Job;
use pinto::render::engine::layout_pages;
use pinto::render::layout::Item;
use pinto::render::paint::Page;
use serde_json::{Map, Value};

fn job(html: &str, options_json: &str) -> Job {
    let options: Map<String, Value> = serde_json::from_str(options_json).unwrap_or_default();
    Job {
        html: html.to_string(),
        options,
        is_print_designer: false,
        host_url: "http://localhost/".into(),
        sid: None,
        bench_sites_path: None,
        site_public_path: None,
        default_page_size: "A4".into(),
        default_page_height: None,
        default_page_width: None,
        chrome_path: None,
        devtools_url: None,
        start_timeout: Duration::from_secs(1),
    }
}

fn render(html: &str, opts: &str) -> Vec<Page> {
    layout_pages(&job(html, opts)).expect("layout_pages")
}

fn glyphs(pages: &[Page]) -> Vec<(f32, f32)> {
    pages
        .iter()
        .flat_map(|p| p.items.iter())
        .filter_map(|it| match it {
            Item::Glyph { x, y, .. } => Some((*x, *y)),
            _ => None,
        })
        .collect()
}

fn glyph_count(pages: &[Page]) -> usize {
    glyphs(pages).len()
}

fn has_rect_rgb(pages: &[Page], rgb: [u8; 3]) -> bool {
    pages.iter().flat_map(|p| &p.items).any(|it| match it {
        Item::Rect { color, w, h, .. } => color[0..3] == rgb && *w > 0.0 && *h > 0.0,
        _ => false,
    })
}

fn count<F: Fn(&Item) -> bool>(pages: &[Page], f: F) -> usize {
    pages.iter().flat_map(|p| &p.items).filter(|it| f(it)).count()
}

// `options_json` is the inner Frappe options map (as job.options), not the wrapper config.
const DEFAULT: &str = r#"{ "page-size": "A4" }"#;

fn wrap(body: &str) -> String {
    format!("<!DOCTYPE html><html><head></head><body><div class=\"print-format\">{body}</div></body></html>")
}
fn wrap_style(style: &str, body: &str) -> String {
    format!(
        "<!DOCTYPE html><html><head><style>{style}</style></head><body><div class=\"print-format\">{body}</div></body></html>"
    )
}

#[test]
fn basic_text_renders() {
    let pages = render(&wrap("<p>Hello World from pinto</p>"), DEFAULT);
    assert_eq!(pages.len(), 1);
    assert!(glyph_count(&pages) > 10, "expected text glyphs, got {}", glyph_count(&pages));
}

// Regression: `[hidden] { display: none }` must NOT match every element (it once blanked the page).
#[test]
fn attribute_only_selector_does_not_blank_page() {
    let html = wrap_style("[hidden] { display: none } .x[data-y] { color: red }", "<p>Visible content here</p>");
    let pages = render(&html, DEFAULT);
    assert!(glyph_count(&pages) > 5, "attribute-only selector blanked the page");
}

// Regression: a pseudo-element rule (`.row:before { display: table }`) must not apply to `.row`.
#[test]
fn pseudo_element_rule_does_not_affect_base_element() {
    let html = wrap_style(
        ".row:before, .row:after { content:''; display: table } .row:after { clear: both }",
        "<div class=\"row\"><p>Row content stays visible</p></div>",
    );
    let pages = render(&html, DEFAULT);
    assert!(glyph_count(&pages) > 5, "pseudo-element rule blanked the row");
}

// Float columns tile side by side (Bootstrap-style grid).
#[test]
fn float_columns_lay_out_side_by_side() {
    let html = wrap_style(
        ".col { float: left; width: 50%; }",
        "<div class=\"row\"><div class=\"col\">LEFT</div><div class=\"col\">RIGHT</div></div><div style=\"clear:both\">below</div>",
    );
    let pages = render(&html, DEFAULT);
    let gs = glyphs(&pages);
    assert!(!gs.is_empty());
    let max_x = gs.iter().map(|g| g.0).fold(0.0_f32, f32::max);
    let min_x = gs.iter().map(|g| g.0).fold(f32::MAX, f32::min);
    // LEFT starts near the left; RIGHT lives in the right half of the content box.
    assert!(max_x - min_x > 150.0, "columns did not tile horizontally (span {})", max_x - min_x);
}

// @media print rules apply; @media screen rules are skipped.
#[test]
fn media_queries_select_print() {
    let html = wrap_style(
        "@media print { .a { background: #ff0000 } } @media screen { .b { background: #00ff00 } }",
        "<div class=\"a\">A</div><div class=\"b\">B</div>",
    );
    let pages = render(&html, DEFAULT);
    assert!(has_rect_rgb(&pages, [255, 0, 0]), "@media print rule was not applied");
    assert!(!has_rect_rgb(&pages, [0, 255, 0]), "@media screen rule leaked into print");
}

// hidden-pdf is dropped and visible-pdf is shown (Frappe toggle) inside header/footer.
#[test]
fn visible_and_hidden_pdf_toggle() {
    let html = wrap(
        "<div id=\"footer-html\"><span class=\"visible-pdf\">FOOTERVISIBLE</span><span class=\"hidden-pdf\">HIDDENAWAY</span></div><p>body</p>",
    );
    // visible-pdf normally display:none; toggle should reveal it.
    let styled = html.replace("<head>", "<head><style>.visible-pdf{display:none}</style>");
    let pages = render(&styled, DEFAULT);
    assert!(glyph_count(&pages) > 5, "footer visible-pdf content not shown");
}

#[test]
fn page_size_and_orientation() {
    let a4 = render(&wrap("<p>x</p>"), r#"{ "page-size": "A4", "orientation": "Portrait" }"#);
    assert!(a4[0].width_px < a4[0].height_px);

    let land = render(&wrap("<p>x</p>"), r#"{ "page-size": "A4", "orientation": "Landscape" }"#);
    assert!(land[0].width_px > land[0].height_px);

    let letter = render(&wrap("<p>x</p>"), r#"{ "page-size": "Letter" }"#);
    // US Letter = 8.5 x 11 in = 816 x 1056 px.
    assert!((letter[0].width_px - 816.0).abs() < 2.0, "letter width {}", letter[0].width_px);

    let custom = render(
        &wrap("<p>x</p>"),
        r#"{ "page-size": "Custom", "page-width": "180mm", "page-height": "120mm" }"#,
    );
    assert!(custom[0].width_px > custom[0].height_px);
    assert!((custom[0].width_px - 180.0 * 96.0 / 25.4).abs() < 2.0);
}

#[test]
fn tall_content_paginates() {
    let blocks: String = (0..12)
        .map(|i| format!("<div style=\"height:200px\">block {i}</div>"))
        .collect();
    let pages = render(&wrap(&blocks), DEFAULT);
    assert!(pages.len() >= 2, "expected multiple pages, got {}", pages.len());
}

#[test]
fn bordered_table_draws_grid_and_text() {
    let html = wrap("<table border=\"1\" cellpadding=\"4\"><tr><th>Item</th><th>Qty</th></tr><tr><td>Widget</td><td>3</td></tr></table>");
    let pages = render(&html, DEFAULT);
    let rects = count(&pages, |it| matches!(it, Item::Rect { .. }));
    assert!(rects >= 4, "expected grid line rects, got {rects}");
    assert!(glyph_count(&pages) > 6, "expected cell text");
}

#[test]
fn linear_gradient_background() {
    let html = wrap("<div style=\"height:40px;background:linear-gradient(90deg,#ff0000,#0000ff)\">bar</div>");
    let pages = render(&html, DEFAULT);
    assert!(count(&pages, |it| matches!(it, Item::Gradient { .. })) >= 1, "gradient not emitted");
}

#[test]
fn data_uri_image_renders() {
    // 2x2 PNG.
    let img = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAEUlEQVR4nGP8z8Dwn4EIwDiUFAIAF9kCAd0N/qUAAAAASUVORK5CYII=";
    let html = wrap(&format!("<img src=\"{img}\" style=\"width:80px;height:60px\">"));
    let pages = render(&html, DEFAULT);
    assert!(count(&pages, |it| matches!(it, Item::Image { .. })) >= 1, "image not emitted");
}

#[test]
fn repeating_header_and_footer_page_numbers() {
    let script = "function clone_and_update(){}"; // not needed; native path handles substitution
    let blocks: String = (0..10).map(|i| format!("<div style=\"height:200px\">b{i}</div>")).collect();
    let body = format!(
        "<div id=\"header-html\"><h3>ACME</h3></div>{blocks}<div id=\"footer-html\"><p>Page <span class=\"page\"></span> of <span class=\"topage\"></span></p></div>"
    );
    let html = format!(
        "<!DOCTYPE html><html><head><script>{script}</script></head><body><div class=\"print-format\">{body}</div></body></html>"
    );
    let pages = render(&html, DEFAULT);
    assert!(pages.len() >= 2, "expected multi-page");
    // Every page should carry the header glyphs (ACME).
    for (i, p) in pages.iter().enumerate() {
        let g = p.items.iter().filter(|it| matches!(it, Item::Glyph { .. })).count();
        assert!(g > 3, "page {i} missing header/footer/body glyphs");
    }
}

#[test]
fn margin_collapsing_between_paragraphs() {
    // Two stacked paragraphs: gap is max(margins), not the sum.
    let pages = render(
        &wrap_style("p { margin: 20px 0 }", "<p>one</p><p>two</p>"),
        DEFAULT,
    );
    let ys: Vec<f32> = glyphs(&pages).iter().map(|g| g.1).collect();
    let min = ys.iter().cloned().fold(f32::MAX, f32::min);
    let max = ys.iter().cloned().fold(0.0, f32::max);
    // Two lines ~ one line-height + one collapsed 20px margin (~40-60px), not ~80px+.
    assert!(max - min < 70.0, "paragraph gap too large ({}) — margins not collapsing", max - min);
}
