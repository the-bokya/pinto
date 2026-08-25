//! Block + inline + table layout producing an absolute display list in CSS px (y-down).

use cosmic_text::{Attrs, Buffer, Family, Metrics, Shaping, Weight, fontdb};
use kuchikiki::NodeRef;

use super::fonts::Fonts;
use super::style::{ComputedStyle, Display, Float, Rgba, Stylesheet, TextAlign, compute};

/// Active left-float row while laying out a flow (Bootstrap grid columns).
#[derive(Default)]
struct Floats {
    active: bool,
    pen: f32,
    top: f32,
    height: f32,
}
impl Floats {
    fn bottom(&self) -> f32 {
        if self.active { self.top + self.height } else { f32::MIN }
    }
    fn clear(&mut self) {
        self.active = false;
        self.pen = 0.0;
        self.height = 0.0;
    }
}

/// Outer (margin-box) width of a block given its containing width.
fn outer_width(style: &ComputedStyle, avail: f32) -> f32 {
    let bl = style.border.left.eff_width();
    let br = style.border.right.eff_width();
    let content = if let Some(w) = style.width {
        w
    } else if let Some(p) = style.width_percent {
        (avail - style.margin.left - style.margin.right) * p - bl - br - style.padding.left - style.padding.right
    } else {
        (avail - style.margin.left - style.margin.right - bl - br - style.padding.left - style.padding.right).max(0.0)
    };
    content + style.padding.left + style.padding.right + bl + br + style.margin.left + style.margin.right
}

#[derive(Clone, Debug)]
pub enum Item {
    Rect { x: f32, y: f32, w: f32, h: f32, color: Rgba },
    Gradient { x: f32, y: f32, w: f32, h: f32, grad: super::style::Gradient },
    Glyph { font: fontdb::ID, gid: u16, x: f32, y: f32, size: f32, color: Rgba },
    Image { x: f32, y: f32, w: f32, h: f32, src: String },
}

impl Item {
    pub fn translated(&self, dx: f32, dy: f32) -> Item {
        match self.clone() {
            Item::Rect { x, y, w, h, color } => Item::Rect { x: x + dx, y: y + dy, w, h, color },
            Item::Gradient { x, y, w, h, grad } => Item::Gradient { x: x + dx, y: y + dy, w, h, grad },
            Item::Glyph { font, gid, x, y, size, color } => Item::Glyph { font, gid, x: x + dx, y: y + dy, size, color },
            Item::Image { x, y, w, h, src } => Item::Image { x: x + dx, y: y + dy, w, h, src },
        }
    }
}

/// Result of laying out a flow: items plus the advanced height and page-break candidates.
#[derive(Default)]
pub struct Flow {
    pub items: Vec<Item>,
    pub height: f32,
    pub breaks: Vec<f32>,
}

pub struct Engine<'a> {
    pub fonts: &'a mut Fonts,
    pub sheets: Vec<&'a Stylesheet>,
}

struct Span {
    text: String,
    family: String,
    size: f32,
    weight: u16,
    italic: bool,
    color: Rgba,
    line_height: f32,
}

