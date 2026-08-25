//! Top-level: HTML + options -> PDF, fully in-process (no browser).

use anyhow::Result;
use kuchikiki::NodeRef;
use kuchikiki::traits::*;

use crate::browser::Job;
use crate::css;
use crate::options::{self, opt_str};

use super::fonts::Fonts;
use super::layout::{Engine, Flow, Item};
use super::paint::{self, Page};
use super::style::{ComputedStyle, Stylesheet, parse_stylesheet, ua_stylesheet};

const MM_TO_PX: f32 = 96.0 / 25.4;

struct Geometry {
    page_w: f32,
    page_h: f32,
    margin_top: f32,
    margin_right: f32,
    margin_bottom: f32,
    margin_left: f32,
}

/// HTML + options -> PDF bytes.
pub fn render(job: &Job) -> Result<Vec<u8>> {
    let (pages, mut fonts) = build(job)?;
    paint::paint(&pages, &mut fonts)
}

/// Lay out the document into a page display list without painting (for tests/inspection).
pub fn layout_pages(job: &Job) -> Result<Vec<Page>> {
    Ok(build(job)?.0)
}

fn build(job: &Job) -> Result<(Vec<Page>, Fonts)> {
    super::image::set_resolver(job.host_url.clone(), job.site_public_path.clone(), job.bench_sites_path.clone());

    let document = kuchikiki::parse_html().one(job.html.as_str());
    let stylesheet_text = collect_styles(&document);

    let mut fonts = Fonts::new();

    // Extract repeating header/footer before geometry (their presence changes margins).
    let header = take_by_id(&document, "header-html");
    let footer = take_by_id(&document, "footer-html");
    // Emulate Frappe's toggle_visible_pdf: unhide `visible-pdf` (e.g. the footer page number)
    // and drop `hidden-pdf` content, since we render for print media.
    for node in [header.as_ref(), footer.as_ref()].into_iter().flatten() {
        crate::html::toggle_visible_pdf(node);
    }

    // Chrome only applies the implicit 15mm top/bottom margin when there is no header/footer;
    // with one present the header/footer sit flush and provide the spacing.
    let geom = geometry(job, &stylesheet_text, header.is_some(), footer.is_some());

    let ua = ua_stylesheet();
    let media = super::style::MediaCtx { print: true, width_px: geom.page_w };
    let author = parse_stylesheet(&stylesheet_text, false, 100_000, &media);
    let sheets: Vec<&Stylesheet> = vec![&ua, &author];

    let root_node = document
        .select_first(".print-format")
        .map(|n| n.as_node().clone())
        .unwrap_or_else(|_| document.select_first("body").map(|n| n.as_node().clone()).unwrap_or(document.clone()));

    let content_w = (geom.page_w - geom.margin_left - geom.margin_right).max(1.0);
    let root_style = root_base_style(&root_node, &sheets);

    // Header / footer heights (footer laid out with placeholder page numbers).
    let header_flow = header.as_ref().map(|h| {
        let mut eng = Engine { fonts: &mut fonts, sheets: sheets.clone() };
        eng.layout_flow(h, &root_style, 0.0, 0.0, content_w)
    });
    let header_h = header_flow.as_ref().map(|f| f.height).unwrap_or(0.0);

    let footer_h = footer.as_ref().map(|f| {
        substitute_page_numbers(f, 1, 1);
        let mut eng = Engine { fonts: &mut fonts, sheets: sheets.clone() };
        eng.layout_flow(f, &root_style, 0.0, 0.0, content_w).height
    }).unwrap_or(0.0);

    let content_top = geom.margin_top + header_h;
    let content_h = (geom.page_h - geom.margin_top - geom.margin_bottom - header_h - footer_h).max(1.0);

    let body = {
        let mut eng = Engine { fonts: &mut fonts, sheets: sheets.clone() };
        eng.layout_flow(&root_node, &root_style, 0.0, 0.0, content_w)
    };

    let slices = slice_pages(&body, content_h);
    let total = slices.len().max(1);
    let mut pages: Vec<Page> = Vec::new();

    for (i, (top, cut)) in slices.iter().enumerate() {
        let mut items: Vec<Item> = Vec::new();
        if let Some(hf) = &header_flow {
            items.extend(hf.items.iter().map(|it| it.translated(geom.margin_left, geom.margin_top)));
        }
        items.extend(
            body.items
                .iter()
                .filter(|it| overlaps(it, *top, *cut))
                .map(|it| it.translated(geom.margin_left, content_top - top)),
        );
        if let Some(f) = &footer {
            substitute_page_numbers(f, i + 1, total);
            let mut eng = Engine { fonts: &mut fonts, sheets: sheets.clone() };
            let ff = eng.layout_flow(f, &root_style, 0.0, 0.0, content_w);
            let foot_top = geom.page_h - geom.margin_bottom - footer_h;
            items.extend(ff.items.iter().map(|it| it.translated(geom.margin_left, foot_top)));
        }
        pages.push(Page { width_px: geom.page_w, height_px: geom.page_h, items });
    }
    if pages.is_empty() {
        pages.push(Page { width_px: geom.page_w, height_px: geom.page_h, items: vec![] });
    }

    Ok((pages, fonts))
}

