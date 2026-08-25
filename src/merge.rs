//! Compose the final PDF from the body/header/footer page PDFs, reproducing the
//! geometry of frappe/utils/pdf_generator/pdf_merge.py (PDFTransformer.transform_pdf).
//!
//! pypdf grows each body page's MediaBox and translates its content, then overlays the
//! header/footer pages. We reproduce the identical geometry by drawing all three source
//! pages as Form XObjects onto a fresh page of the combined height — visually identical.

use std::collections::BTreeMap;

use anyhow::{Result, anyhow};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream};

struct Source {
    doc: Document,
    pages: Vec<ObjectId>,
    offset: u32,
}

impl Source {
    fn load(bytes: &[u8]) -> Result<Self> {
        let doc = Document::load_mem(bytes)?;
        let map: BTreeMap<u32, ObjectId> = doc.get_pages();
        let pages: Vec<ObjectId> = map.into_values().collect();
        Ok(Self { doc, pages, offset: 0 })
    }

    fn page_height(&self, idx: usize) -> f64 {
        let mb = self.mediabox(idx);
        mb[3] - mb[1]
    }

    fn page_width(&self, idx: usize) -> f64 {
        let mb = self.mediabox(idx);
        mb[2] - mb[0]
    }

    fn mediabox(&self, idx: usize) -> [f64; 4] {
        page_mediabox(&self.doc, self.pages[idx]).unwrap_or([0.0, 0.0, 612.0, 792.0])
    }
}

pub struct MergeInput {
    pub body: Vec<u8>,
    pub header: Option<Vec<u8>>,
    pub footer: Option<Vec<u8>>,
    pub is_header_dynamic: bool,
    pub is_footer_dynamic: bool,
    pub is_print_designer: bool,
}

pub fn transform_pdf(input: MergeInput) -> Result<Vec<u8>> {
    let mut body = Source::load(&input.body)?;
    let mut header = input.header.as_ref().map(|b| Source::load(b)).transpose()?;
    let mut footer = input.footer.as_ref().map(|b| Source::load(b)).transpose()?;

    if header.is_none() && footer.is_none() {
        return Ok(input.body);
    }

    let n = body.pages.len();
    let body_h = body.page_height(0);
    let width = body.page_width(0);
    let head_h = header.as_ref().map(|h| h.page_height(0)).unwrap_or(0.0);
    let foot_h = footer.as_ref().map(|f| f.page_height(0)).unwrap_or(0.0);

    let header_present = header.is_some();
    let _footer_present = footer.is_some();

    // Geometry per pdf_merge.py.
    let header_body_top = if header_present { head_h + body_h + foot_h } else { 0.0 };
    let out_height = if header_present { header_body_top } else { body_h };
    let ty_body = if header_present { foot_h } else { 0.0 };
    let ty_header = body_h + foot_h; // header_transform
    let ty_footer = 0.0;

    if std::env::var("PINTO_DEBUG").is_ok() {
        eprintln!(
            "merge: n={n} body_h={body_h} head_h={head_h} foot_h={foot_h} out_h={out_height} ty_body={ty_body} ty_header={ty_header}"
        );
        if let Some(h) = header.as_ref() {
            eprintln!("header pages={} heights={:?}", h.pages.len(), (0..h.pages.len()).map(|i| h.page_height(i)).collect::<Vec<_>>());
        }
        if let Some(f) = footer.as_ref() {
            eprintln!("footer pages={} heights={:?}", f.pages.len(), (0..f.pages.len()).map(|i| f.page_height(i)).collect::<Vec<_>>());
        }
    }

    let mut out = Document::with_version("1.7");

    // Copy each source's object graph into `out` once; remember the id offset.
    body.offset = copy_all(&mut out, &body.doc);
    if let Some(h) = header.as_mut() {
        h.offset = copy_all(&mut out, &h.doc);
    }
    if let Some(f) = footer.as_mut() {
        f.offset = copy_all(&mut out, &f.doc);
    }

    let pages_id = out.new_object_id();
    let mut kids: Vec<Object> = Vec::with_capacity(n);

    for i in 0..n {
        // Keep the body page as the base (like pypdf) so its content is never clipped by
        // an XObject BBox; only the header/footer are overlaid as Form XObjects.
        let body_page_orig = body.pages[i];
        let body_content = body.doc.get_page_content(body_page_orig);

        let mut content = Vec::new();
        content.extend_from_slice(format!("q 1 0 0 1 0 {ty_body:.6} cm\n").as_bytes());
        content.extend_from_slice(&body_content);
        content.extend_from_slice(b"\nQ\n");

        let mut overlays: Vec<(&str, ObjectId)> = Vec::new();
        if let Some(h) = header.as_ref() {
            let idx = header_index(i, n, input.is_header_dynamic, input.is_print_designer, h.pages.len());
            // pypdf grows the header page to the full combined height and translates its
            // content inside; the overlay is then drawn at identity. Match that exactly.
            let bbox = [0.0, 0.0, width, out_height];
            let xh = make_overlay(&mut out, h, idx, bbox, ty_header)?;
            content.extend_from_slice(b"q /PintoHdr Do Q\n");
            overlays.push(("PintoHdr", xh));
        }
        if let Some(f) = footer.as_ref() {
            let idx = footer_index(i, n, input.is_footer_dynamic, input.is_print_designer, f.pages.len());
            // The footer is merged as-is (BBox = footer mediabox), drawn at identity.
            let bbox = f.mediabox(idx);
            let xf = make_overlay(&mut out, f, idx, bbox, ty_footer)?;
            content.extend_from_slice(b"q /PintoFtr Do Q\n");
            overlays.push(("PintoFtr", xf));
        }

        let content_id = out.add_object(Stream::new(Dictionary::new(), content));

        // Clone the body page's own resources and add the overlay XObjects, so body fonts
        // and images keep working while header/footer names don't collide.
        let body_page_copied = (body_page_orig.0 + body.offset, body_page_orig.1);
        let mut resources = resolve_resources(&out, body_page_copied);
        let mut xobjects = match resources.get(b"XObject") {
            Ok(Object::Dictionary(d)) => d.clone(),
            Ok(Object::Reference(r)) => out
                .get_object(*r)
                .ok()
                .and_then(|o| o.as_dict().ok())
                .cloned()
                .unwrap_or_else(Dictionary::new),
            _ => Dictionary::new(),
        };
        for (name, id) in &overlays {
            xobjects.set(*name, Object::Reference(*id));
        }
        resources.set("XObject", Object::Dictionary(xobjects));

        if let Some(Object::Dictionary(page)) = out.objects.get_mut(&body_page_copied) {
            page.set("Type", Object::Name(b"Page".to_vec()));
            page.set("Parent", Object::Reference(pages_id));
            page.set(
                "MediaBox",
                Object::Array(vec![
                    Object::Real(0.0),
                    Object::Real(0.0),
                    Object::Real(width as f32),
                    Object::Real(out_height as f32),
                ]),
            );
            page.set("Resources", Object::Dictionary(resources));
            page.set("Contents", Object::Reference(content_id));
            page.remove(b"Annots");
        }
        kids.push(Object::Reference(body_page_copied));
    }

    let count = kids.len() as i64;
    let mut pages = Dictionary::new();
    pages.set("Type", Object::Name(b"Pages".to_vec()));
    pages.set("Kids", Object::Array(kids));
    pages.set("Count", Object::Integer(count));
    out.objects.insert(pages_id, Object::Dictionary(pages));

    let mut catalog = Dictionary::new();
    catalog.set("Type", Object::Name(b"Catalog".to_vec()));
    catalog.set("Pages", Object::Reference(pages_id));
    let catalog_id = out.add_object(catalog);
    out.trailer.set("Root", Object::Reference(catalog_id));

    out.compress();
    let mut buf = Vec::new();
    out.save_to(&mut buf)?;
    Ok(buf)
}