impl<'a> Engine<'a> {
    /// Lay out block-level children of `container` in normal flow.
    pub fn layout_flow(
        &mut self,
        container: &NodeRef,
        parent_style: &ComputedStyle,
        x: f32,
        y: f32,
        avail_width: f32,
    ) -> Flow {
        let mut flow = Flow { items: vec![], height: 0.0, breaks: vec![] };
        let mut cursor_y = y;
        let mut inline_run: Vec<NodeRef> = vec![];
        let mut prev_mb = 0.0f32; // previous block's bottom margin (for collapsing)
        let mut first = true;
        let mut fl = Floats::default(); // active left-float row

        for child in container.children() {
            if let Some(text) = child.as_text() {
                if text.borrow().trim().is_empty() && inline_run.is_empty() {
                    continue;
                }
                inline_run.push(child.clone());
                continue;
            }
            let Some(el) = child.as_element() else { continue };
            let style = compute(&child, parent_style, &self.sheets);
            let is_img = el.name.local.to_string().eq_ignore_ascii_case("img");
            match style.display {
                Display::None => continue,
                Display::Inline | Display::InlineBlock if !is_img => inline_run.push(child.clone()),
                _ => {
                    if !inline_run.is_empty() {
                        let h = self.layout_inline(&inline_run, parent_style, x, cursor_y, avail_width, &mut flow.items);
                        cursor_y += h;
                        inline_run.clear();
                        prev_mb = 0.0;
                        first = false;
                    }

                    if style.float == Float::Left {
                        let ow = outer_width(&style, avail_width);
                        if fl.active && fl.pen + ow > avail_width + 0.5 {
                            fl.top += fl.height;
                            fl.pen = 0.0;
                            fl.height = 0.0;
                        }
                        if !fl.active {
                            fl.active = true;
                            fl.top = cursor_y;
                            fl.pen = 0.0;
                            fl.height = 0.0;
                        }
                        let frag = self.layout_block(&child, &style, x + fl.pen, fl.top, avail_width);
                        flow.items.extend(frag.items);
                        fl.pen += ow;
                        fl.height = fl.height.max(frag.height);
                        continue;
                    }

                    // Non-float block: clear any active floats first.
                    if fl.active || style.clear {
                        cursor_y = cursor_y.max(fl.bottom());
                        fl.clear();
                        prev_mb = 0.0;
                    }
                    let overlap = if first { 0.0 } else { prev_mb.min(style.margin.top) };
                    cursor_y -= overlap;
                    let frag = self.layout_block(&child, &style, x, cursor_y, avail_width);
                    flow.items.extend(frag.items);
                    flow.breaks.extend(frag.breaks);
                    cursor_y += frag.height;
                    flow.breaks.push(cursor_y);
                    prev_mb = style.margin.bottom;
                    first = false;
                }
            }
        }
        if !inline_run.is_empty() {
            let h = self.layout_inline(&inline_run, parent_style, x, cursor_y, avail_width, &mut flow.items);
            cursor_y += h;
        }
        cursor_y = cursor_y.max(fl.bottom());
        flow.height = cursor_y - y;
        flow
    }

    fn layout_block(&mut self, node: &NodeRef, style: &ComputedStyle, x: f32, y: f32, avail_width: f32) -> Flow {
        let tag = node.as_element().map(|e| e.name.local.to_string().to_ascii_lowercase()).unwrap_or_default();

        if tag == "img" {
            return self.layout_image(node, style, x, y, avail_width);
        }
        if style.display == Display::Table {
            return self.layout_table(node, style, x, y, avail_width);
        }

        let bl = style.border.left.eff_width();
        let br = style.border.right.eff_width();
        let bt = style.border.top.eff_width();
        let bb = style.border.bottom.eff_width();

        let content_width = if let Some(w) = style.width {
            w
        } else if let Some(p) = style.width_percent {
            (avail_width - style.margin.left - style.margin.right) * p - bl - br - style.padding.left - style.padding.right
        } else {
            (avail_width - style.margin.left - style.margin.right - bl - br - style.padding.left - style.padding.right).max(0.0)
        };

        let border_box_left = x + style.margin.left;
        let content_left = border_box_left + bl + style.padding.left;
        let content_top = y + style.margin.top + bt + style.padding.top;

        let inner = self.layout_flow(node, style, content_left, content_top, content_width);
        let content_height = style.height.unwrap_or(inner.height);
        let border_box_w = content_width + style.padding.left + style.padding.right + bl + br;
        let border_box_h = content_height + style.padding.top + style.padding.bottom + bt + bb;
        let border_box_top = y + style.margin.top;

        let mut items = Vec::new();
        if let Some(bg) = style.background
            && bg[3] > 0 {
                items.push(Item::Rect { x: border_box_left, y: border_box_top, w: border_box_w, h: border_box_h, color: bg });
            }
        if let Some(grad) = style.background_gradient {
            items.push(Item::Gradient { x: border_box_left, y: border_box_top, w: border_box_w, h: border_box_h, grad });
        }
        push_borders(&mut items, style, border_box_left, border_box_top, border_box_w, border_box_h);
        items.extend(inner.items);

        let mut breaks = inner.breaks;
        Flow { items, height: style.margin.top + border_box_h + style.margin.bottom, breaks: std::mem::take(&mut breaks) }
    }