/// Break the body flow into (top, bottom) y-slices no taller than `content_h`,
/// preferring page-break candidates at block/row boundaries.
fn slice_pages(body: &Flow, content_h: f32) -> Vec<(f32, f32)> {
    let mut breaks = body.breaks.clone();
    breaks.sort_by(|a, b| a.partial_cmp(b).unwrap());
    breaks.dedup();
    let total = body.height.max(1.0);

    let mut slices = Vec::new();
    let mut top = 0.0f32;
    while top < total - 0.5 {
        let limit = top + content_h;
        let cut = breaks
            .iter()
            .copied()
            .filter(|&b| b > top + 1.0 && b <= limit + 0.5)
            .fold(top, f32::max);
        let cut = if cut <= top { limit.min(total).max(top + 1.0) } else { cut };
        slices.push((top, cut));
        top = cut;
        if slices.len() > 5000 {
            break;
        }
    }
    if slices.is_empty() {
        slices.push((0.0, total));
    }
    slices
}

fn overlaps(item: &Item, top: f32, bottom: f32) -> bool {
    let (y0, y1) = match item {
        Item::Rect { y, h, .. } | Item::Gradient { y, h, .. } | Item::Image { y, h, .. } => (*y, y + h),
        Item::Glyph { y, size, .. } => (y - size, *y),
    };
    y1 > top - 0.5 && y0 < bottom + 0.5
}

fn take_by_id(document: &NodeRef, id: &str) -> Option<NodeRef> {
    let node = document.select_first(&format!("#{id}")).ok()?.as_node().clone();
    node.detach();
    Some(node)
}

fn set_text(node: &NodeRef, s: &str) {
    for child in node.children().collect::<Vec<_>>() {
        child.detach();
    }
    node.append(NodeRef::new_text(s));
}

fn substitute_page_numbers(footer: &NodeRef, page: usize, total: usize) {
    if let Ok(sel) = footer.select(".page") {
        for n in sel.collect::<Vec<_>>() {
            set_text(n.as_node(), &page.to_string());
        }
    }
    if let Ok(sel) = footer.select(".topage") {
        for n in sel.collect::<Vec<_>>() {
            set_text(n.as_node(), &total.to_string());
        }
    }
}

fn collect_styles(document: &NodeRef) -> String {
    let mut css = String::new();
    // External <link rel="stylesheet"> — resolve to disk and inline (e.g. print.bundle.css).
    if let Ok(links) = document.select("link") {
        for link in links {
            let attrs = link.attributes.borrow();
            let is_css = attrs.get("rel").map(|r| r.contains("stylesheet")).unwrap_or(false);
            if let (true, Some(href)) = (is_css, attrs.get("href"))
                && let Some(path) = super::image::resolve_url(href)
                    && let Ok(text) = std::fs::read_to_string(&path) {
                        css.push_str(&text);
                        css.push('\n');
                    }
        }
    }
    if let Ok(styles) = document.select("style") {
        for s in styles {
            css.push_str(&s.as_node().text_contents());
            css.push('\n');
        }
    }
    css
}

fn root_base_style(root: &NodeRef, sheets: &[&Stylesheet]) -> ComputedStyle {
    let mut style = ComputedStyle::initial();
    style.font_family = vec!["sans-serif".into()];
    let mut chain = Vec::new();
    let mut cur = Some(root.clone());
    while let Some(n) = cur {
        if n.as_element().is_some() {
            chain.push(n.clone());
        }
        cur = n.parent();
    }
    for node in chain.iter().rev() {
        style = super::style::compute(node, &style, sheets);
    }
    style
}

fn geometry(job: &Job, stylesheet_text: &str, has_header: bool, has_footer: bool) -> Geometry {
    let decls = css::print_format_declarations(stylesheet_text);
    let get = |k: &str| decls.iter().rev().find(|(n, _)| n == k).map(|(_, v)| v.clone());

    let page_size = get("page-size")
        .or_else(|| opt_str(&job.options, "page-size"))
        .filter(|s| !s.is_empty() && s != "Custom")
        .unwrap_or_else(|| if job.default_page_size.is_empty() { "A4".into() } else { job.default_page_size.clone() });

    let (w0, h0) = options::page_size_mm(&page_size).unwrap_or((210.0, 297.0));
    let (mut w_mm, mut h_mm) = (w0 as f32, h0 as f32);

    if let Some(pw) = get("page-width").or_else(|| opt_str(&job.options, "page-width"))
        && let Some(px) = super::style::length_px(&pw, 16.0, 16.0) {
            w_mm = px / MM_TO_PX;
        }
    if let Some(ph) = get("page-height").or_else(|| opt_str(&job.options, "page-height"))
        && let Some(px) = super::style::length_px(&ph, 16.0, 16.0) {
            h_mm = px / MM_TO_PX;
        }

    let landscape = get("orientation").or_else(|| opt_str(&job.options, "orientation")).as_deref() == Some("Landscape");
    if landscape {
        std::mem::swap(&mut w_mm, &mut h_mm);
    }

    let margin = |name: &str, default_mm: f32| -> f32 {
        get(name)
            .or_else(|| opt_str(&job.options, name))
            .and_then(|v| super::style::length_px(&v, 16.0, 16.0))
            .unwrap_or(default_mm * MM_TO_PX)
    };

    Geometry {
        page_w: w_mm * MM_TO_PX,
        page_h: h_mm * MM_TO_PX,
        margin_top: margin("margin-top", if has_header { 0.0 } else { 15.0 }),
        margin_right: margin("margin-right", 15.0),
        margin_bottom: margin("margin-bottom", if has_footer { 0.0 } else { 15.0 }),
        margin_left: margin("margin-left", 15.0),
    }
}
