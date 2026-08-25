//! Minimal CSS scanner for `.print-format` page options.
//! Mirrors frappe/utils/pdf.py:get_print_format_styles — only top-level style rules
//! whose comma-split selector list contains exactly `.print-format`; @-rules are skipped
//! (never recursed into), and shorthands are not expanded.

/// Return (property-name, value) pairs declared on `.print-format` rules, in order.
pub fn print_format_declarations(stylesheet: &str) -> Vec<(String, String)> {
    let css = strip_comments(stylesheet);
    let bytes = css.as_bytes();
    let n = bytes.len();
    let mut out = Vec::new();
    let mut i = 0;
    let mut start = 0;

    while i < n {
        match bytes[i] {
            b';' => {
                start = i + 1;
                i += 1;
            }
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
                let block_end = j.saturating_sub(1).min(n);
                let block = &css[block_start..block_end];
                if !prelude.starts_with('@') && selector_matches(&prelude) {
                    parse_declarations(block, &mut out);
                }
                i = j;
                start = i;
            }
            _ => i += 1,
        }
    }
    out
}

fn selector_matches(prelude: &str) -> bool {
    prelude.split(',').any(|s| s.trim() == ".print-format")
}

fn parse_declarations(block: &str, out: &mut Vec<(String, String)>) {
    for decl in block.split(';') {
        let decl = decl.trim();
        if decl.is_empty() {
            continue;
        }
        if let Some((name, value)) = decl.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            let mut value = value.trim();
            if let Some(stripped) = strip_important(value) {
                value = stripped;
            }
            if !name.is_empty() {
                out.push((name, normalize_zero(value.trim())));
            }
        }
    }
}

/// Normalize a zero-magnitude length to a bare "0", matching cssutils (e.g. "0mm" -> "0").
fn normalize_zero(value: &str) -> String {
    const UNITS: [&str; 8] = ["px", "mm", "cm", "in", "pt", "em", "rem", "%"];
    let mut magnitude = value;
    for unit in UNITS {
        if let Some(prefix) = value.strip_suffix(unit) {
            magnitude = prefix.trim_end();
            break;
        }
    }
    if magnitude.parse::<f64>().map(|n| n == 0.0).unwrap_or(false) {
        return "0".to_string();
    }
    value.to_string()
}

fn strip_important(value: &str) -> Option<&str> {
    let lower = value.to_ascii_lowercase();
    let pos = lower.rfind("!important")?;
    Some(value[..pos].trim_end())
}

pub fn strip_comments(css: &str) -> String {
    let bytes = css.as_bytes();
    let n = bytes.len();
    let mut out = String::with_capacity(n);
    let mut i = 0;
    while i < n {
        if i + 1 < n && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < n && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i += 2;
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // Mirrors frappe/tests/test_pdf.py::test_read_options_from_html (stylesheet text only).
    #[test]
    fn matches_frappe_read_options_from_html() {
        let css = r#"
            .print-format {
             margin-top: 0mm;
             margin-left: 10mm;
             margin-right: 0mm;
            }
            "#;
        let decls = print_format_declarations(css);
        let get = |k: &str| decls.iter().rev().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        assert_eq!(get("margin-top"), Some("0".into()));
        assert_eq!(get("margin-left"), Some("10mm".into()));
        assert_eq!(get("margin-right"), Some("0".into()));
    }

    #[test]
    fn ignores_descendant_selectors() {
        let css = r#"
            .print-format { margin-top: 0mm; margin-left: 10mm; }
            .print-format .more-info { margin-right: 15mm; }
            .print-format, .more-info { margin-bottom: 20mm; }
            "#;
        let decls = print_format_declarations(css);
        let get = |k: &str| decls.iter().rev().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        assert_eq!(get("margin-top"), Some("0".into()));
        assert_eq!(get("margin-left"), Some("10mm".into()));
        assert_eq!(get("margin-bottom"), Some("20mm".into()));
        // margin-right belongs to a descendant selector and must be dropped.
        assert_eq!(get("margin-right"), None);
    }
}
