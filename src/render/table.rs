//! Table layout: auto column widths from content, presentational border/cellpadding
//! attributes, and a collapsed 1px grid (the common print case).

use kuchikiki::NodeRef;

use super::layout::{Engine, Flow, Item, push_borders};
use super::style::{Border, ComputedStyle, Display, Rgba, compute};

struct Cell {
    node: NodeRef,
    style: ComputedStyle,
}

fn attr(node: &NodeRef, name: &str) -> Option<String> {
    node.as_element().and_then(|e| e.attributes.borrow().get(name).map(|s| s.to_string()))
}

pub fn layout(eng: &mut Engine, node: &NodeRef, style: &ComputedStyle, x: f32, y: f32, avail_width: f32) -> Flow {
    let border_attr = attr(node, "border").and_then(|v| v.trim().parse::<f32>().ok()).filter(|&n| n > 0.0);
    let cellpadding = attr(node, "cellpadding").and_then(|v| v.trim().parse::<f32>().ok());

    let bl = style.border.left.eff_width();
    let br = style.border.right.eff_width();
    let bt = style.border.top.eff_width();

    let table_left = x + style.margin.left;
    let content_left = table_left + bl + style.padding.left;
    let content_top = y + style.margin.top + bt + style.padding.top;
    let outer_avail = (avail_width - style.margin.left - style.margin.right - bl - br - style.padding.left - style.padding.right).max(0.0);

    let mut rows: Vec<Vec<Cell>> = Vec::new();
    collect_rows(eng, node, style, &mut rows);
    apply_presentational(&mut rows, border_attr, cellpadding);
    if rows.is_empty() {
        return Flow { items: vec![], height: 0.0, breaks: vec![] };
    }
    let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);

    // Collapsed grid line (1px) when the table declares a border.
    let grid: Option<Border> = border_attr
        .map(|_| Border { width: 1.0, color: [0, 0, 0, 255], present: true })
        .or_else(|| rows.iter().flatten().map(|c| c.style.border.top).find(|b| b.present));
    let collapse = grid.is_some();
    let gw = grid.map(|g| g.width).unwrap_or(0.0);

    // Preferred column widths from cell content.
    let mut pref = vec![0f32; cols];
    for row in &rows {
        for (ci, cell) in row.iter().enumerate() {
            let extra = cell.style.padding.left + cell.style.padding.right;
            let natural = eng.measure_text_width(&cell.node, &cell.style).ceil() + extra;
            pref[ci] = pref[ci].max(cell.style.width.map(|w| w + extra).unwrap_or(natural));
        }
    }
    let total_pref: f32 = pref.iter().sum::<f32>() + gw * (cols as f32 + 1.0);
    let want_full = style.width.is_some() || style.width_percent.is_some();
    let col_w: Vec<f32> = if total_pref > outer_avail || want_full {
        let usable = (outer_avail - gw * (cols as f32 + 1.0)).max(0.0);
        let sum: f32 = pref.iter().sum::<f32>().max(1.0);
        pref.iter().map(|w| w / sum * usable).collect()
    } else {
        pref.clone()
    };

    let mut content = Vec::new();
    let mut breaks = Vec::new();
    let mut row_tops = Vec::new();
    let mut cursor_y = content_top + gw;

    for row in &rows {
        row_tops.push(cursor_y);
        let mut cx = content_left + gw;
        let mut row_h = 0f32;
        let mut laid: Vec<(f32, Flow, f32)> = Vec::new();
        for (ci, cell) in row.iter().enumerate() {
            let cw = col_w.get(ci).copied().unwrap_or(0.0);
            let cell_content_w = (cw - cell.style.padding.left - cell.style.padding.right).max(0.0);
            let content_x = cx + cell.style.padding.left;
            let content_y = cursor_y + cell.style.padding.top;
            let flow = eng.layout_flow(&cell.node, &cell.style, content_x, content_y, cell_content_w);
            let cell_h = flow.height + cell.style.padding.top + cell.style.padding.bottom;
            row_h = row_h.max(cell.style.height.unwrap_or(cell_h));
            laid.push((cx, flow, cw));
            cx += cw + gw;
        }
        for (ci, (col_x, flow, cw)) in laid.into_iter().enumerate() {
            if let Some(bg) = row[ci].style.background
                && bg[3] > 0 {
                    content.push(Item::Rect { x: col_x, y: cursor_y, w: cw, h: row_h, color: bg });
                }
            if !collapse {
                push_borders(&mut content, &row[ci].style, col_x, cursor_y, cw, row_h);
            }
            content.extend(flow.items);
        }
        cursor_y += row_h + gw;
        breaks.push(cursor_y);
    }

    let grid_w: f32 = col_w.iter().sum::<f32>() + gw * (cols as f32 + 1.0);
    let grid_h: f32 = cursor_y - content_top;

    let mut items = Vec::new();
    if let Some(bg) = style.background
        && bg[3] > 0 {
            items.push(Item::Rect { x: table_left, y: y + style.margin.top, w: grid_w, h: grid_h, color: bg });
        }
    if let Some(g) = grid {
        draw_grid(&mut items, content_left, content_top, &col_w, &row_tops, cursor_y, g);
    } else {
        push_borders(&mut items, style, table_left, y + style.margin.top, grid_w, grid_h);
    }
    items.extend(content);

    let table_h = cursor_y - (y + style.margin.top);
    Flow { items, height: style.margin.top + table_h + style.margin.bottom, breaks }
}

