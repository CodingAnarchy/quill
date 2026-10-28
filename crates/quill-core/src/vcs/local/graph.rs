//! The commit-graph lane layout (#964): turning a log's commit/parent topology into
//! the Unicode branch/merge graph column each history row renders behind its text.
//! Pure string layout over ids — no repository access — so it's unit-testable in
//! isolation and [`log`](super::history::log) just stamps each row's column on.

/// Lay out a Unicode commit graph, one column string per commit (#964). `revs[i]`
/// is the i-th commit's full id and `parents[i]` its parent ids, in the log's
/// display order (a child always before its parents). Returns a graph prefix for
/// each row: the active branch lanes as `│`, this commit's own node as `●` in its
/// lane. Every string is padded to a common width so the summaries that follow
/// them line up in a column.
///
/// Standard single-pass lane assignment: a list of "active lanes", each holding the
/// id of the next commit expected to occupy it. For each commit we find (or open)
/// its lane, mark its node, then hand that lane to the commit's first parent and
/// open fresh lanes for any extra (merge) parents. A lane waiting on a still-unseen
/// commit carries a straight `│` down through the row.
pub(super) fn graph_rows(revs: &[String], parents: &[Vec<String>]) -> Vec<String> {
    // Each lane holds the id of the commit that will next land in it, or `None` for
    // a free slot. Newest-first order means we always meet a commit before its
    // parents, so a lane is opened when a child names the parent and closed (or
    // reused) when that parent's own row is reached.
    let mut lanes: Vec<Option<String>> = Vec::new();
    let mut out = Vec::with_capacity(revs.len());

    for (rev, ps) in revs.iter().zip(parents.iter()) {
        // The lane this commit sits in: the first already expecting it, else a new
        // one appended on the right.
        let col = lanes
            .iter()
            .position(|l| l.as_deref() == Some(rev.as_str()))
            .unwrap_or_else(|| {
                lanes.push(None);
                lanes.len() - 1
            });

        // Draw the row: every active lane is a `│`, except this commit's, a `●`.
        let mut row = String::new();
        for (i, lane) in lanes.iter().enumerate() {
            if i == col {
                row.push('●');
            } else if lane.is_some() {
                row.push('│');
            } else {
                row.push(' ');
            }
            row.push(' ');
        }
        out.push(row);

        // Advance the lanes past this commit. Clear every lane that was waiting on
        // *this* commit (a commit can head more than one lane where branches merge
        // back together), then route the parents.
        for lane in &mut lanes {
            if lane.as_deref() == Some(rev.as_str()) {
                *lane = None;
            }
        }
        // The first parent continues this commit's lane.
        if let Some(p) = ps.first() {
            place_parent(&mut lanes, col, p);
        }
        // Extra merge parents each take a lane of their own (or reuse a free one).
        for p in ps.iter().skip(1) {
            let c = lanes.iter().position(Option::is_none).unwrap_or_else(|| {
                lanes.push(None);
                lanes.len() - 1
            });
            place_parent(&mut lanes, c, p);
        }
        // Trim trailing empty lanes so the graph keeps a clean right edge.
        while matches!(lanes.last(), Some(None)) {
            lanes.pop();
        }
    }

    // Pad every row to the widest so the summaries after them align in a column.
    // Count characters, not bytes: `●`/`│` are one display cell but several bytes,
    // and each lane is one glyph + one space, so a char count is the cell width.
    let width = out.iter().map(|r| r.chars().count()).max().unwrap_or(0);
    for row in &mut out {
        for _ in row.chars().count()..width {
            row.push(' ');
        }
    }
    out
}

/// Assign parent id `p` to lane `col`, unless another lane already expects it — a
/// parent shared by two branches keeps its single lane, so the two draw as lines
/// converging rather than a duplicate column.
fn place_parent(lanes: &mut [Option<String>], col: usize, p: &str) {
    if lanes.iter().any(|l| l.as_deref() == Some(p)) {
        return; // already tracked elsewhere; leave `col` free for reuse/trimming
    }
    if let Some(slot) = lanes.get_mut(col) {
        *slot = Some(p.to_string());
    }
}

#[cfg(test)]
mod graph_tests {
    use super::graph_rows;

    /// The `●` sits in the column named by `col`; every earlier active lane draws a
    /// `│`. A helper so the expectations read as "node in lane N".
    fn node_col(row: &str) -> usize {
        row.chars().step_by(2).position(|c| c == '●').unwrap_or(999)
    }

    #[test]
    fn a_linear_history_is_a_single_lane() {
        // c -> b -> a (newest first). One straight lane: every node in column 0.
        let revs = vec!["c".to_string(), "b".to_string(), "a".to_string()];
        let parents = vec![
            vec!["b".to_string()],
            vec!["a".to_string()],
            vec![], // root
        ];
        let rows = graph_rows(&revs, &parents);
        assert_eq!(rows.len(), 3);
        for r in &rows {
            assert_eq!(node_col(r), 0, "linear history stays in lane 0: {r:?}");
            assert!(r.contains('●'));
        }
    }

    #[test]
    fn a_branch_and_merge_opens_and_closes_a_second_lane() {
        // A merge `m` with two parents `x` (mainline) and `y` (a side branch),
        // which both descend from base `a`:
        //   m ─┬─ x ─── a
        //      └─ y ────┘
        // Newest first: m, x, y, a. The merge row must show a node plus a second
        // lane, and while the branch is live (rows for x and y) two lanes are drawn.
        let revs = vec![
            "m".to_string(),
            "x".to_string(),
            "y".to_string(),
            "a".to_string(),
        ];
        let parents = vec![
            vec!["x".to_string(), "y".to_string()], // merge: two parents
            vec!["a".to_string()],
            vec!["a".to_string()],
            vec![],
        ];
        let rows = graph_rows(&revs, &parents);
        assert_eq!(rows.len(), 4);
        // The merge row (row 0) is itself a single node; its extra-parent lane opens
        // on the rows *below* it, git-graph style.
        assert!(rows[0].contains('●'), "the merge is a node: {:?}", rows[0]);
        // Once the branch has fanned out, two lanes are drawn while both the
        // mainline (x) and side (y) commits are still unresolved.
        let live_lanes = |r: &str| r.matches('│').count() + r.matches('●').count();
        assert!(
            live_lanes(&rows[1]) >= 2 || live_lanes(&rows[2]) >= 2,
            "a second lane is drawn while the branch is live: {:?}",
            &rows[1..3]
        );
        // The shared base collapses back to a single lane (node in column 0).
        assert_eq!(
            node_col(&rows[3]),
            0,
            "the base rejoins lane 0: {:?}",
            rows[3]
        );
        // Every row is padded to the same display width for column alignment.
        let w = rows[0].chars().count();
        assert!(rows.iter().all(|r| r.chars().count() == w), "aligned width");
    }

    #[test]
    fn an_empty_log_yields_no_rows() {
        assert!(graph_rows(&[], &[]).is_empty());
    }
}
