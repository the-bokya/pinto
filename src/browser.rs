//! Orchestrates a PDF: launches/attaches Chrome, renders body/header/footer, computes
//! the printToPDF options, drives page numbering, and merges. Ports
//! frappe/utils/pdf_generator/browser.py.

use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use serde_json::{Map, Value, json};

use crate::cdp::chrome::Chromium;
use crate::cdp::connection::CdpClient;
use crate::cdp::page::{Interceptor, Page};
use crate::html::{self, Html};
use crate::merge::{self, MergeInput};
use crate::options::{self, opt_bool, opt_num, opt_str};

/// A single PDF job: rendered HTML + options + ambient config.
pub struct Job {
    pub html: String,
    pub options: Map<String, Value>,
    pub is_print_designer: bool,
    pub host_url: String,
    pub sid: Option<String>,
    pub bench_sites_path: Option<String>,
    pub site_public_path: Option<String>,
    pub default_page_size: String,
    pub default_page_height: Option<String>,
    pub default_page_width: Option<String>,
    pub chrome_path: Option<String>,
    pub devtools_url: Option<String>,
    pub start_timeout: Duration,
}

/// JS shim reproducing print_designer's clone_and_update for core formats, only
/// installed when the page does not already define it.
const CLONE_AND_UPDATE_JS: &str = r#"
window.clone_and_update = window.clone_and_update || function(selector, total_pages, is_pd, type, is_dynamic) {
  var el = document.querySelector(selector);
  if (!el) return;
  var parent = el.parentNode;
  var template = el.cloneNode(true);
  el.remove();
  var count = is_dynamic ? total_pages : 4;
  for (var i = 1; i <= count; i++) {
    var clone = template.cloneNode(true);
    var topage = is_dynamic ? total_pages : 0;
    clone.querySelectorAll('.page, .page_info_page').forEach(function(n){ n.textContent = i; });
    clone.querySelectorAll('.topage, .page_info_topage').forEach(function(n){ n.textContent = topage; });
    clone.querySelectorAll('.frompage, .page_info_frompage').forEach(function(n){ n.textContent = 1; });
    parent.appendChild(clone);
  }
};
"#;

fn clone_call(selector: &str, total: usize, is_pd: i32, kind: &str, is_dynamic: i32) -> String {
    format!("{CLONE_AND_UPDATE_JS}\nclone_and_update('{selector}', {total}, {is_pd}, '{kind}', {is_dynamic});")
}

pub async fn run(job: Job) -> Result<Vec<u8>> {
    let mut chromium: Option<Chromium> = None;
    let devtools_url = match &job.devtools_url {
        Some(url) => url.clone(),
        None => {
            let path = job
                .chrome_path
                .clone()
                .ok_or_else(|| anyhow!("chrome_path or devtools_url required"))?;
            let chrome = Chromium::launch(&path, job.start_timeout).await?;
            let url = chrome.devtools_url.clone();
            chromium = Some(chrome);
            url
        }
    };

    let client = CdpClient::connect(&devtools_url).await?;
    let ctx = client
        .send("Target.createBrowserContext", json!({ "disposeOnDetach": true }), None)
        .await?;
    let browser_context_id = ctx["browserContextId"]
        .as_str()
        .ok_or_else(|| anyhow!("no browserContextId"))?
        .to_string();

    let interceptor = Interceptor {
        host_url: job.host_url.clone(),
        bench_sites_path: job.bench_sites_path.clone(),
        site_public_path: job.site_public_path.clone(),
        sid: job.sid.clone(),
    };

    let result = build_pdf(&job, &client, &browser_context_id, &interceptor).await;

    // Chromium is killed on drop (kill_on_drop). Explicitly hold until here.
    drop(chromium);
    result
}