fn header_index(i: usize, n: usize, dynamic: bool, pd: bool, count: usize) -> usize {
    if dynamic {
        i.min(count.saturating_sub(1))
    } else if pd {
        variant_index(i, n, count)
    } else {
        0
    }
}

fn footer_index(i: usize, n: usize, dynamic: bool, pd: bool, count: usize) -> usize {
    if dynamic && count > 1 {
        i.min(count - 1)
    } else if pd {
        variant_index(i, n, count)
    } else {
        0
    }
}

/// print_designer first/last/even/odd scheme: pages [0]=first, [3]=last, [2]=even, [1]=odd.
fn variant_index(i: usize, n: usize, count: usize) -> usize {
    let idx = if i == 0 {
        0
    } else if i == n.saturating_sub(1) {
        3
    } else if i.is_multiple_of(2) {
        2
    } else {
        1
    };
    idx.min(count.saturating_sub(1))
}

/// Copy every object of `src` into `out`, shifting object ids by an offset; return the offset.
fn copy_all(out: &mut Document, src: &Document) -> u32 {
    let offset = out.max_id;
    for (id, obj) in &src.objects {
        out.objects.insert((id.0 + offset, id.1), shift_object(obj, offset));
    }
    out.max_id += src.max_id;
    offset
}

fn shift_object(obj: &Object, offset: u32) -> Object {
    match obj {
        Object::Reference((n, g)) => Object::Reference((n + offset, *g)),
        Object::Array(items) => Object::Array(items.iter().map(|o| shift_object(o, offset)).collect()),
        Object::Dictionary(d) => Object::Dictionary(shift_dict(d, offset)),
        Object::Stream(s) => {
            let mut stream = Stream::new(shift_dict(&s.dict, offset), s.content.clone());
            stream.allows_compression = s.allows_compression;
            Object::Stream(stream)
        }
        other => other.clone(),
    }
}

fn shift_dict(dict: &Dictionary, offset: u32) -> Dictionary {
    let mut out = Dictionary::new();
    for (k, v) in dict.iter() {
        out.set(k.clone(), shift_object(v, offset));
    }
    out
}

