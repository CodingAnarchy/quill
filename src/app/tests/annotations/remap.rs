//! The batched annotation remap (#893): re-anchoring a whole annotation set
//! through an edit burst in one pass must land on exactly the positions the old
//! per-edit walk produced, must leave threads and resolved flags untouched, and
//! must scale with the annotation count rather than annotations × edits (the
//! `#[ignore]`d perf guard, run via `cargo perf`).

use super::*;
use crate::annotation::Remapper;
use crate::buffer::Cursor;
use crate::testutil::perf_budget_ms;
use std::time::Instant;

#[test]
fn a_reply_thread_and_resolved_flag_survive_an_edit_burst() {
    // Threads ride along with the anchor through the idle remap: the replies and the
    // resolved flag aren't disturbed when the note re-anchors (#889 + #893).
    let (mut app, _dir) = two_chapter_app("anno-thread-remap");
    app.create_annotation(6, 13, "note on chapter".into()); // "chapter"
    app.open_annotation_review();
    app.on_key(key(KeyCode::Char('a')));
    type_str(&mut app, "keep me");
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Char('r'))); // resolve
    app.on_key(key(KeyCode::Esc)); // back to editing

    app.buffer.cursor = Cursor { line: 0, col: 0 };
    type_str(&mut app, "XYZ");
    app.on_idle();

    let a = &app.annotations[0];
    assert_eq!((a.start, a.end), (9, 16), "re-anchored past the insert");
    assert_eq!(a.replies.len(), 1, "reply preserved through the remap");
    assert_eq!(a.replies[0].text, "keep me");
    assert!(a.resolved, "resolved flag preserved through the remap");
}

/// The old per-edit remap: walk every annotation once per edit, dropping any whose
/// text is consumed. The batched [`Remapper`] must produce identical positions.
fn remap_naive(anns: &mut Vec<(usize, usize)>, edits: &[(usize, usize, usize)]) {
    for &e in edits {
        anns.retain_mut(|r| match crate::annotation::remap(*r, e) {
            Some((s, en)) => {
                *r = (s, en);
                true
            }
            None => false,
        });
    }
}

#[test]
fn batched_remap_matches_the_per_edit_walk_on_many_annotations_and_edits() {
    // Many annotations spread across a buffer, many edits: the one-pass Remapper
    // must land every note exactly where the old walk-per-edit did (#893).
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    let mut rng = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for _ in 0..40 {
        let mut anns: Vec<(usize, usize)> = (0..200)
            .map(|i| {
                let s = i * 3;
                (s, s + 2)
            })
            .collect();
        let edits: Vec<(usize, usize, usize)> = (0..150)
            .map(|_| {
                let start = (rng() % 600) as usize;
                let old_len = (rng() % 4) as usize;
                let new_len = (rng() % 4) as usize;
                (start, start + old_len, start + new_len)
            })
            .collect();

        let mut expected = anns.clone();
        remap_naive(&mut expected, &edits);

        let mapper = Remapper::new(&edits);
        anns.retain_mut(|r| match mapper.remap_range(r.0, r.1) {
            Some((s, e)) => {
                *r = (s, e);
                true
            }
            None => false,
        });

        assert_eq!(anns, expected, "batched remap diverged for edits {edits:?}");
    }
}

#[test]
#[ignore = "release-only perf guard; run via cargo perf"]
fn batched_remap_is_not_quadratic_in_annotations_times_edits() {
    // The defect (#893) was the per-annotation cost: the old code walked *every*
    // annotation once per edit, so re-anchoring N notes through E edits cost N·E.
    // The Remapper builds the composed edit map once, then answers each annotation
    // with a binary search — so the annotation pass is linear in N, independent of
    // the edit count. Measure a fixed edit burst against a growing annotation set
    // and confirm the remap pass scales with N, not N·E.
    let remap_pass = |n_anns: usize, edits: &[(usize, usize, usize)]| -> std::time::Duration {
        let span = n_anns * 6 + 1;
        let mut anns: Vec<(usize, usize)> = (0..n_anns).map(|i| (i * 6, i * 6 + 3)).collect();
        let mapper = Remapper::new(edits);
        // Time only the per-annotation remap — the part that used to be O(edits ×
        // annotations). Building the map is done once, up front.
        let t = Instant::now();
        anns.retain_mut(|r| match mapper.remap_range(r.0.min(span), r.1.min(span)) {
            Some((s, e)) => {
                *r = (s, e);
                true
            }
            None => false,
        });
        t.elapsed()
    };

    // A fixed 2,000-edit burst spread across the buffer.
    let edits: Vec<(usize, usize, usize)> = (0..2000)
        .map(|i| {
            let start = (i * 13) % 60_001;
            (start, start + 1, start + 2)
        })
        .collect();

    let _ = remap_pass(1000, &edits); // warm
    let small = remap_pass(5_000, &edits);
    let big = remap_pass(20_000, &edits);
    println!("remap pass: 5k anns = {small:?}, 20k anns = {big:?} (edits fixed at 2000)");

    // 4× the annotations must not cost anywhere near 4× · (edit growth) — with the
    // old N·E walk, holding E fixed, 4× N is 4× work; the guard is that 20k stays a
    // small multiple of 5k (linear), and both are well within a frame. A quadratic
    // regression (re-introducing the per-edit walk) would make `big` blow past this.
    assert!(
        big.as_millis() < perf_budget_ms(16),
        "remap pass took {big:?}"
    );
    assert!(
        big < small * 8,
        "remap should scale ~linearly with annotations: 5k={small:?}, 20k={big:?}"
    );
}
