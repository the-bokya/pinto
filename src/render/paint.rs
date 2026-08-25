//! Paint pages of display-list items to a PDF via krilla. Items are in CSS px (y-down).

use anyhow::{Result, anyhow};
use krilla::Document;
use krilla::color::rgb;
use krilla::geom::{Point, Rect, Size, Transform};
use krilla::num::NormalizedF32;
use krilla::page::PageSettings;
use krilla::paint::{Fill, FillRule};
use krilla::text::{GlyphId, KrillaGlyph};

use super::PX_TO_PT;
use super::fonts::Fonts;
use super::image as img;
use super::layout::Item;
use super::style::Rgba;

pub struct Page {
    pub width_px: f32,
    pub height_px: f32,
    pub items: Vec<Item>,
}

fn fill(color: Rgba) -> Fill {
    Fill {
        paint: rgb::Color::new(color[0], color[1], color[2]).into(),
        opacity: NormalizedF32::new(color[3] as f32 / 255.0).unwrap_or(NormalizedF32::ONE),
        rule: FillRule::NonZero,
    }
}

pub fn paint(pages: &[Page], fonts: &mut Fonts) -> Result<Vec<u8>> {
    let mut doc = Document::new();
    for page in pages {
        let size = Size::from_wh(page.width_px * PX_TO_PT, page.height_px * PX_TO_PT).unwrap();
        let mut kpage = doc.start_page_with(PageSettings::new(size));
        let mut surface = kpage.surface();

        for item in &page.items {
            match item {
                Item::Rect { x, y, w, h, color } => {
                    if *w <= 0.0 || *h <= 0.0 || color[3] == 0 {
                        continue;
                    }
                    surface.set_fill(Some(fill(*color)));
                    let mut pb = krilla::geom::PathBuilder::new();
                    pb.push_rect(Rect::from_xywh(x * PX_TO_PT, y * PX_TO_PT, w * PX_TO_PT, h * PX_TO_PT).unwrap());
                    if let Some(path) = pb.finish() {
                        surface.draw_path(&path);
                    }
                }
                Item::Gradient { x, y, w, h, grad } => {
                    if *w <= 0.0 || *h <= 0.0 {
                        continue;
                    }
                    let (px, py, pw, ph) = (x * PX_TO_PT, y * PX_TO_PT, w * PX_TO_PT, h * PX_TO_PT);
                    let (x2, y2) = if grad.horizontal { (px + pw, py) } else { (px, py + ph) };
                    let stop = |off: f32, c: Rgba| krilla::paint::Stop {
                        offset: NormalizedF32::new(off).unwrap_or(NormalizedF32::ZERO),
                        color: rgb::Color::new(c[0], c[1], c[2]).into(),
                        opacity: NormalizedF32::ONE,
                    };
                    let lg = krilla::paint::LinearGradient {
                        x1: px,
                        y1: py,
                        x2,
                        y2,
                        transform: Transform::from_translate(0.0, 0.0),
                        spread_method: krilla::paint::SpreadMethod::Pad,
                        stops: vec![stop(0.0, grad.from), stop(1.0, grad.to)],
                        anti_alias: true,
                    };
                    surface.set_fill(Some(Fill {
                        paint: lg.into(),
                        opacity: NormalizedF32::ONE,
                        rule: FillRule::NonZero,
                    }));
                    let mut pb = krilla::geom::PathBuilder::new();
                    pb.push_rect(Rect::from_xywh(px, py, pw, ph).unwrap());
                    if let Some(path) = pb.finish() {
                        surface.draw_path(&path);
                    }
                }
                Item::Glyph { font, gid, x, y, size, color } => {
                    let Some(kfont) = fonts.krilla_font(*font) else { continue };
                    surface.set_fill(Some(fill(*color)));
                    let g = KrillaGlyph::new(GlyphId::new(*gid as u32), 0.0, 0.0, 0.0, 0.0, 0..0, None);
                    surface.draw_glyphs(
                        Point::from_xy(x * PX_TO_PT, y * PX_TO_PT),
                        &[g],
                        kfont,
                        "",
                        size * PX_TO_PT,
                        false,
                    );
                }
                Item::Image { x, y, w, h, src } => {
                    let Some(decoded) = img::load(src) else { continue };
                    let image = krilla::image::Image::from_rgba8(decoded.rgba, decoded.w, decoded.h);
                    surface.push_transform(&Transform::from_translate(x * PX_TO_PT, y * PX_TO_PT));
                    surface.draw_image(image, Size::from_wh(w * PX_TO_PT, h * PX_TO_PT).unwrap());
                    surface.pop();
                }
            }
        }
        surface.finish();
        kpage.finish();
    }
    doc.finish().map_err(|e| anyhow!("krilla finish: {e:?}"))
}
