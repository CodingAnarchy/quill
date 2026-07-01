//! Serialization for the PDF writer: the bookmark outline tree and the final
//! objects → xref → trailer byte assembly. Layout produces the objects and the
//! outline; this turns them into the file.

use std::io::Write;

use flate2::write::ZlibEncoder;
use flate2::Compression;

use super::font::{pdf_text_string, GidMap, ObjMap};
use super::num;

/// Build a FlateDecode-compressed stream object body: `<< /Filter /FlateDecode
/// /Length N{extra} >>\nstream\n<zlib bytes>\nendstream`. `data` is the raw
/// (uncompressed) stream payload; `extra` is any additional dict entries (e.g.
/// ` /Length1 <n>` for a font program, spelling out the *uncompressed* length).
/// Content streams and embedded font programs (#662) ship compressed to shrink
/// the exported PDF; the bytes round-trip to `data` via zlib inflate.
pub(super) fn flate_stream(data: &[u8], extra: &str) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(data)
        .expect("zlib encode to an in-memory buffer never fails");
    let compressed = encoder
        .finish()
        .expect("zlib encode to an in-memory buffer never fails");
    let mut v = format!(
        "<< /Filter /FlateDecode /Length {}{extra} >>\nstream\n",
        compressed.len()
    )
    .into_bytes();
    v.extend_from_slice(&compressed);
    v.extend_from_slice(b"\nendstream");
    v
}

/// Remap the glyph ids in a content stream from the original fonts to their embedded
/// subsets. The writer emits one op per line: a `/Fn … Tf` font switch, then glyph
/// runs as `<hhhh…> Tj` (4 hex digits per id). This tracks the current font and
/// rewrites each id through that font's map. Every drawn glyph was recorded before
/// subsetting, so every id has a mapping; a miss would fall back to `.notdef` (0)
/// rather than a stale id that points at the wrong glyph in the subset.
pub(super) fn remap_glyph_ids(content: &str, maps: &[(&str, &GidMap)]) -> String {
    let mut out = String::with_capacity(content.len());
    let mut current: Option<&GidMap> = None;
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix('/') {
            if line.ends_with(" Tf") {
                let name = rest.split(' ').next().unwrap_or("");
                current = maps.iter().find(|(n, _)| *n == name).map(|(_, m)| *m);
            }
            out.push_str(line);
        } else if let (Some(hex), Some(map)) = (
            line.strip_prefix('<').and_then(|l| l.strip_suffix("> Tj")),
            current,
        ) {
            out.push('<');
            for chunk in hex.as_bytes().chunks(4) {
                let old =
                    u16::from_str_radix(std::str::from_utf8(chunk).unwrap_or(""), 16).unwrap_or(0);
                out.push_str(&format!("{:04X}", map.get(&old).copied().unwrap_or(0)));
            }
            out.push_str("> Tj");
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

/// A bookmark: a title, the page it jumps to, and any nested entries.
pub(super) struct OutlineItem {
    pub(super) title: String,
    pub(super) page: usize,
    pub(super) children: Vec<OutlineItem>,
}

impl OutlineItem {
    /// A bookmark to `page`; nest entries by pushing onto `children` afterward.
    pub(super) fn new(title: String, page: usize) -> Self {
        OutlineItem {
            title,
            page,
            children: Vec::new(),
        }
    }
}

/// Flatten an outline subtree into PDF objects under `parent_id`, returning the
/// (first, last) child object ids. `count` accumulates the total item count.
pub(super) fn flatten_outline(
    items: &[OutlineItem],
    parent_id: u32,
    next: &mut u32,
    page_obj: &[u32],
    page_h: f32,
    objects: &mut ObjMap,
    count: &mut usize,
) -> (Option<u32>, Option<u32>) {
    if items.is_empty() {
        return (None, None);
    }
    let ids: Vec<u32> = items
        .iter()
        .map(|_| {
            let id = *next;
            *next += 1;
            id
        })
        .collect();
    *count += items.len();
    for (k, item) in items.iter().enumerate() {
        let id = ids[k];
        let (first, last) =
            flatten_outline(&item.children, id, next, page_obj, page_h, objects, count);
        // Jump to the top of the destination page (its height varies by trim).
        let dest = format!(
            "[{} 0 R /XYZ null {} null]",
            page_obj[item.page],
            num(page_h)
        );
        let mut body = format!(
            "<< /Title {} /Parent {parent_id} 0 R /Dest {dest}",
            pdf_text_string(&item.title)
        );
        if k > 0 {
            body.push_str(&format!(" /Prev {} 0 R", ids[k - 1]));
        }
        if k + 1 < ids.len() {
            body.push_str(&format!(" /Next {} 0 R", ids[k + 1]));
        }
        if let (Some(f), Some(l)) = (first, last) {
            body.push_str(&format!(
                " /First {f} 0 R /Last {l} 0 R /Count {}",
                item.children.len()
            ));
        }
        body.push_str(" >>");
        objects.insert(id, body.into_bytes());
    }
    (ids.first().copied(), ids.last().copied())
}

/// Write the objects (in id order), the xref table, and the trailer.
pub(super) fn serialize(objects: &ObjMap) -> Vec<u8> {
    let max_id = *objects.keys().max().expect("at least the catalog");
    let mut out: Vec<u8> = b"%PDF-1.7\n".to_vec();
    let mut offsets = vec![0usize; (max_id + 1) as usize];
    for id in 1..=max_id {
        let body = objects.get(&id).expect("contiguous object ids");
        offsets[id as usize] = out.len();
        out.extend_from_slice(format!("{id} 0 obj\n").as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n", max_id + 1).as_bytes());
    out.extend_from_slice(b"0000000000 65535 f \n");
    for id in 1..=max_id {
        out.extend_from_slice(format!("{:010} {:05} n \n", offsets[id as usize], 0).as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            max_id + 1
        )
        .as_bytes(),
    );
    out
}
