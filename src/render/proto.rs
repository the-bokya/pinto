//! Prototype: validates fonts -> cosmic-text shaping -> krilla glyphs + rects -> PDF.

use anyhow::{Result, anyhow};
use cosmic_text::{Attrs, Buffer, Family, Metrics, Shaping};
use krilla::Document;
use krilla::color::rgb;
use krilla::geom::{Point, Rect, Size};
use krilla::num::NormalizedF32;
use krilla::page::PageSettings;
use krilla::paint::{Fill, FillRule};
use krilla::text::GlyphId;

use super::PX_TO_PT;
use super::fonts::Fonts;

fn fill_rgb(r: u8, g: u8, b: u8) -> Fill {
    Fill {
        paint: rgb::Color::new(r, g, b).into(),
        opacity: NormalizedF32::ONE,
        rule: FillRule::NonZero,
    }
}

fn px_rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect::from_xywh(x * PX_TO_PT, y * PX_TO_PT, w * PX_TO_PT, h * PX_TO_PT).unwrap()
}

pub fn render(out_path: &str) -> Result<()> {
    let mut fonts = Fonts::new();

    // A4 in points.
    let page_w = 210.0 / 25.4 * 72.0;
    let page_h = 297.0 / 25.4 * 72.0;

    let mut doc = Document::new();
    let mut page = doc.start_page_with(PageSettings::new(Size::from_wh(page_w, page_h).unwrap()));
    let mut surface = page.surface();

    // Background band + a bordered box (filled rects).
    surface.set_fill(Some(fill_rgb(0xd1, 0xf0, 0xff)));
    let mut pb = krilla::geom::PathBuilder::new();
    pb.push_rect(px_rect(40.0, 40.0, 515.0, 40.0));
    if let Some(path) = pb.finish() {
        surface.draw_path(&path);
    }

    // Shape a paragraph.
    let font_px = 16.0; // 12pt
    let line_px = 20.0;
    let text = "Hello Pinto — a self-contained HTML→PDF renderer. \
                The quick brown fox jumps over the lazy dog 0123456789.";
    let mut buffer = Buffer::new(&mut fonts.system, Metrics::new(font_px, line_px));
    buffer.set_size(Some(515.0), None);
    buffer.set_text(
        text,
        &Attrs::new().family(Family::SansSerif),
        Shaping::Advanced,
        None,
    );
    buffer.shape_until_scroll(&mut fonts.system, false);

    let origin_x = 45.0_f32;
    let origin_y = 100.0_f32;

    surface.set_fill(Some(fill_rgb(0x20, 0x20, 0x20)));
    let runs: Vec<_> = buffer
        .layout_runs()
        .map(|run| {
            let glyphs: Vec<_> = run
                .glyphs
                .iter()
                .map(|g| (g.font_id, g.glyph_id, g.x, g.font_size, g.start..g.end))
                .collect();
            (run.line_y, glyphs)
        })
        .collect();

    for (line_y, glyphs) in runs {
        let baseline = origin_y + line_y;
        for (font_id, glyph_id, gx, gsize, range) in glyphs {
            let Some(font) = fonts.krilla_font(font_id) else { continue };
            let kg = krilla::text::KrillaGlyph::new(
                GlyphId::new(glyph_id as u32),
                0.0,
                0.0,
                0.0,
                0.0,
                range,
                None,
            );
            surface.draw_glyphs(
                Point::from_xy((origin_x + gx) * PX_TO_PT, baseline * PX_TO_PT),
                &[kg],
                font,
                text,
                gsize * PX_TO_PT,
                false,
            );
        }
    }

    surface.finish();
    page.finish();
    let pdf = doc.finish().map_err(|e| anyhow!("krilla finish: {e:?}"))?;
    std::fs::write(out_path, pdf)?;
    Ok(())
}