async fn build_pdf(
    job: &Job,
    client: &std::sync::Arc<CdpClient>,
    browser_context_id: &str,
    interceptor: &Interceptor,
) -> Result<Vec<u8>> {
    let is_pd = job.is_print_designer;
    let doc = Html::parse(&job.html);
    let head = doc.head_children();
    let styles = doc.style_tags();
    let lang = doc.lang();
    let dir = doc.direction();

    let mut opts = job.options.clone();

    let header_present = doc.has_element("header-html");
    let footer_present = doc.has_element("footer-html");

    // Header page.
    let mut header_page: Option<Page> = None;
    let mut header_height = 0.0;
    let mut is_header_dynamic = false;
    if let Some(hf) = doc.take_header_footer("header-html") {
        is_header_dynamic = hf.is_dynamic;
        let content = html::render_header_footer(&lang, &dir, &head, &styles, &hf.content_children);
        let page = Page::new(client.clone(), browser_context_id, is_pd, interceptor.clone()).await?;
        page.set_tab_url().await?;
        page.set_content(&content).await?;
        header_height = page.get_element_height().await?;
        header_page = Some(page);
    } else {
        opts.insert("margin-top".into(), json!("15mm"));
    }

    // Footer page.
    let mut footer_page: Option<Page> = None;
    let mut footer_height = 0.0;
    let mut is_footer_dynamic = false;
    if let Some(hf) = doc.take_header_footer("footer-html") {
        is_footer_dynamic = hf.is_dynamic;
        let content = html::render_header_footer(&lang, &dir, &head, &styles, &hf.content_children);
        let page = Page::new(client.clone(), browser_context_id, is_pd, interceptor.clone()).await?;
        page.set_tab_url().await?;
        page.set_content(&content).await?;
        footer_height = page.get_element_height().await?;
        footer_page = Some(page);
    } else {
        opts.insert("margin-bottom".into(), json!("15mm"));
    }

    // Body page (document with header/footer already removed).
    let body_html = doc.serialize();
    let body_page = Page::new(client.clone(), browser_context_id, is_pd, interceptor.clone()).await?;
    body_page.set_tab_url().await?;
    body_page.set_content(&body_html).await?;

    let (body_opts, header_opts, footer_opts) = prepare_options(
        &doc,
        &mut opts,
        is_pd,
        header_present,
        footer_present,
        header_height,
        footer_height,
        job,
    )?;

    let mut body_page = body_page;
    body_page.options = body_opts;
    if let (Some(page), Some(o)) = (header_page.as_mut(), header_opts) {
        page.options = o;
    }
    if let (Some(page), Some(o)) = (footer_page.as_mut(), footer_opts) {
        page.options = o;
    }

    // print_designer non-dynamic header/footer produce four first/odd/even/last variants.
    if is_pd {
        if let Some(page) = header_page.as_ref()
            && !is_header_dynamic {
                page.evaluate(&clone_call("#header-render-container", 0, 1, "Header", 0), true).await?;
            }
        if let Some(page) = footer_page.as_ref()
            && !is_footer_dynamic {
                page.evaluate(&clone_call("#footer-render-container", 0, 1, "Footer", 0), true).await?;
            }
    }

    let body_pdf = body_page.generate_pdf().await?;
    let total_pages = merge::page_count(&body_pdf)?;

    let header_pdf = match header_page.as_ref() {
        Some(page) => {
            if is_header_dynamic {
                let selector = if is_pd { "#header-render-container" } else { ".wrapper" };
                let call = clone_call(selector, total_pages, is_pd as i32, "Header", 1);
                page.evaluate(&call, true).await?;
            }
            Some(page.generate_pdf().await?)
        }
        None => None,
    };

    let footer_pdf = match footer_page.as_ref() {
        Some(page) => {
            if is_footer_dynamic {
                let selector = if is_pd { "#footer-render-container" } else { ".wrapper" };
                let call = clone_call(selector, total_pages, is_pd as i32, "Footer", 1);
                page.evaluate(&call, true).await?;
            }
            Some(page.generate_pdf().await?)
        }
        None => None,
    };

    if let Ok(dir) = std::env::var("PINTO_DUMP") {
        let _ = std::fs::write(format!("{dir}/pinto_body.pdf"), &body_pdf);
        if let Some(h) = &header_pdf {
            let _ = std::fs::write(format!("{dir}/pinto_header.pdf"), h);
        }
        if let Some(f) = &footer_pdf {
            let _ = std::fs::write(format!("{dir}/pinto_footer.pdf"), f);
        }
        eprintln!("dump: header_height={header_height} footer_height={footer_height}");
    }

    let merged = merge::transform_pdf(MergeInput {
        body: body_pdf,
        header: header_pdf,
        footer: footer_pdf,
        is_header_dynamic,
        is_footer_dynamic,
        is_print_designer: is_pd,
    })?;

    let _ = body_page.close().await;
    if let Some(page) = header_page.as_ref() {
        let _ = page.close().await;
    }
    if let Some(page) = footer_page.as_ref() {
        let _ = page.close().await;
    }

    Ok(merged)
}