    /// Lay out a run of inline nodes as an anonymous block; returns its height.
    fn layout_inline(
        &mut self,
        nodes: &[NodeRef],
        parent_style: &ComputedStyle,
        x: f32,
        y: f32,
        width: f32,
        out: &mut Vec<Item>,
    ) -> f32 {
        let mut spans: Vec<Span> = Vec::new();
        for n in nodes {
            self.collect_spans(n, parent_style, &mut spans);
        }
        if spans.iter().all(|s| s.text.trim().is_empty()) {
            return 0.0;
        }

        // Concatenate text and record per-span color/offset ranges.
        let mut text = String::new();
        let mut color_ranges: Vec<(usize, Rgba)> = Vec::new();
        let default_lh = spans.first().map(|s| s.line_height).unwrap_or(parent_style.line_height_px());
        let mut rich: Vec<(std::ops::Range<usize>, &Span)> = Vec::new();
        for s in &spans {
            let start = text.len();
            text.push_str(&s.text);
            rich.push((start..text.len(), s));
            color_ranges.push((text.len(), s.color));
        }

        let mut buffer = Buffer::new(&mut self.fonts.system, Metrics::new(16.0, default_lh));
        buffer.set_size(Some(width), None);
        let align = match parent_style.text_align {
            TextAlign::Left => None,
            TextAlign::Right => Some(cosmic_text::Align::Right),
            TextAlign::Center => Some(cosmic_text::Align::Center),
            TextAlign::Justify => Some(cosmic_text::Align::Justified),
        };
        let spans_iter = rich.iter().map(|(r, s)| {
            let family = family_of(&s.family);
            let attrs = Attrs::new()
                .family(family)
                .weight(Weight(s.weight))
                .style(if s.italic { cosmic_text::Style::Italic } else { cosmic_text::Style::Normal })
                .metrics(Metrics::new(s.size, s.line_height));
            (&text[r.clone()], attrs)
        });
        buffer.set_rich_text(spans_iter, &Attrs::new(), Shaping::Advanced, align);
        buffer.shape_until_scroll(&mut self.fonts.system, false);

        let mut max_bottom = 0.0f32;
        for run in buffer.layout_runs() {
            for g in run.glyphs.iter() {
                let color = color_ranges.iter().find(|(end, _)| g.start < *end).map(|(_, c)| *c).unwrap_or([0, 0, 0, 255]);
                out.push(Item::Glyph {
                    font: g.font_id,
                    gid: g.glyph_id,
                    x: x + g.x,
                    y: y + run.line_y,
                    size: g.font_size,
                    color,
                });
            }
            max_bottom = max_bottom.max(run.line_top + run.line_height);
        }
        max_bottom
    }

    /// Widest unwrapped line width of a node's inline content (for auto table sizing).
    pub fn measure_text_width(&mut self, node: &NodeRef, style: &ComputedStyle) -> f32 {
        let mut spans: Vec<Span> = Vec::new();
        self.collect_spans(node, style, &mut spans);
        if spans.iter().all(|s| s.text.trim().is_empty()) {
            return 0.0;
        }
        let mut text = String::new();
        let mut rich: Vec<(std::ops::Range<usize>, &Span)> = Vec::new();
        for s in &spans {
            let start = text.len();
            text.push_str(&s.text);
            rich.push((start..text.len(), s));
        }
        let mut buffer = Buffer::new(&mut self.fonts.system, Metrics::new(16.0, style.line_height_px()));
        buffer.set_size(None, None);
        let spans_iter = rich.iter().map(|(r, s)| {
            let attrs = Attrs::new()
                .family(family_of(&s.family))
                .weight(Weight(s.weight))
                .style(if s.italic { cosmic_text::Style::Italic } else { cosmic_text::Style::Normal })
                .metrics(Metrics::new(s.size, s.line_height));
            (&text[r.clone()], attrs)
        });
        buffer.set_rich_text(spans_iter, &Attrs::new(), Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.fonts.system, false);
        buffer.layout_runs().map(|r| r.line_w).fold(0.0, f32::max)
    }