fn draw_grid(items: &mut Vec<Item>, x0: f32, y0: f32, col_w: &[f32], row_tops: &[f32], y_end: f32, g: Border) {
    let color: Rgba = g.color;
    let w = g.width;
    let total_w: f32 = col_w.iter().sum::<f32>() + w * (col_w.len() as f32 + 1.0);
    // Horizontal lines: top, between rows, bottom.
    let mut ys: Vec<f32> = vec![y0];
    for &t in row_tops.iter().skip(1) {
        ys.push(t - w);
    }
    ys.push(y_end - w);
    for y in ys {
        items.push(Item::Rect { x: x0, y, w: total_w, h: w, color });
    }
    // Vertical lines: left, between columns, right.
    let mut xs: Vec<f32> = vec![x0];
    let mut cx = x0 + w;
    for cwv in col_w {
        cx += cwv;
        xs.push(cx);
        cx += w;
    }
    for x in xs {
        items.push(Item::Rect { x, y: y0, w, h: y_end - y0 - w, color });
    }
}

fn apply_presentational(rows: &mut [Vec<Cell>], border_attr: Option<f32>, cellpadding: Option<f32>) {
    for row in rows.iter_mut() {
        for cell in row.iter_mut() {
            if let Some(pad) = cellpadding {
                cell.style.padding.top = pad;
                cell.style.padding.right = pad;
                cell.style.padding.bottom = pad;
                cell.style.padding.left = pad;
            }
            if border_attr.is_some() && !cell.style.border.top.present {
                let b = Border { width: 1.0, color: [0, 0, 0, 255], present: true };
                cell.style.border.top = b;
                cell.style.border.right = b;
                cell.style.border.bottom = b;
                cell.style.border.left = b;
            }
        }
    }
}

fn collect_rows(eng: &mut Engine, node: &NodeRef, parent_style: &ComputedStyle, rows: &mut Vec<Vec<Cell>>) {
    for child in node.children() {
        if child.as_element().is_none() {
            continue;
        }
        let style = compute(&child, parent_style, &eng.sheets);
        match style.display {
            Display::TableRow => {
                let mut cells = Vec::new();
                for c in child.children() {
                    if c.as_element().is_some() {
                        let cs = compute(&c, &style, &eng.sheets);
                        if cs.display == Display::TableCell {
                            cells.push(Cell { node: c.clone(), style: cs });
                        }
                    }
                }
                if !cells.is_empty() {
                    rows.push(cells);
                }
            }
            Display::TableRowGroup | Display::Table => collect_rows(eng, &child, &style, rows),
            _ => {}
        }
    }
}