/// printToPDF option map, and the (body, header, footer) triple prepare_options returns.
type PageOptions = Map<String, Value>;
type PdfOptions = (PageOptions, Option<PageOptions>, Option<PageOptions>);

/// Compute the printToPDF option maps for body/header/footer. Ports prepare_options_for_pdf.
#[allow(clippy::too_many_arguments)]
fn prepare_options(
    doc: &Html,
    opts: &mut Map<String, Value>,
    is_pd: bool,
    header_present: bool,
    footer_present: bool,
    header_height: f64,
    footer_height: f64,
    job: &Job,
) -> Result<PdfOptions> {
    // 1. .print-format CSS overrides.
    const ATTRS: [&str; 9] = [
        "margin-top", "margin-bottom", "margin-left", "margin-right", "page-size",
        "header-spacing", "orientation", "page-width", "page-height",
    ];
    for (name, value) in doc.print_format_declarations() {
        if ATTRS.contains(&name.as_str()) {
            opts.insert(name, Value::String(value));
        }
    }

    // 2. default page size.
    let page_size = opt_str(opts, "page-size")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            if job.default_page_size.is_empty() { "A4".into() } else { job.default_page_size.clone() }
        });
    if page_size == "Custom" {
        if opt_str(opts, "page-height").is_none()
            && let Some(h) = &job.default_page_height {
                opts.insert("page-height".into(), Value::String(h.clone()));
            }
        if opt_str(opts, "page-width").is_none()
            && let Some(w) = &job.default_page_width {
                opts.insert("page-width".into(), Value::String(w.clone()));
            }
    } else {
        opts.insert("page-size".into(), Value::String(page_size));
    }

    // 3. base printToPDF options.
    let landscape = opt_str(opts, "orientation").as_deref() == Some("Landscape");
    let mut updated = Map::new();
    updated.insert("scale".into(), json!(1));
    updated.insert("printBackground".into(), json!(true));
    updated.insert("transferMode".into(), json!("ReturnAsStream"));
    updated.insert("marginTop".into(), json!(0));
    updated.insert("marginBottom".into(), json!(0));
    updated.insert("marginLeft".into(), json!(0));
    updated.insert("marginRight".into(), json!(0));
    updated.insert("landscape".into(), json!(landscape));
    updated.insert("preferCSSPageSize".into(), json!(false));
    updated.insert("pageRanges".into(), json!(opt_str(opts, "page-ranges").unwrap_or_default()));
    updated.insert("generateTaggedPDF".into(), json!(opt_bool(opts, "generate-tagged-pdf")));
    updated.insert("generateOutline".into(), json!(opt_bool(opts, "generate-outline")));

    // 4. implicit side margins for non-print-designer formats.
    if !is_pd {
        if !truthy(opts, "margin-right") {
            opts.insert("margin-right".into(), json!("15mm"));
        }
        if !truthy(opts, "margin-left") {
            opts.insert("margin-left".into(), json!("15mm"));
        }
    }

    // 5. resolve page dimensions from a named size when missing.
    if !truthy(opts, "page-height") || !truthy(opts, "page-width") {
        let page_size = opt_str(opts, "page-size").ok_or_else(|| anyhow!("Page size is required"))?;
        if page_size == "CUSTOM" {
            bail!("Custom page size requires page-height and page-width");
        }
        let (w_mm, h_mm) = options::page_size_mm(&page_size).ok_or_else(|| anyhow!("Invalid page size"))?;
        opts.insert("page-height".into(), json!(options::convert_uom(h_mm, "mm", "px")));
        opts.insert("page-width".into(), json!(options::convert_uom(w_mm, "mm", "px")));
    }

    // 6. normalize string dimensions to px numbers.
    if let Some(Value::String(s)) = opts.get("page-height").cloned() {
        opts.insert("page-height".into(), json!(get_converted_num(&s)));
    }
    if let Some(Value::String(s)) = opts.get("page-width").cloned() {
        opts.insert("page-width".into(), json!(get_converted_num(&s)));
    }

    // 7. paper width (inches).
    let page_width_px = opt_num(opts, "page-width").unwrap_or(0.0);
    updated.insert("paperWidth".into(), json!(options::convert_uom(page_width_px, "px", "in")));

    // 8. side margins (inches).
    if truthy(opts, "margin-left") {
        let px = get_converted_num(&opt_str(opts, "margin-left").unwrap());
        updated.insert("marginLeft".into(), json!(options::convert_uom(px, "px", "in")));
    }
    if truthy(opts, "margin-right") {
        let px = get_converted_num(&opt_str(opts, "margin-right").unwrap());
        updated.insert("marginRight".into(), json!(options::convert_uom(px, "px", "in")));
    }

    // 9. per-page copies.
    let mut body_opts = updated.clone();
    let mut header_opts = header_present.then(|| updated.clone());
    let mut footer_opts = footer_present.then(|| updated.clone());

    // 10. top/bottom margins (px).
    let margin_top = margin_px(opts.get("margin-top"));
    let margin_bottom = margin_px(opts.get("margin-bottom"));

    // 11-12. header paper height.
    let mut header_with_spacing_top_margin = 0.0;
    if header_present {
        let header_with_top_margin = header_height + margin_top;
        let header_spacing = opt_num(opts, "header-spacing").unwrap_or(0.0);
        header_with_spacing_top_margin = header_with_top_margin + header_spacing;
        let ph = if header_with_spacing_top_margin != 0.0 {
            options::convert_uom(header_with_spacing_top_margin, "px", "in")
        } else {
            0.0
        };
        header_opts.as_mut().unwrap().insert("paperHeight".into(), json!(ph));
    }

    let margin_top_in = options::convert_uom(margin_top, "px", "in");
    if header_present {
        header_opts.as_mut().unwrap().insert("marginTop".into(), json!(margin_top_in));
    } else {
        body_opts.insert("marginTop".into(), json!(margin_top_in));
    }

    // 15. footer paper height.
    let mut footer_with_bottom_margin = 0.0;
    if footer_present {
        let ph = if footer_height != 0.0 {
            options::convert_uom(footer_height, "px", "in")
        } else {
            0.0
        };
        footer_opts.as_mut().unwrap().insert("paperHeight".into(), json!(ph));
        footer_with_bottom_margin = footer_height + margin_bottom;
    }

    let margin_bottom_in = options::convert_uom(margin_bottom, "px", "in");
    if footer_present {
        footer_opts.as_mut().unwrap().insert("marginBottom".into(), json!(margin_bottom_in));
    } else {
        body_opts.insert("marginBottom".into(), json!(margin_bottom_in));
    }

    // 18-19. body paper height.
    let page_height_px = opt_num(opts, "page-height").unwrap_or(0.0);
    let body_height = page_height_px - (header_with_spacing_top_margin + footer_with_bottom_margin);
    body_opts.insert("paperHeight".into(), json!(options::convert_uom(body_height, "px", "in")));

    Ok((body_opts, header_opts, footer_opts))
}

/// _get_converted_num: parse a length string and convert to px.
fn get_converted_num(s: &str) -> f64 {
    match options::parse_float_and_unit(s, "px") {
        Some(fu) => options::convert_uom(fu.value, &fu.unit, "px"),
        None => 0.0,
    }
}

/// _get_converted_num applied to an options value (missing → 0, number → px).
fn margin_px(value: Option<&Value>) -> f64 {
    match value {
        Some(Value::String(s)) => get_converted_num(s),
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        _ => 0.0,
    }
}

/// Python truthiness for an options value.
fn truthy(opts: &Map<String, Value>, key: &str) -> bool {
    match opts.get(key) {
        None | Some(Value::Null) => false,
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Number(n)) => n.as_f64().map(|f| f != 0.0).unwrap_or(false),
        Some(Value::Bool(b)) => *b,
        _ => true,
    }
}
