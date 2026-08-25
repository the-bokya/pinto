//! A pragmatic CSS cascade: parse stylesheets, match selectors, and resolve computed
//! styles for the property subset print formats use. Not a full CSS engine, but covers
//! the common cases (block/inline/table, box model, fonts, colors, borders, alignment).

use kuchikiki::NodeRef;

use crate::css::strip_comments;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Display {
    Block,
    Inline,
    InlineBlock,
    ListItem,
    Table,
    TableRowGroup,
    TableRow,
    TableCell,
    None,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Float {
    None,
    Left,
    Right,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TextAlign {
    Left,
    Right,
    Center,
    Justify,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VerticalAlign {
    Top,
    Middle,
    Bottom,
    Baseline,
}

pub type Rgba = [u8; 4];

#[derive(Clone, Copy, Debug)]
pub struct Edges {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}
impl Edges {
    const ZERO: Edges = Edges { top: 0.0, right: 0.0, bottom: 0.0, left: 0.0 };
}

#[derive(Clone, Copy, Debug)]
pub struct Border {
    pub width: f32,
    pub color: Rgba,
    pub present: bool,
}
impl Border {
    const NONE: Border = Border { width: 0.0, color: [0, 0, 0, 255], present: false };
    pub fn eff_width(&self) -> f32 {
        if self.present { self.width } else { 0.0 }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Gradient {
    pub from: Rgba,
    pub to: Rgba,
    pub horizontal: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct Borders {
    pub top: Border,
    pub right: Border,
    pub bottom: Border,
    pub left: Border,
}

#[derive(Clone, Debug)]
pub struct ComputedStyle {
    pub display: Display,
    pub font_family: Vec<String>,
    pub font_size: f32,
    pub font_weight: u16,
    pub italic: bool,
    pub line_height: Option<f32>, // None => normal (~1.2 * font-size resolved in layout)
    pub color: Rgba,
    pub background: Option<Rgba>,
    pub background_gradient: Option<Gradient>,
    pub text_align: TextAlign,
    pub vertical_align: VerticalAlign,
    pub white_space_nowrap: bool,
    pub margin: Edges,
    pub padding: Edges,
    pub border: Borders,
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub width_percent: Option<f32>,
    pub float: Float,
    pub clear: bool,
    pub page_break_after: bool,
    pub page_break_before: bool,
    pub bold_default: bool,
}

impl ComputedStyle {
    pub fn initial() -> Self {
        ComputedStyle {
            display: Display::Inline,
            font_family: vec![],
            font_size: 16.0,
            font_weight: 400,
            italic: false,
            line_height: None,
            color: [0, 0, 0, 255],
            background: None,
            background_gradient: None,
            text_align: TextAlign::Left,
            vertical_align: VerticalAlign::Baseline,
            white_space_nowrap: false,
            margin: Edges::ZERO,
            padding: Edges::ZERO,
            border: Borders {
                top: Border::NONE,
                right: Border::NONE,
                bottom: Border::NONE,
                left: Border::NONE,
            },
            width: None,
            height: None,
            width_percent: None,
            float: Float::None,
            clear: false,
            page_break_after: false,
            page_break_before: false,
            bold_default: false,
        }
    }

    /// Inheritable base for a child: copy inherited properties, reset the rest.
    fn inherited_from(parent: &ComputedStyle) -> Self {
        let mut s = ComputedStyle::initial();
        s.font_family = parent.font_family.clone();
        s.font_size = parent.font_size;
        s.font_weight = parent.font_weight;
        s.italic = parent.italic;
        s.line_height = parent.line_height;
        s.color = parent.color;
        s.text_align = parent.text_align;
        s.white_space_nowrap = parent.white_space_nowrap;
        s
    }

    pub fn line_height_px(&self) -> f32 {
        self.line_height.unwrap_or(self.font_size * 1.2)
    }
}

// ----- selectors -----

#[derive(Clone, Debug)]
struct Compound {
    tag: Option<String>,
    id: Option<String>,
    classes: Vec<String>,
}

#[derive(Clone, Debug)]
struct Selector {
    // descendant chain, rightmost last
    compounds: Vec<Compound>,
    spec: (u32, u32, u32),
}

#[derive(Clone, Debug)]
struct Rule {
    selectors: Vec<Selector>,
    decls: Vec<(String, String)>,
    order: usize,
    ua: bool,
}

pub struct Stylesheet {
    rules: Vec<Rule>,
}

// Legacy single-colon pseudo-elements (they style generated content, not the element).
const PSEUDO_ELEMENTS: [&str; 8] = [
    ":before", ":after", ":first-line", ":first-letter", ":placeholder", ":selection", ":marker", ":backdrop",
];

fn parse_compound(text: &str) -> Option<Compound> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    // Split the base compound (tag/.class/#id) from any pseudo/attribute suffix.
    let cut = text.find([':', '[']).unwrap_or(text.len());
    let (base, suffix) = text.split_at(cut);
    let low = suffix.to_ascii_lowercase();
    // A pseudo-element targets generated content — drop the whole selector.
    if low.starts_with("::") || PSEUDO_ELEMENTS.iter().any(|pe| low.starts_with(pe)) {
        return None;
    }
    // Attribute-only or pseudo-class-only selectors have no base to match on — drop them.
    if base.is_empty() {
        return None;
    }

    let mut c = Compound { tag: None, id: None, classes: vec![] };
    let mut cur = String::new();
    let mut kind = 't'; // t=tag, .=class, #=id
    let flush = |kind: char, cur: &mut String, c: &mut Compound| {
        if cur.is_empty() {
            return;
        }
        match kind {
            '.' => c.classes.push(std::mem::take(cur)),
            '#' => c.id = Some(std::mem::take(cur)),
            _ => c.tag = Some(std::mem::take(cur).to_ascii_lowercase()),
        }
    };
    for ch in base.chars() {
        if ch == '.' || ch == '#' {
            flush(kind, &mut cur, &mut c);
            kind = ch;
        } else {
            cur.push(ch);
        }
    }
    flush(kind, &mut cur, &mut c);
    if c.tag.as_deref() == Some("*") {
        c.tag = None;
    }
    Some(c)
}

fn parse_selector(text: &str, ua: bool) -> Option<Selector> {
    // Combinators are approximated as descendant; an unparseable compound drops the selector.
    let mut compounds: Vec<Compound> = Vec::new();
    for part in text.split_whitespace() {
        if matches!(part, ">" | "+" | "~") {
            continue;
        }
        compounds.push(parse_compound(part)?);
    }
    if compounds.is_empty() {
        return None;
    }
    let mut spec = (0u32, 0u32, 0u32);
    for c in &compounds {
        if c.id.is_some() {
            spec.0 += 1;
        }
        spec.1 += c.classes.len() as u32;
        if c.tag.is_some() {
            spec.2 += 1;
        }
    }
    // UA rules rank below all author rules.
    if ua {
        spec.0 = 0;
    }
    Some(Selector { compounds, spec })
}

/// The media environment for evaluating `@media` at parse time (we render for print).
#[derive(Clone, Copy)]
pub struct MediaCtx {
    pub print: bool,
    pub width_px: f32,
}

impl MediaCtx {
    pub const NONE: MediaCtx = MediaCtx { print: true, width_px: 794.0 };
}

pub fn parse_stylesheet(css: &str, ua: bool, start_order: usize, media: &MediaCtx) -> Stylesheet {
    let css = strip_comments(css);
    let mut rules = Vec::new();
    let mut order = start_order;
    collect_rules(&css, ua, media, &mut order, &mut rules);
    Stylesheet { rules }
}

fn collect_rules(css: &str, ua: bool, media: &MediaCtx, order: &mut usize, rules: &mut Vec<Rule>) {
    let bytes = css.as_bytes();
    let n = bytes.len();
    let mut i = 0;
    let mut start = 0;
    while i < n {
        match bytes[i] {
            b'{' => {
                let prelude = css[start..i].trim().to_string();
                let block_start = i + 1;
                let mut depth = 1;
                let mut j = block_start;
                while j < n && depth > 0 {
                    match bytes[j] {
                        b'{' => depth += 1,
                        b'}' => depth -= 1,
                        _ => {}
                    }
                    j += 1;
                }
                let block = &css[block_start..j.saturating_sub(1).min(n)];
                if let Some(query) = prelude.strip_prefix("@media") {
                    if eval_media(query, media) {
                        collect_rules(block, ua, media, order, rules);
                    }
                } else if !prelude.starts_with('@') {
                    let selectors: Vec<Selector> =
                        prelude.split(',').filter_map(|s| parse_selector(s, ua)).collect();
                    if !selectors.is_empty() {
                        rules.push(Rule { selectors, decls: parse_decls(block), order: *order, ua });
                        *order += 1;
                    }
                }
                i = j;
                start = i;
            }
            b';' => {
                start = i + 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
}

/// Evaluate an `@media` query against the print media context (comma = OR).
fn eval_media(query: &str, m: &MediaCtx) -> bool {
    query.split(',').any(|part| eval_media_part(part.trim(), m))
}

fn eval_media_part(part: &str, m: &MediaCtx) -> bool {
    let p = part.to_ascii_lowercase();
    let negate = p.trim_start().starts_with("not ");
    let p = p.trim_start().trim_start_matches("not ").trim_start_matches("only ").trim();

    let has_print = p.contains("print");
    let has_screen = p.contains("screen");
    let has_all = p.contains("all");
    // media type ok for print rendering unless it's screen-only
    let type_ok = if has_screen && !has_print && !has_all { false } else { m.print || has_all };

    let mut feat_ok = true;
    for (key, val) in feature_conditions(p) {
        if let Some(px) = length_px(&val, 16.0, 16.0) {
            match key.as_str() {
                "min-width" => feat_ok &= m.width_px >= px,
                "max-width" => feat_ok &= m.width_px <= px,
                _ => {}
            }
        }
    }
    let result = type_ok && feat_ok;
    if negate { !result } else { result }
}

fn feature_conditions(p: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = p;
    while let Some(open) = rest.find('(') {
        let after = &rest[open + 1..];
        let close = match after.find(')') {
            Some(c) => c,
            None => break,
        };
        let inner = &after[..close];
        if let Some((k, v)) = inner.split_once(':') {
            out.push((k.trim().to_string(), v.trim().to_string()));
        }
        rest = &after[close + 1..];
    }
    out
}

fn parse_decls(block: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for decl in block.split(';') {
        if let Some((k, v)) = decl.split_once(':') {
            let k = k.trim().to_ascii_lowercase();
            let mut v = v.trim();
            if let Some(p) = v.to_ascii_lowercase().rfind("!important") {
                v = v[..p].trim_end();
            }
            if !k.is_empty() && !v.is_empty() {
                out.push((k, v.trim().to_string()));
            }
        }
    }
    out
}

// ----- matching -----

fn elem_tag(node: &NodeRef) -> Option<String> {
    node.as_element().map(|e| e.name.local.to_string().to_ascii_lowercase())
}
fn elem_id(node: &NodeRef) -> Option<String> {
    node.as_element().and_then(|e| e.attributes.borrow().get("id").map(|s| s.to_string()))
}
fn elem_classes(node: &NodeRef) -> Vec<String> {
    node.as_element()
        .and_then(|e| e.attributes.borrow().get("class").map(|s| s.split_whitespace().map(|t| t.to_string()).collect()))
        .unwrap_or_default()
}

fn compound_matches(c: &Compound, node: &NodeRef) -> bool {
    if let Some(t) = &c.tag
        && elem_tag(node).as_deref() != Some(t.as_str()) {
            return false;
        }
    if let Some(id) = &c.id
        && elem_id(node).as_deref() != Some(id.as_str()) {
            return false;
        }
    if !c.classes.is_empty() {
        let classes = elem_classes(node);
        if !c.classes.iter().all(|c| classes.iter().any(|k| k == c)) {
            return false;
        }
    }
    true
}

fn selector_matches(sel: &Selector, node: &NodeRef) -> bool {
    let last = sel.compounds.len() - 1;
    if !compound_matches(&sel.compounds[last], node) {
        return false;
    }
    // Walk ancestors satisfying earlier compounds (descendant combinator only).
    let mut idx = last;
    let mut current = node.parent();
    while idx > 0 {
        idx -= 1;
        let want = &sel.compounds[idx];
        let mut matched = false;
        while let Some(anc) = current.clone() {
            current = anc.parent();
            if anc.as_element().is_some() && compound_matches(want, &anc) {
                matched = true;
                break;
            }
        }
        if !matched {
            return false;
        }
    }
    true
}

/// A matched rule during cascade: (is_ua, specificity, source-order, declarations).
type Matched<'a> = (bool, (u32, u32, u32), usize, &'a [(String, String)]);

impl Stylesheet {
    /// Declarations matching `node`, tagged with (ua, specificity, order) for cascade sorting.
    fn matched<'a>(&'a self, node: &NodeRef, out: &mut Vec<Matched<'a>>) {
        for rule in &self.rules {
            if rule.selectors.iter().any(|s| selector_matches(s, node)) {
                let spec = rule
                    .selectors
                    .iter()
                    .filter(|s| selector_matches(s, node))
                    .map(|s| s.spec)
                    .max()
                    .unwrap();
                out.push((rule.ua, spec, rule.order, &rule.decls));
            }
        }
    }
}

/// Resolve the computed style of `node` given its parent's computed style and the sheets.
pub fn compute(node: &NodeRef, parent: &ComputedStyle, sheets: &[&Stylesheet]) -> ComputedStyle {
    let mut style = ComputedStyle::inherited_from(parent);

    let mut matched: Vec<Matched> = Vec::new();
    for sheet in sheets {
        sheet.matched(node, &mut matched);
    }
    // Apply in increasing priority so the winner is applied last: author beats UA,
    // then higher specificity, then later source order.
    matched.sort_by(|a, b| {
        let ka = (!a.0, a.1, a.2); // author (ua=false → !ua=true) ranks above UA
        let kb = (!b.0, b.1, b.2);
        ka.cmp(&kb)
    });

    for (_, _, _, decls) in &matched {
        for (name, value) in *decls {
            apply(&mut style, name, value, parent);
        }
    }

    // Inline style attribute wins last.
    if let Some(el) = node.as_element()
        && let Some(inline) = el.attributes.borrow().get("style") {
            for (name, value) in parse_decls(inline) {
                apply(&mut style, &name, &value, parent);
            }
        }

    if style.bold_default && style.font_weight == 400 {
        style.font_weight = 700;
    }
    style
}

fn apply(s: &mut ComputedStyle, name: &str, value: &str, parent: &ComputedStyle) {
    let v = value.trim();
    match name {
        "display" => {
            s.display = match v {
                "block" => Display::Block,
                "inline" => Display::Inline,
                "inline-block" => Display::InlineBlock,
                "list-item" => Display::ListItem,
                "table" => Display::Table,
                "table-row-group" | "thead" | "tbody" | "tfoot" => Display::TableRowGroup,
                "table-row" => Display::TableRow,
                "table-cell" => Display::TableCell,
                "none" => Display::None,
                _ => s.display,
            }
        }
        "font-size" => {
            if let Some(px) = length_px(v, parent.font_size, parent.font_size) {
                s.font_size = px;
            }
        }
        "font-family" => {
            s.font_family = v.split(',').map(|f| f.trim().trim_matches(['"', '\'']).to_string()).collect();
        }
        "font-weight" => {
            s.font_weight = match v {
                "normal" => 400,
                "bold" => 700,
                "bolder" => 700,
                "lighter" => 300,
                n => n.parse().unwrap_or(s.font_weight),
            }
        }
        "font-style" => s.italic = v == "italic" || v == "oblique",
        "font" => { /* shorthand: skip for now, rely on longhands */ }
        "line-height" => {
            if v == "normal" {
                s.line_height = None;
            } else if let Ok(mult) = v.parse::<f32>() {
                s.line_height = Some(mult * s.font_size);
            } else if let Some(px) = length_px(v, s.font_size, parent.font_size) {
                s.line_height = Some(px);
            }
        }
        "color" => {
            if let Some(c) = parse_color(v, s.color) {
                s.color = c;
            }
        }
        "background-color" | "background" | "background-image" => {
            if v.contains("linear-gradient") {
                s.background_gradient = parse_gradient(v, s.color);
            } else if v.starts_with("url") {
                // image backgrounds unsupported; leave as-is
            } else if v == "transparent" || v == "none" {
                s.background = None;
            } else if let Some(c) = parse_color(v, s.color) {
                s.background = Some(c);
            }
        }
        "text-align" => {
            s.text_align = match v {
                "right" => TextAlign::Right,
                "center" => TextAlign::Center,
                "justify" => TextAlign::Justify,
                _ => TextAlign::Left,
            }
        }
        "vertical-align" => {
            s.vertical_align = match v {
                "top" => VerticalAlign::Top,
                "middle" => VerticalAlign::Middle,
                "bottom" => VerticalAlign::Bottom,
                _ => VerticalAlign::Baseline,
            }
        }
        "white-space" => s.white_space_nowrap = v == "nowrap" || v == "pre",
        "width" => {
            if let Some(p) = percent(v) {
                s.width_percent = Some(p);
                s.width = None;
            } else if v == "auto" {
                s.width = None;
                s.width_percent = None;
            } else if let Some(px) = length_px(v, s.font_size, parent.font_size) {
                s.width = Some(px);
                s.width_percent = None;
            }
        }
        "height" => {
            if let Some(px) = length_px(v, s.font_size, parent.font_size) {
                s.height = Some(px);
            }
        }
        "margin" => set_edges(&mut s.margin, v, s.font_size, parent.font_size),
        "margin-top" => s.margin.top = length_px(v, s.font_size, parent.font_size).unwrap_or(s.margin.top),
        "margin-right" => s.margin.right = length_px(v, s.font_size, parent.font_size).unwrap_or(s.margin.right),
        "margin-bottom" => s.margin.bottom = length_px(v, s.font_size, parent.font_size).unwrap_or(s.margin.bottom),
        "margin-left" => s.margin.left = length_px(v, s.font_size, parent.font_size).unwrap_or(s.margin.left),
        "padding" => set_edges(&mut s.padding, v, s.font_size, parent.font_size),
        "padding-top" => s.padding.top = length_px(v, s.font_size, parent.font_size).unwrap_or(s.padding.top),
        "padding-right" => s.padding.right = length_px(v, s.font_size, parent.font_size).unwrap_or(s.padding.right),
        "padding-bottom" => s.padding.bottom = length_px(v, s.font_size, parent.font_size).unwrap_or(s.padding.bottom),
        "padding-left" => s.padding.left = length_px(v, s.font_size, parent.font_size).unwrap_or(s.padding.left),
        "border" => set_border_all(s, v),
        "border-top" => set_border_side(&mut s.border.top, v),
        "border-right" => set_border_side(&mut s.border.right, v),
        "border-bottom" => set_border_side(&mut s.border.bottom, v),
        "border-left" => set_border_side(&mut s.border.left, v),
        "border-width" => {
            let w = length_px(v, s.font_size, parent.font_size).unwrap_or(0.0);
            for b in [&mut s.border.top, &mut s.border.right, &mut s.border.bottom, &mut s.border.left] {
                b.width = w;
                b.present = w > 0.0;
            }
        }
        "border-color" => {
            if let Some(c) = parse_color(v, s.color) {
                for b in [&mut s.border.top, &mut s.border.right, &mut s.border.bottom, &mut s.border.left] {
                    b.color = c;
                }
            }
        }
        "float" => {
            s.float = match v {
                "left" => Float::Left,
                "right" => Float::Right,
                _ => Float::None,
            }
        }
        "clear" => s.clear = v == "both" || v == "left" || v == "right",
        "page-break-after" | "break-after" => s.page_break_after = v == "always" || v == "page",
        "page-break-before" | "break-before" => s.page_break_before = v == "always" || v == "page",
        _ => {}
    }
}

fn set_edges(e: &mut Edges, v: &str, fs: f32, pfs: f32) {
    let parts: Vec<f32> = v.split_whitespace().map(|p| length_px(p, fs, pfs).unwrap_or(0.0)).collect();
    match parts.len() {
        1 => *e = Edges { top: parts[0], right: parts[0], bottom: parts[0], left: parts[0] },
        2 => *e = Edges { top: parts[0], right: parts[1], bottom: parts[0], left: parts[1] },
        3 => *e = Edges { top: parts[0], right: parts[1], bottom: parts[2], left: parts[1] },
        4 => *e = Edges { top: parts[0], right: parts[1], bottom: parts[2], left: parts[3] },
        _ => {}
    }
}

fn set_border_all(s: &mut ComputedStyle, v: &str) {
    let mut b = Border::NONE;
    set_border_side(&mut b, v);
    s.border = Borders { top: b, right: b, bottom: b, left: b };
}

fn set_border_side(b: &mut Border, v: &str) {
    // e.g. "1px solid #ddd"
    let mut width = 1.0;
    let mut color = b.color;
    const STYLES: [&str; 8] = ["solid", "dashed", "dotted", "double", "groove", "ridge", "inset", "outset"];
    for tok in v.split_whitespace() {
        if let Some(px) = length_px(tok, 16.0, 16.0) {
            width = px;
        } else if matches!(tok, "none" | "hidden") {
            b.present = false;
            b.width = 0.0;
            return;
        } else if STYLES.contains(&tok) {
            // recognized border-style keyword (consumed so it isn't parsed as a color)
        } else if let Some(c) = parse_color(tok, color) {
            color = c;
        }
    }
    b.width = width;
    b.color = color;
    b.present = width > 0.0;
}

/// Parse a two-stop `linear-gradient(...)`; direction reduced to horizontal/vertical.
fn parse_gradient(v: &str, current: Rgba) -> Option<Gradient> {
    let start = v.find("linear-gradient(")? + "linear-gradient(".len();
    let end = v.rfind(')')?;
    let inner = &v[start..end];
    let parts: Vec<&str> = inner.split(',').map(|p| p.trim()).collect();
    let mut horizontal = false;
    let mut colors: Vec<Rgba> = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        if i == 0 && (part.contains("deg") || part.starts_with("to ")) {
            let p = part.to_ascii_lowercase();
            horizontal = p.contains("right") || p.contains("left") || p.contains("90deg") || p.contains("270deg");
            continue;
        }
        // strip a trailing stop position ("#f00 40%")
        let color_tok = part.split_whitespace().next().unwrap_or(part);
        if let Some(c) = parse_color(color_tok, current) {
            colors.push(c);
        }
    }
    if colors.len() < 2 {
        return None;
    }
    Some(Gradient { from: colors[0], to: *colors.last().unwrap(), horizontal })
}

fn percent(v: &str) -> Option<f32> {
    v.strip_suffix('%').and_then(|n| n.trim().parse::<f32>().ok()).map(|p| p / 100.0)
}

/// Resolve a CSS length to px. em/rem relative to font sizes; pt/mm/cm/in absolute.
pub fn length_px(v: &str, em_base: f32, root_em: f32) -> Option<f32> {
    let v = v.trim();
    if v == "0" {
        return Some(0.0);
    }
    for (unit, factor) in [("px", 1.0), ("pt", 96.0 / 72.0), ("mm", 96.0 / 25.4), ("cm", 96.0 / 2.54), ("in", 96.0), ("q", 96.0 / 101.6)] {
        if let Some(num) = v.strip_suffix(unit) {
            return num.trim().parse::<f32>().ok().map(|n| n * factor);
        }
    }
    if let Some(num) = v.strip_suffix("rem") {
        return num.trim().parse::<f32>().ok().map(|n| n * root_em);
    }
    if let Some(num) = v.strip_suffix("em") {
        return num.trim().parse::<f32>().ok().map(|n| n * em_base);
    }
    // Unitless number: treat as px (common in HTML attributes)
    v.parse::<f32>().ok()
}

pub fn parse_color(v: &str, current: Rgba) -> Option<Rgba> {
    let v = v.trim();
    if v.eq_ignore_ascii_case("currentcolor") {
        return Some(current);
    }
    if let Some(hex) = v.strip_prefix('#') {
        return parse_hex(hex);
    }
    if let Some(inner) = v.strip_prefix("rgb(").and_then(|s| s.strip_suffix(')')) {
        let c: Vec<f32> = inner.split(',').filter_map(|p| p.trim().parse().ok()).collect();
        if c.len() == 3 {
            return Some([c[0] as u8, c[1] as u8, c[2] as u8, 255]);
        }
    }
    if let Some(inner) = v.strip_prefix("rgba(").and_then(|s| s.strip_suffix(')')) {
        let c: Vec<f32> = inner.split(',').filter_map(|p| p.trim().parse().ok()).collect();
        if c.len() == 4 {
            return Some([c[0] as u8, c[1] as u8, c[2] as u8, (c[3] * 255.0) as u8]);
        }
    }
    named_color(&v.to_ascii_lowercase())
}

fn parse_hex(hex: &str) -> Option<Rgba> {
    let h = hex.trim();
    let bytes = |s: &str| u8::from_str_radix(s, 16).ok();
    match h.len() {
        3 => {
            let r = bytes(&h[0..1].repeat(2))?;
            let g = bytes(&h[1..2].repeat(2))?;
            let b = bytes(&h[2..3].repeat(2))?;
            Some([r, g, b, 255])
        }
        6 => Some([bytes(&h[0..2])?, bytes(&h[2..4])?, bytes(&h[4..6])?, 255]),
        8 => Some([bytes(&h[0..2])?, bytes(&h[2..4])?, bytes(&h[4..6])?, bytes(&h[6..8])?]),
        _ => None,
    }
}

fn named_color(name: &str) -> Option<Rgba> {
    let c = match name {
        "black" => [0, 0, 0],
        "white" => [255, 255, 255],
        "red" => [255, 0, 0],
        "green" => [0, 128, 0],
        "blue" => [0, 0, 255],
        "gray" | "grey" => [128, 128, 128],
        "lightgray" | "lightgrey" => [211, 211, 211],
        "darkgray" | "darkgrey" => [169, 169, 169],
        "silver" => [192, 192, 192],
        "orange" => [255, 165, 0],
        "yellow" => [255, 255, 0],
        "navy" => [0, 0, 128],
        "teal" => [0, 128, 128],
        "maroon" => [128, 0, 0],
        "purple" => [128, 0, 128],
        "transparent" => return Some([0, 0, 0, 0]),
        _ => return None,
    };
    Some([c[0], c[1], c[2], 255])
}

/// The built-in user-agent stylesheet (a minimal subset).
pub fn ua_stylesheet() -> Stylesheet {
    parse_stylesheet(UA_CSS, true, 0, &MediaCtx::NONE)
}

const UA_CSS: &str = r#"
html, body, div, p, h1, h2, h3, h4, h5, h6, table, tr, td, th, ul, ol, li, header, footer, section, article { display: block; }
span, a, b, strong, i, em, small, code, img, sub, sup, label { display: inline; }
table { display: table; }
tr { display: table-row; }
thead, tbody, tfoot { display: table-row-group; }
td, th { display: table-cell; padding: 1px; }
body { margin: 8px; }
p { margin-top: 16px; margin-bottom: 16px; }
h1 { font-size: 2em; margin-top: 21px; margin-bottom: 21px; font-weight: bold; }
h2 { font-size: 1.5em; margin-top: 20px; margin-bottom: 20px; font-weight: bold; }
h3 { font-size: 1.17em; margin-top: 19px; margin-bottom: 19px; font-weight: bold; }
h4 { margin-top: 21px; margin-bottom: 21px; font-weight: bold; }
h5 { font-size: 0.83em; margin-top: 22px; margin-bottom: 22px; font-weight: bold; }
h6 { font-size: 0.67em; margin-top: 25px; margin-bottom: 25px; font-weight: bold; }
b, strong, th { font-weight: bold; }
i, em { font-style: italic; }
th { text-align: center; }
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors() {
        assert_eq!(parse_color("#f00", [0, 0, 0, 255]), Some([255, 0, 0, 255]));
        assert_eq!(parse_color("#00ff00", [0, 0, 0, 255]), Some([0, 255, 0, 255]));
        assert_eq!(parse_color("rgb(1, 2, 3)", [0, 0, 0, 255]), Some([1, 2, 3, 255]));
        assert_eq!(parse_color("rgba(1,2,3,0.5)", [0, 0, 0, 255]), Some([1, 2, 3, 127]));
        assert_eq!(parse_color("white", [0, 0, 0, 255]), Some([255, 255, 255, 255]));
        assert_eq!(parse_color("currentcolor", [9, 9, 9, 255]), Some([9, 9, 9, 255]));
    }

    #[test]
    fn lengths() {
        assert_eq!(length_px("0", 16.0, 16.0), Some(0.0));
        assert_eq!(length_px("12px", 16.0, 16.0), Some(12.0));
        assert_eq!(length_px("72pt", 16.0, 16.0), Some(96.0));
        assert_eq!(length_px("1in", 16.0, 16.0), Some(96.0));
        assert_eq!(length_px("2em", 16.0, 16.0), Some(32.0));
        assert!((length_px("25.4mm", 16.0, 16.0).unwrap() - 96.0).abs() < 0.01);
    }

    #[test]
    fn media_print_vs_screen() {
        let m = MediaCtx { print: true, width_px: 794.0 };
        assert!(eval_media("print", &m));
        assert!(!eval_media("screen", &m));
        assert!(eval_media("print, screen", &m));
        assert!(eval_media("(min-width: 500px)", &m));
        assert!(!eval_media("(min-width: 900px)", &m));
        assert!(!eval_media("only screen and (max-width: 600px)", &m));
    }

    #[test]
    fn selectors_pseudo_and_attr() {
        // Pseudo-element and attribute-only selectors are invalid (must not match anything).
        assert!(parse_selector(".row:before", false).is_none());
        assert!(parse_selector("[hidden]", false).is_none());
        assert!(parse_selector(":root", false).is_none());
        // Pseudo-class keeps the base compound; universal and normal selectors parse.
        assert!(parse_selector(".row:not(.x)", false).is_some());
        assert!(parse_selector("div:hover", false).is_some());
        assert!(parse_selector("*", false).is_some());
        assert!(parse_selector(".a > .b", false).is_some());
    }
}