/// Build a Form XObject from source page `idx` (already copied into `out`).
/// `bbox` is the clip box in the XObject's own space; `internal_ty` translates the page
/// content upward inside the XObject (reproducing pypdf's pre-merge page transform).
fn make_overlay(
    out: &mut Document,
    src: &Source,
    idx: usize,
    bbox: [f64; 4],
    internal_ty: f64,
) -> Result<ObjectId> {
    let page_id = src.pages[idx];
    let page_content = src.doc.get_page_content(page_id);
    let content = if internal_ty != 0.0 {
        let mut c = format!("q 1 0 0 1 0 {internal_ty:.6} cm\n").into_bytes();
        c.extend_from_slice(&page_content);
        c.extend_from_slice(b"\nQ\n");
        c
    } else {
        page_content
    };

    let shifted_page_id = (page_id.0 + src.offset, page_id.1);
    let resources = page_resources(out, shifted_page_id, src.offset)
        .ok_or_else(|| anyhow!("page has no resolvable /Resources"))?;

    let mut dict = Dictionary::new();
    dict.set("Type", Object::Name(b"XObject".to_vec()));
    dict.set("Subtype", Object::Name(b"Form".to_vec()));
    dict.set("FormType", Object::Integer(1));
    dict.set(
        "BBox",
        Object::Array(bbox.iter().map(|v| Object::Real(*v as f32)).collect()),
    );
    dict.set(
        "Matrix",
        Object::Array(vec![
            Object::Real(1.0),
            Object::Real(0.0),
            Object::Real(0.0),
            Object::Real(1.0),
            Object::Real(0.0),
            Object::Real(0.0),
        ]),
    );
    dict.set("Resources", resources);
    Ok(out.add_object(Stream::new(dict, content)))
}

/// Resolve a copied page's /Resources to an owned Dictionary (following /Parent inheritance).
fn resolve_resources(out: &Document, page_id: ObjectId) -> Dictionary {
    let mut current = page_id;
    for _ in 0..8 {
        let Some(obj) = out.objects.get(&current) else { break };
        let Ok(dict) = obj.as_dict() else { break };
        match dict.get(b"Resources") {
            Ok(Object::Dictionary(d)) => return d.clone(),
            Ok(Object::Reference(r)) => {
                if let Some(d) = out.objects.get(r).and_then(|o| o.as_dict().ok()) {
                    return d.clone();
                }
                break;
            }
            _ => {}
        }
        match dict.get(b"Parent").ok().and_then(|o| o.as_reference().ok()) {
            Some(p) => current = p,
            None => break,
        }
    }
    Dictionary::new()
}

/// Return the /Resources value (Reference or inline Dictionary) of a copied page.
fn page_resources(out: &Document, page_id: ObjectId, offset: u32) -> Option<Object> {
    let obj = out.objects.get(&page_id)?;
    let dict = obj.as_dict().ok()?;
    match dict.get(b"Resources").ok()? {
        Object::Reference(r) => Some(Object::Reference(*r)),
        Object::Dictionary(d) => Some(Object::Dictionary(d.clone())),
        _ => {
            // Resources may be inherited from the Pages parent.
            let parent = dict.get(b"Parent").ok()?.as_reference().ok()?;
            let _ = offset;
            let parent_dict = out.objects.get(&parent)?.as_dict().ok()?;
            match parent_dict.get(b"Resources").ok()? {
                Object::Reference(r) => Some(Object::Reference(*r)),
                Object::Dictionary(d) => Some(Object::Dictionary(d.clone())),
                _ => None,
            }
        }
    }
}

fn page_mediabox(doc: &Document, page_id: ObjectId) -> Option<[f64; 4]> {
    let mut current = page_id;
    for _ in 0..8 {
        let dict = doc.get_object(current).ok()?.as_dict().ok()?;
        if let Ok(mb) = dict.get(b"MediaBox") {
            let arr = resolve(doc, mb).as_array().ok()?.clone();
            if arr.len() == 4 {
                return Some([
                    num(doc, &arr[0]),
                    num(doc, &arr[1]),
                    num(doc, &arr[2]),
                    num(doc, &arr[3]),
                ]);
            }
        }
        current = dict.get(b"Parent").ok()?.as_reference().ok()?;
    }
    None
}

fn resolve<'a>(doc: &'a Document, obj: &'a Object) -> &'a Object {
    match obj {
        Object::Reference(id) => doc.get_object(*id).unwrap_or(obj),
        _ => obj,
    }
}

fn num(doc: &Document, obj: &Object) -> f64 {
    match resolve(doc, obj) {
        Object::Integer(i) => *i as f64,
        Object::Real(r) => *r as f64,
        _ => 0.0,
    }
}

/// Number of pages in a PDF byte buffer.
pub fn page_count(bytes: &[u8]) -> Result<usize> {
    Ok(Document::load_mem(bytes)?.get_pages().len())
}
