//! HTML DOM handling with kuchikiki. Ports the soup manipulation in
//! frappe/utils/pdf.py + browser.py: header/footer extraction, visible/hidden-pdf
//! toggling, head/style collection, and the chrome header/footer template.

use kuchikiki::NodeRef;
use kuchikiki::traits::*;

use crate::css;

pub struct Html {
    pub doc: NodeRef,
}

/// A header or footer captured from the body for separate rendering.
pub struct HeaderFooter {
    /// Direct children of the #header-html / #footer-html element, serialized.
    pub content_children: Vec<String>,
    /// Whether the content uses page-number classes (dynamic).
    pub is_dynamic: bool,
}

impl Html {
    pub fn parse(html: &str) -> Self {
        Self { doc: kuchikiki::parse_html().one(html) }
    }

    pub fn lang(&self) -> String {
        self.html_attr("lang").unwrap_or_else(|| "en".into())
    }

    pub fn direction(&self) -> String {
        self.html_attr("dir").unwrap_or_else(|| "ltr".into())
    }

    fn html_attr(&self, name: &str) -> Option<String> {
        let el = self.doc.select_first("html").ok()?;
        let attrs = el.attributes.borrow();
        attrs.get(name).map(|s| s.to_string())
    }

    /// Serialized direct children of <head> (meta/link/style/title …), in order.
    pub fn head_children(&self) -> Vec<String> {
        match self.doc.select_first("head") {
            Ok(head) => head.as_node().children().map(|c| serialize(&c)).collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Serialized <style> tags across the document, in order.
    pub fn style_tags(&self) -> Vec<String> {
        match self.doc.select("style") {
            Ok(sel) => sel.map(|s| serialize(s.as_node())).collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Concatenated text of all <style> tags, for CSS option parsing.
    pub fn stylesheet_text(&self) -> String {
        let mut sheet = String::new();
        if let Ok(sel) = self.doc.select("style") {
            for s in sel {
                sheet.push_str(&s.as_node().text_contents());
                sheet.push('\n');
            }
        }
        sheet
    }

    /// `.print-format`-scoped declarations (property, value), in document order.
    pub fn print_format_declarations(&self) -> Vec<(String, String)> {
        css::print_format_declarations(&self.stylesheet_text())
    }

    /// Extract #{id}-html: capture its toggled children, then remove it from the body.
    pub fn take_header_footer(&self, id: &str) -> Option<HeaderFooter> {
        let selector = format!("#{id}");
        let element = self.doc.select_first(&selector).ok()?;
        let node = element.as_node().clone();

        let is_dynamic = is_page_no_used(&node);
        toggle_visible_pdf(&node);

        let content_children: Vec<String> = node.children().map(|c| serialize(&c)).collect();

        // Remove every element carrying this id from the body.
        drop(element);
        if let Ok(matches) = self.doc.select(&selector) {
            for m in matches.collect::<Vec<_>>() {
                m.as_node().detach();
            }
        }
        Some(HeaderFooter { content_children, is_dynamic })
    }

    pub fn has_element(&self, id: &str) -> bool {
        self.doc.select_first(&format!("#{id}")).is_ok()
    }

    /// Serialize the whole document for injection into the body page.
    pub fn serialize(&self) -> String {
        serialize(&self.doc)
    }
}

/// Remove `visible-pdf` classes and delete `hidden-pdf` elements within `root`'s subtree.
pub fn toggle_visible_pdf(root: &NodeRef) {
    // Delete hidden-pdf descendants.
    let hidden: Vec<NodeRef> = root
        .descendants()
        .filter(|n| has_class(n, "hidden-pdf"))
        .collect();
    for n in hidden {
        n.detach();
    }
    // Unhide visible-pdf descendants by dropping the class token.
    for n in root.descendants() {
        if has_class(&n, "visible-pdf") {
            remove_class(&n, "visible-pdf");
        }
    }
}

/// Detect page-number classes used by dynamic headers/footers (is_page_no_used).
pub fn is_page_no_used(root: &NodeRef) -> bool {
    const CLASSES: [&str; 6] = [
        "page",
        "frompage",
        "topage",
        "page_info_page",
        "page_info_frompage",
        "page_info_topage",
    ];
    root.descendants().any(|n| CLASSES.iter().any(|c| has_class(&n, c)))
}

fn has_class(node: &NodeRef, class: &str) -> bool {
    let Some(el) = node.as_element() else { return false };
    let attrs = el.attributes.borrow();
    attrs
        .get("class")
        .map(|c| c.split_whitespace().any(|t| t == class))
        .unwrap_or(false)
}

fn remove_class(node: &NodeRef, class: &str) {
    if let Some(el) = node.as_element() {
        let mut attrs = el.attributes.borrow_mut();
        if let Some(existing) = attrs.get_mut("class") {
            let kept: Vec<&str> = existing.split_whitespace().filter(|t| *t != class).collect();
            *existing = kept.join(" ");
        }
    }
}

fn serialize(node: &NodeRef) -> String {
    node.to_string()
}

/// Build the header/footer page HTML, porting templates/print_formats/chrome_pdf_header_footer.html.
pub fn render_header_footer(
    lang: &str,
    direction: &str,
    head: &[String],
    styles: &[String],
    content: &[String],
) -> String {
    const FIXED_STYLE: &str = r#"
			<style>
				body {
					margin: 0 !important;
					border: 0 !important;
					padding-top: 1mm !important;
				}
				.letter-head,
				.letter-head-footer {
					margin-top: -12mm !important;
				}
				/* Dont show explicit links for <a> tags */
				@media print {
					/* padding is added to simulate old wkhtmltopdf format prints */
					.wrapper {
						box-sizing: border-box;
						padding: 1mm 0 1mm !important;
						page-break-after: always !important;
					}
					[document-status] {
						margin-bottom: 0 !important;
					}
					a[href]:after {
						content: none;
					}
				}
			</style>
"#;

    let mut html = String::new();
    html.push_str("<!DOCTYPE html>\n");
    html.push_str(&format!("<html lang={lang} dir={direction}>\n\t<head>\n\t\t<meta charset=\"utf-8\">\n"));
    for tag in head {
        html.push_str(tag);
    }
    html.push_str(FIXED_STYLE);
    for tag in styles {
        html.push_str(tag);
    }
    html.push_str("\n\t</head>\n\t<body>\n\t\t<div class=\"print-format\">\n\t\t\t<div class=\"wrapper\">\n");
    for tag in content {
        html.push_str(tag);
    }
    html.push_str("\t\t\t</div>\n\t\t</div>\n\t</body>\n</html>");
    html
}