    fn collect_spans(&mut self, node: &NodeRef, parent_style: &ComputedStyle, spans: &mut Vec<Span>) {
        if let Some(text) = node.as_text() {
            let raw = text.borrow();
            let collapsed = collapse_ws(&raw);
            if !collapsed.is_empty() {
                spans.push(Span {
                    text: collapsed,
                    family: parent_style.font_family.first().cloned().unwrap_or_default(),
                    size: parent_style.font_size,
                    weight: parent_style.font_weight,
                    italic: parent_style.italic,
                    color: parent_style.color,
                    line_height: parent_style.line_height_px(),
                });
            }
            return;
        }
        let Some(el) = node.as_element() else { return };
        let tag = el.name.local.to_string().to_ascii_lowercase();
        let style = compute(node, parent_style, &self.sheets);
        if style.display == Display::None {
            return;
        }
        if tag == "br" {
            spans.push(Span {
                text: "\n".into(),
                family: style.font_family.first().cloned().unwrap_or_default(),
                size: style.font_size,
                weight: style.font_weight,
                italic: style.italic,
                color: style.color,
                line_height: style.line_height_px(),
            });
            return;
        }
        for child in node.children() {
            self.collect_spans(&child, &style, spans);
        }
    }

    fn layout_image(&mut self, node: &NodeRef, style: &ComputedStyle, x: f32, y: f32, avail_width: f32) -> Flow {
        let src = node.as_element().and_then(|e| e.attributes.borrow().get("src").map(|s| s.to_string())).unwrap_or_default();
        let (iw, ih) = super::image::intrinsic_size(&src).unwrap_or((100.0, 100.0));
        let w = style.width.unwrap_or(iw);
        let h = match (style.width, style.height) {
            (_, Some(hh)) => hh,
            (Some(ww), None) => ww / iw * ih,
            (None, None) => ih,
        };
        let _ = avail_width;
        let items = vec![Item::Image { x: x + style.margin.left, y: y + style.margin.top, w, h, src }];
        Flow { items, height: style.margin.top + h + style.margin.bottom, breaks: vec![] }
    }

    fn layout_table(&mut self, node: &NodeRef, style: &ComputedStyle, x: f32, y: f32, avail_width: f32) -> Flow {
        super::table::layout(self, node, style, x, y, avail_width)
    }
}

fn family_of(name: &str) -> Family<'_> {
    match name.to_ascii_lowercase().as_str() {
        "serif" => Family::Serif,
        "sans-serif" | "" => Family::SansSerif,
        "monospace" => Family::Monospace,
        "cursive" => Family::Cursive,
        "fantasy" => Family::Fantasy,
        _ => Family::Name(name),
    }
}

/// Collapse runs of ASCII whitespace to single spaces (normal white-space).
fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !prev_space {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    out
}

pub fn push_borders(items: &mut Vec<Item>, style: &ComputedStyle, x: f32, y: f32, w: f32, h: f32) {
    let b = &style.border;
    if b.top.present {
        items.push(Item::Rect { x, y, w, h: b.top.width, color: b.top.color });
    }
    if b.bottom.present {
        items.push(Item::Rect { x, y: y + h - b.bottom.width, w, h: b.bottom.width, color: b.bottom.color });
    }
    if b.left.present {
        items.push(Item::Rect { x, y, w: b.left.width, h, color: b.left.color });
    }
    if b.right.present {
        items.push(Item::Rect { x: x + w - b.right.width, y, w: b.right.width, h, color: b.right.color });
    }
}
