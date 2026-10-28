//! Vector math for ranking (#831): L2 normalization and cosine similarity, the two
//! pure primitives the index's ranking is built on. Kept tiny and dependency-free
//! so the hermetic tests exercise ranking with injected vectors (#531).

/// L2-normalize a vector in place so a dot product is cosine similarity. A
/// zero-norm vector (all zeros) is left as-is (its similarity to anything is 0).
pub(super) fn normalize(v: &mut [f32]) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > f32::EPSILON {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

/// Cosine similarity of two vectors assumed L2-normalized (a plain dot product).
/// Mismatched lengths score 0 (a defensive guard; the index keeps widths uniform).
pub(super) fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}
