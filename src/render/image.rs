//! Image decoding for layout (intrinsic size) and paint (rgba pixels).

use std::sync::Mutex;

use base64::Engine as _;

pub struct Decoded {
    pub rgba: Vec<u8>,
    pub w: u32,
    pub h: u32,
}

/// Resolve an <img src> to encoded bytes: data: URIs, and local file paths.
/// Host-relative URLs (http://host/...) are mapped by the caller via `set_resolver`.
fn fetch_bytes(src: &str) -> Option<Vec<u8>> {
    if let Some(rest) = src.strip_prefix("data:") {
        let comma = rest.find(',')?;
        let meta = &rest[..comma];
        let payload = &rest[comma + 1..];
        return if meta.contains(";base64") {
            base64::engine::general_purpose::STANDARD.decode(payload.trim()).ok()
        } else {
            Some(percent_encoding::percent_decode_str(payload).collect())
        };
    }
    if let Some(path) = resolve_url(src) {
        return std::fs::read(path).ok();
    }
    if src.starts_with('/') || src.starts_with("./") || src.starts_with("../") {
        return std::fs::read(src).ok();
    }
    None
}

pub fn load(src: &str) -> Option<Decoded> {
    let bytes = fetch_bytes(src)?;
    let img = image::load_from_memory(&bytes).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Some(Decoded { rgba: rgba.into_raw(), w, h })
}

/// Intrinsic size in CSS px (image pixels map 1:1 to CSS px at 96dpi).
pub fn intrinsic_size(src: &str) -> Option<(f32, f32)> {
    let d = load(src)?;
    Some((d.w as f32, d.h as f32))
}

// ----- host-relative URL resolution (files/, assets/) -----

struct Resolver {
    host_url: String,
    site_public_path: Option<String>,
    bench_sites_path: Option<String>,
}

static RESOLVER: Mutex<Option<Resolver>> = Mutex::new(None);

pub fn set_resolver(host_url: String, site_public_path: Option<String>, bench_sites_path: Option<String>) {
    *RESOLVER.lock().unwrap() = Some(Resolver { host_url, site_public_path, bench_sites_path });
}

pub fn resolve_url(src: &str) -> Option<std::path::PathBuf> {
    let guard = RESOLVER.lock().unwrap();
    let r = guard.as_ref()?;
    let rel = src.strip_prefix(&r.host_url).or_else(|| src.strip_prefix('/'))?;
    let clean = percent_encoding::percent_decode_str(rel.split("?v").next().unwrap_or(rel)).decode_utf8_lossy().into_owned();
    if let Some(rest) = clean.strip_prefix("assets/") {
        let sites = r.bench_sites_path.as_ref()?;
        return Some(std::path::Path::new(sites).join("assets").join(rest));
    }
    let public = r.site_public_path.as_ref()?;
    Some(std::path::Path::new(public).join(&clean))
}
