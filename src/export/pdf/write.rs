//! Serialization for the PDF writer: the bookmark outline tree and the final
//! objects → xref → trailer byte assembly. Layout produces the objects and the
//! outline; this turns them into the file.

use super::font::{pdf_text_string, ObjMap};
use super::num;

/// A bookmark: a title, the page it jumps to, and any nested entries.
pub(super) struct OutlineItem {
    title: String,
    page: usize,
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
