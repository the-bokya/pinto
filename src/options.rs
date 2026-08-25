//! Unit conversion, page-size table, and options-map helpers.
//! Ports frappe/utils/print_utils.py (convert_uom, parse_float_and_unit) and
//! the PageSize table from frappe/utils/pdf_generator/browser.py.

use serde_json::{Map, Value};

/// Round to 3 decimals, matching Python's `round(x, 3)` closely enough for geometry.
pub fn round3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

fn unit_value(unit: &str) -> f64 {
    match unit {
        "px" => 1.0,
        "mm" => 3.7795275591,
        "cm" => 37.795275591,
        "in" => 96.0,
        _ => 1.0,
    }
}

/// convert_uom: factor(from, to) = unit_values[from] / unit_values[to].
pub fn convert_uom(number: f64, from_uom: &str, to_uom: &str) -> f64 {
    round3(number * (unit_value(from_uom) / unit_value(to_uom)))
}

pub struct FloatUnit {
    pub value: f64,
    pub unit: String,
}

/// parse_float_and_unit: first numeric match + single recognized unit (px/mm/cm/in).
pub fn parse_float_and_unit(input: &str, default_unit: &str) -> Option<FloatUnit> {
    let value = first_number(input)?;
    let units = ["px", "mm", "cm", "in"];
    let matched: Vec<&str> = units.into_iter().filter(|u| input.contains(u)).collect();
    let unit = if matched.len() == 1 {
        matched[0].to_string()
    } else {
        default_unit.to_string()
    };
    Some(FloatUnit { value, unit })
}

/// Extract the first `[+-]?([0-9]*[.])?[0-9]+` occurrence, mirroring Frappe's regex.
fn first_number(text: &str) -> Option<f64> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c.is_ascii_digit() || ((c == b'+' || c == b'-' || c == b'.') && starts_number(bytes, i)) {
            let start = i;
            if bytes[i] == b'+' || bytes[i] == b'-' {
                i += 1;
            }
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b'.' {
                i += 1;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
            }
            return std::str::from_utf8(&bytes[start..i]).ok()?.parse().ok();
        }
        i += 1;
    }
    None
}

fn starts_number(bytes: &[u8], i: usize) -> bool {
    // A sign or dot begins a number only if a digit follows (allowing one dot after a sign).
    let mut j = i + 1;
    if (bytes[i] == b'+' || bytes[i] == b'-') && j < bytes.len() && bytes[j] == b'.' {
        j += 1;
    }
    j < bytes.len() && bytes[j].is_ascii_digit()
}

/// Named page sizes in millimetres (width, height). Ports PageSize.page_sizes.
pub fn page_size_mm(name: &str) -> Option<(f64, f64)> {
    let (w, h): (f64, f64) = match name {
        "A10" => (26.0, 37.0),
        "A1" => (594.0, 841.0),
        "A0" => (841.0, 1189.0),
        "A3" => (297.0, 420.0),
        "A2" => (420.0, 594.0),
        "A5" => (148.0, 210.0),
        "A4" => (210.0, 297.0),
        "A7" => (74.0, 105.0),
        "A6" => (105.0, 148.0),
        "A9" => (37.0, 52.0),
        "A8" => (52.0, 74.0),
        "B10" => (44.0, 31.0),
        "B1+" => (1020.0, 720.0),
        "B4" => (353.0, 250.0),
        "B5" => (250.0, 176.0),
        "B6" => (176.0, 125.0),
        "B7" => (125.0, 88.0),
        "B0" => (1414.0, 1000.0),
        "B1" => (1000.0, 707.0),
        "B2" => (707.0, 500.0),
        "B3" => (500.0, 353.0),
        "B2+" => (720.0, 520.0),
        "B8" => (88.0, 62.0),
        "B9" => (62.0, 44.0),
        "C10" => (40.0, 28.0),
        "C9" => (57.0, 40.0),
        "C8" => (81.0, 57.0),
        "C3" => (458.0, 324.0),
        "C2" => (648.0, 458.0),
        "C1" => (917.0, 648.0),
        "C0" => (1297.0, 917.0),
        "C7" => (114.0, 81.0),
        "C6" => (162.0, 114.0),
        "C5" => (229.0, 162.0),
        "C4" => (324.0, 229.0),
        "Legal" => (216.0, 356.0),
        "Junior Legal" => (127.0, 203.0),
        "Letter" => (216.0, 279.0),
        "Tabloid" => (279.0, 432.0),
        "Ledger" => (432.0, 279.0),
        "ANSI C" => (432.0, 559.0),
        "ANSI A (letter)" => (216.0, 279.0),
        "ANSI B (ledger & tabloid)" => (279.0, 432.0),
        "ANSI E" => (864.0, 1118.0),
        "ANSI D" => (559.0, 864.0),
        _ => return None,
    };
    Some((w, h))
}

/// Read a string-ish option value (str or number) from the options map.
pub fn opt_str(map: &Map<String, Value>, key: &str) -> Option<String> {
    match map.get(key) {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    }
}

/// Read a numeric option value (accepts JSON number or numeric string).
pub fn opt_num(map: &Map<String, Value>, key: &str) -> Option<f64> {
    match map.get(key) {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => parse_float_and_unit(s, "px").map(|fu| fu.value),
        _ => None,
    }
}

pub fn opt_bool(map: &Map<String, Value>, key: &str) -> bool {
    matches!(map.get(key), Some(Value::Bool(true)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn convert_uom_matches_frappe() {
        // 1 in == 96 px
        assert_eq!(convert_uom(1.0, "in", "px"), 96.0);
        // 1 mm == 3.78 px (rounded to 3)
        assert_eq!(convert_uom(1.0, "mm", "px"), 3.78);
        // A4 width 210mm -> px
        assert_eq!(convert_uom(210.0, "mm", "px"), 793.701);
        // px -> in
        assert_eq!(convert_uom(96.0, "px", "in"), 1.0);
    }

    #[test]
    fn parse_units() {
        let a = parse_float_and_unit("15mm", "px").unwrap();
        assert_eq!(a.value, 15.0);
        assert_eq!(a.unit, "mm");
        let b = parse_float_and_unit("0", "px").unwrap();
        assert_eq!(b.value, 0.0);
        assert_eq!(b.unit, "px");
        let c = parse_float_and_unit("10mm", "px").unwrap();
        assert_eq!(c.value, 10.0);
        assert_eq!(c.unit, "mm");
    }
}
