//! On-device semantic retrieval for grounded-Ask (#831): a local text-embedding
//! index over the manuscript's chapters, exposed as a `search(query) → ranked
//! passages + jumpable citations` primitive.
//!
//! "Ask the manuscript" (#609) sends the whole active book as context and leans on
//! the model to cite; that caps Ask at one context window and only cites as well as
//! full-context recall allows. This module indexes chapters into fixed-width
//! embedding vectors, so a question can *retrieve* the most relevant passages
//! (ranked by cosine similarity) and feed only those — with their chapter
//! citations — to the Q&A request. That scales past a single book's window and
//! catches paraphrase/synonym matches lexical search misses. It **only ever reads**
//! prose to retrieve and cite it; it never writes, drafts, or edits — squarely
//! inside Quill's AI boundary.
//!
//! The design is split so the retrieval logic is **hermetically testable** without
//! a model (#531): the [`Embedder`] trait is the seam, chunking / cosine ranking /
//! index storage / [`Index::search`] are pure and exercised with an injected fake
//! embedder, and only [`LocalEmbedder`] (behind `#[cfg(feature = "local")]`) pulls
//! in candle. When no embedder is available the caller falls back to the whole-book
//! Ask path unchanged.
//!
//! The index is content-addressed like the bible's incremental state (#90): each
//! chapter's passages are keyed to its content digest ([`crate::ai::incremental`]),
//! so a refresh re-embeds only the chapters whose text changed and the index can't
//! drift from the manuscript.
//!
//! The module is split by concern (#968), each file well under the ~1,000-line
//! convention, with the public surface re-exported here so `crate::ai::embed::…`
//! callers are unaffected:
//! - [`chunk`]: the [`Breadth`] knob and [`chunk_chapter`] paragraph packing.
//! - [`types`]: the value types ([`Passage`], [`Hit`], [`EchoPair`], [`Chapter`])
//!   and the [`Embedder`] model seam.
//! - [`index`]: the [`Index`] — incremental refresh, ranking (search / find-similar
//!   / relevant-prior / echo pairs), and persistence.
//! - `math`: the internal L2-normalize / cosine primitives.
//! - `local` (feature `local`): the candle [`LocalEmbedder`].
//!
//! The retrieval core (chunking, ranking, index storage) is always compiled so the
//! hermetic unit tests exercise it in the normal suite (#531), but its only *non*-
//! test caller — the Ask retrieval step in [`crate::app::assist`] — is behind
//! `#[cfg(feature = "local")]`, since retrieval needs the candle embedder. So in a
//! default (no-`local`) non-test build every item is genuinely unreachable; the
//! module-level allow below scopes the dead-code silence to exactly that build,
//! rather than deleting logic the tests and the `local` build both need (#406).
#![cfg_attr(not(feature = "local"), allow(dead_code))]

mod chunk;
mod index;
mod math;
mod types;

// `chunk_chapter` and `Index` complete the module's public surface but have no
// in-crate consumer once split out: `Index` is driven by the assist retrieval step
// only under `local`, and `chunk_chapter` only by the hermetic tests — so these
// re-exports are `unused_imports`-silenced, the re-export analogue of the file's
// `dead_code` allow (#406), keeping both the default and `local` builds warning-free.
#[allow(unused_imports)]
pub use chunk::{chunk_chapter, Breadth};
#[allow(unused_imports)]
pub use index::Index;
pub use types::{Chapter, EchoPair, Embedder, Hit, Passage};

// The per-passage index record, private to the module but shared by `index` (which
// stores it) and the hermetic tests below (which build it with injected vectors).
use types::IndexedPassage;

#[cfg(feature = "local")]
pub use local::LocalEmbedder;

#[cfg(feature = "local")]
mod local;

#[cfg(test)]
mod tests {
    use super::chunk::PASSAGE_CHARS;
    use super::math::normalize;
    use super::*;
    use crate::ai::AiError;

    /// A deterministic fake embedder for hermetic tests (#531): maps each text to a
    /// fixed vector by a keyword → axis scheme, so ranking is predictable with no
    /// model download. Each configured keyword owns one axis; a text's vector counts
    /// its keyword hits per axis, so a query and a passage sharing a keyword rank
    /// high and unrelated ones rank low.
    struct FakeEmbedder {
        keywords: Vec<&'static str>,
    }

    impl Embedder for FakeEmbedder {
        fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, AiError> {
            Ok(texts
                .iter()
                .map(|t| {
                    let lower = t.to_lowercase();
                    self.keywords
                        .iter()
                        .map(|k| lower.matches(k).count() as f32)
                        .collect()
                })
                .collect())
        }
        fn model_tag(&self) -> String {
            "fake-v1".into()
        }
    }

    /// An embedder that counts how many texts it was asked to embed, so a test can
    /// prove a refresh re-embeds only the changed chapters (#90).
    struct Counting {
        calls: std::cell::Cell<usize>,
    }

    impl Embedder for Counting {
        fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, AiError> {
            self.calls.set(self.calls.get() + texts.len());
            Ok(texts.iter().map(|t| vec![t.len() as f32]).collect())
        }
        fn model_tag(&self) -> String {
            "count-v1".into()
        }
    }

    /// A second embedder with a *different* model tag, so a test can prove swapping
    /// the embedding model invalidates the index and rebuilds it.
    struct Other;

    impl Embedder for Other {
        fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, AiError> {
            Ok(texts.iter().map(|_| vec![1.0, 0.0]).collect())
        }
        fn model_tag(&self) -> String {
            "other-v1".into()
        }
    }

    fn chapter(title: &str, file: &str, body: &str) -> Chapter {
        Chapter {
            title: title.into(),
            file: file.into(),
            body: body.into(),
        }
    }

    #[test]
    fn chunk_splits_on_paragraphs_and_caps_length() {
        let body = "First paragraph.\n\nSecond paragraph.";
        let passages = chunk_chapter("One", "one.md", body, Breadth::Normal);
        // Both short paragraphs pack into a single passage.
        assert_eq!(passages.len(), 1);
        assert!(passages[0].text.contains("First") && passages[0].text.contains("Second"));
        assert_eq!(passages[0].title, "One");
        assert_eq!(passages[0].file, "one.md");

        // A body far over the cap yields multiple passages, none empty.
        let big = "word ".repeat(PASSAGE_CHARS); // ~5x the cap
        let paras = format!("{big}\n\n{big}\n\n{big}");
        let many = chunk_chapter("Big", "big.md", &paras, Breadth::Normal);
        assert!(many.len() >= 3, "oversized chapter splits: {}", many.len());
        assert!(many.iter().all(|p| !p.text.trim().is_empty()));
    }

    #[test]
    fn empty_chapter_yields_no_passages() {
        assert!(chunk_chapter("Empty", "e.md", "   \n\n  ", Breadth::Normal).is_empty());
    }

    #[test]
    fn breadth_controls_passage_granularity() {
        // Same body chunked at each breadth (#931): finer → more, smaller passages;
        // broader → fewer, larger ones; normal in between. A paragraph a bit under the
        // Fine cap, so several pack into one Normal/Broad passage but each fills a
        // Fine one.
        let para = "word ".repeat(Breadth::Fine.passage_chars() * 3 / 4 / 5);
        let body = format!("{para}\n\n{para}\n\n{para}\n\n{para}\n\n{para}\n\n{para}");

        let fine = chunk_chapter("C", "c.md", &body, Breadth::Fine);
        let normal = chunk_chapter("C", "c.md", &body, Breadth::Normal);
        let broad = chunk_chapter("C", "c.md", &body, Breadth::Broad);

        // Finer breadth splits into strictly more passages than broader breadth.
        assert!(
            fine.len() > normal.len(),
            "fine yields more passages than normal: {} vs {}",
            fine.len(),
            normal.len()
        );
        assert!(
            normal.len() > broad.len(),
            "normal yields more passages than broad: {} vs {}",
            normal.len(),
            broad.len()
        );
        // Every breadth still covers the whole body (no prose dropped) and each
        // passage honors its breadth's cap.
        for (b, passages) in [
            (Breadth::Fine, &fine),
            (Breadth::Normal, &normal),
            (Breadth::Broad, &broad),
        ] {
            assert!(!passages.is_empty());
            assert!(
                passages.iter().all(|p| p.text.len() <= b.passage_chars()),
                "passages fit the {b:?} cap"
            );
        }
    }

    #[test]
    fn breadth_scales_top_k_around_the_base() {
        // Broad retrieves fewer passages (they're larger), Fine more (smaller),
        // Normal the base unchanged — so the prose fed to the answer stays comparable.
        assert_eq!(
            Breadth::Normal.top_k(12),
            12,
            "normal is the base unchanged"
        );
        assert!(Breadth::Broad.top_k(12) < 12, "broad retrieves fewer");
        assert!(Breadth::Fine.top_k(12) > 12, "fine retrieves more");
        // Broad never drops to zero when the caller asked for any passages.
        assert_eq!(Breadth::Broad.top_k(1), 1, "broad keeps at least one");
        assert_eq!(Breadth::Broad.top_k(0), 0, "top_k(0) stays zero");
    }

    #[test]
    fn changing_breadth_rebuilds_the_index() {
        // A breadth change re-chunks the manuscript at the new granularity (#931),
        // exactly like an embedder-model swap — the stored vectors aren't comparable
        // across widths. One long chapter so Fine and Broad chunk it differently.
        let embedder = FakeEmbedder {
            keywords: vec!["a"],
        };
        let para = "alpha ".repeat(Breadth::Fine.passage_chars() / 6);
        let body = format!("{para}\n\n{para}\n\n{para}\n\n{para}");
        let chapters = vec![chapter("Long", "long.md", &body)];

        let mut index = Index::default();
        index.refresh(&chapters, Breadth::Broad, &embedder).unwrap();
        let broad_count = index.passage_count();

        // Re-refreshing at the same breadth over an unchanged manuscript is a no-op.
        let n = index.refresh(&chapters, Breadth::Broad, &embedder).unwrap();
        assert_eq!(
            n, 0,
            "same breadth + unchanged manuscript re-embeds nothing"
        );

        // A finer breadth rebuilds everything at the finer granularity.
        let n = index.refresh(&chapters, Breadth::Fine, &embedder).unwrap();
        assert_eq!(n, 1, "the breadth change re-embeds the chapter");
        assert!(
            index.passage_count() > broad_count,
            "finer breadth yields more passages: {} vs {broad_count}",
            index.passage_count()
        );
    }

    #[test]
    fn search_at_any_breadth_resolves_jumpable_citations() {
        // Retrieval at each breadth still ranks the right chapter first and keeps its
        // title/file, so the answer's citations stay jumpable exactly as today (#931).
        let embedder = FakeEmbedder {
            keywords: vec!["locket", "harbor"],
        };
        let chapters = vec![
            chapter("The Locket", "one.md", "She clutched the silver locket."),
            chapter("The Harbor", "two.md", "Ships crowded the busy harbor."),
        ];
        for breadth in [Breadth::Fine, Breadth::Normal, Breadth::Broad] {
            let mut index = Index::default();
            index.refresh(&chapters, breadth, &embedder).unwrap();
            let hits = index
                .search("where is the locket", breadth.top_k(4), &embedder)
                .unwrap();
            assert!(!hits.is_empty(), "{breadth:?} retrieves passages");
            assert_eq!(
                hits[0].passage.title, "The Locket",
                "{breadth:?} ranks the locket chapter first"
            );
            assert_eq!(
                hits[0].passage.file, "one.md",
                "{breadth:?} keeps the citation's file"
            );
        }
    }

    #[test]
    fn search_ranks_the_relevant_passage_first_and_cites_it() {
        let embedder = FakeEmbedder {
            keywords: vec!["locket", "harbor", "sword"],
        };
        let chapters = vec![
            chapter(
                "The Locket",
                "one.md",
                "She clutched the silver locket tightly.",
            ),
            chapter(
                "The Harbor",
                "two.md",
                "Ships crowded the busy harbor at dawn.",
            ),
            chapter(
                "The Duel",
                "three.md",
                "He raised his sword against the knight.",
            ),
        ];
        let mut index = Index::default();
        let embedded = index
            .refresh(&chapters, Breadth::Normal, &embedder)
            .unwrap();
        assert_eq!(embedded, 3, "all three chapters embedded on first build");
        assert_eq!(index.passage_count(), 3);

        // A query about the locket ranks the locket chapter first, and it's cited.
        let hits = index.search("where is the locket", 2, &embedder).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].passage.title, "The Locket");
        assert_eq!(hits[0].passage.file, "one.md");
        assert!(hits[0].score > hits[1].score || hits[1].score == 0.0);
    }

    #[test]
    fn refresh_only_re_embeds_changed_chapters() {
        // A counting embedder proves only changed passages are re-embedded (#90).
        let embedder = Counting {
            calls: std::cell::Cell::new(0),
        };
        let mut chapters = vec![
            chapter("A", "a.md", "alpha content here"),
            chapter("B", "b.md", "beta content here"),
        ];
        let mut index = Index::default();
        index
            .refresh(&chapters, Breadth::Normal, &embedder)
            .unwrap();
        assert_eq!(embedder.calls.get(), 2, "both chapters embedded first time");

        // No change → nothing re-embedded.
        let n = index
            .refresh(&chapters, Breadth::Normal, &embedder)
            .unwrap();
        assert_eq!(n, 0);
        assert_eq!(
            embedder.calls.get(),
            2,
            "unchanged manuscript re-embeds nothing"
        );

        // Change one chapter → only it re-embeds.
        chapters[1].body = "beta content, revised".into();
        let n = index
            .refresh(&chapters, Breadth::Normal, &embedder)
            .unwrap();
        assert_eq!(n, 1);
        assert_eq!(
            embedder.calls.get(),
            3,
            "only the changed chapter re-embeds"
        );
        assert_eq!(index.passage_count(), 2);
    }

    #[test]
    fn removed_chapter_drops_from_the_index() {
        let embedder = FakeEmbedder {
            keywords: vec!["a", "b"],
        };
        let chapters = vec![chapter("A", "a.md", "aaa"), chapter("B", "b.md", "bbb")];
        let mut index = Index::default();
        index
            .refresh(&chapters, Breadth::Normal, &embedder)
            .unwrap();
        assert_eq!(index.passage_count(), 2);

        // Drop chapter B → its passage leaves the index.
        let fewer = vec![chapter("A", "a.md", "aaa")];
        index.refresh(&fewer, Breadth::Normal, &embedder).unwrap();
        assert_eq!(index.passage_count(), 1);
        assert!(index.digests().contains_key("a.md"));
        assert!(!index.digests().contains_key("b.md"));
    }

    #[test]
    fn changing_the_embedder_model_rebuilds() {
        let a = FakeEmbedder {
            keywords: vec!["x"],
        };
        let chapters = vec![chapter("A", "a.md", "xxx")];
        let mut index = Index::default();
        index.refresh(&chapters, Breadth::Normal, &a).unwrap();
        let tag_before = index.model_tag.clone();

        // A different embedder (different tag) forces a full rebuild.
        let n = index.refresh(&chapters, Breadth::Normal, &Other).unwrap();
        assert_eq!(n, 1, "the model swap re-embeds every chapter");
        assert_ne!(index.model_tag, tag_before);
    }

    #[test]
    fn empty_index_search_returns_nothing() {
        let embedder = FakeEmbedder {
            keywords: vec!["x"],
        };
        let index = Index::default();
        assert!(index.search("anything", 5, &embedder).unwrap().is_empty());
    }

    #[test]
    fn rank_orders_by_cosine_and_respects_top_k() {
        // Drive ranking directly with injected vectors (no embedder round-trip).
        let passages = [
            ("a.md", vec![1.0, 0.0]),
            ("b.md", vec![0.0, 1.0]),
            ("c.md", vec![0.7, 0.7]),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, (file, mut vector))| {
            normalize(&mut vector);
            IndexedPassage {
                passage: Passage {
                    title: format!("Ch {i}"),
                    file: file.into(),
                    text: format!("passage {i}"),
                },
                digest: "d".into(),
                vector,
            }
        })
        .collect();
        let index = Index {
            model_tag: "fake".into(),
            breadth: Breadth::Normal,
            passages,
        };
        // Query aligned with the x-axis: a.md is nearest, then the diagonal c.md.
        let mut q = vec![1.0, 0.0];
        normalize(&mut q);
        let hits = index.rank(&q, 2);
        assert_eq!(hits.len(), 2, "top_k caps the result");
        assert_eq!(hits[0].passage.file, "a.md");
        assert_eq!(hits[1].passage.file, "c.md");
    }

    #[test]
    fn index_round_trips_through_disk() {
        let embedder = FakeEmbedder {
            keywords: vec!["gold"],
        };
        let dir = std::env::temp_dir().join(format!("quill-embed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let chapters = vec![chapter("Gold", "g.md", "The gold coin gleamed.")];
        let mut index = Index::default();
        index
            .refresh(&chapters, Breadth::Normal, &embedder)
            .unwrap();
        index.save(&dir).unwrap();

        let loaded = Index::load(&dir);
        assert_eq!(loaded.passage_count(), 1);
        let hits = loaded.search("gold", 1, &embedder).unwrap();
        assert_eq!(hits[0].passage.title, "Gold");
        std::fs::remove_dir_all(&dir).ok();
    }

    // #1099: the index is written atomically, so a crash mid-write leaves the prior
    // `.embed-index.json` intact rather than a truncated file the next load would
    // discard and rebuild from scratch. The atomic fail-point simulates a crash after
    // the temp is staged but before the rename — the window a truncate-then-write
    // would already have corrupted.
    #[test]
    fn a_failed_save_leaves_the_prior_index_intact() {
        let embedder = FakeEmbedder {
            keywords: vec!["gold", "silver"],
        };
        let dir = std::env::temp_dir().join(format!("quill-embed-atomic-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let index_file = dir.join("bible").join(".embed-index.json");

        // A first, good index on disk.
        let mut index = Index::default();
        index
            .refresh(
                &[chapter("Gold", "g.md", "The gold coin gleamed.")],
                Breadth::Normal,
                &embedder,
            )
            .unwrap();
        index.save(&dir).unwrap();
        let before = std::fs::read_to_string(&index_file).unwrap();

        // Re-embed a changed manuscript, then arm a crash before the save's rename.
        index
            .refresh(
                &[chapter("Silver", "s.md", "The silver coin gleamed.")],
                Breadth::Normal,
                &embedder,
            )
            .unwrap();
        crate::atomic::fail_point::arm_pre_rename_failure();
        let err = index
            .save(&dir)
            .expect_err("the armed save fails before its rename");
        assert!(err.to_string().contains("injected"));

        // The file on disk is byte-for-byte the prior good index — never truncated —
        // and still loads and searches as the original.
        assert_eq!(
            std::fs::read_to_string(&index_file).unwrap(),
            before,
            "the previous embedding index must survive a failed save"
        );
        let loaded = Index::load(&dir);
        assert_eq!(loaded.passage_count(), 1);
        let hits = loaded.search("gold", 1, &embedder).unwrap();
        assert_eq!(hits[0].passage.title, "Gold");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn similar_excludes_the_query_passage_and_ranks_the_echo_first() {
        // The find-similar primitive (#924): querying with a passage's own prose
        // drops that passage (a self-match) and surfaces the ones that echo it.
        let embedder = FakeEmbedder {
            keywords: vec!["storm", "calm"],
        };
        let chapters = vec![
            chapter(
                "Storm A",
                "one.md",
                "A sudden storm broke over the storm-lit bay.",
            ),
            chapter(
                "Storm B",
                "two.md",
                "Another storm rolled in, a storm of wind.",
            ),
            chapter("Calm", "three.md", "The calm, calm morning was still."),
        ];
        let mut index = Index::default();
        index
            .refresh(&chapters, Breadth::Normal, &embedder)
            .unwrap();

        // Query with chapter one's exact prose: it must not return itself, and the
        // other storm-heavy passage ranks above the calm one.
        let query = "A sudden storm broke over the storm-lit bay.";
        let hits = index.similar(query, 5, &embedder).unwrap();
        assert!(
            hits.iter().all(|h| h.passage.text != query),
            "the query's own passage is excluded: {hits:?}"
        );
        assert_eq!(
            hits[0].passage.file, "two.md",
            "the echoing storm passage ranks first"
        );
        // The calm passage (no shared keyword) scores 0 and ranks last.
        assert_eq!(hits.last().unwrap().passage.file, "three.md");
    }

    #[test]
    fn similar_from_a_typed_query_behaves_like_search() {
        // A typed query rarely matches a passage verbatim, so nothing is excluded
        // and the most relevant passage still ranks first.
        let embedder = FakeEmbedder {
            keywords: vec!["locket", "harbor"],
        };
        let chapters = vec![
            chapter("Locket", "one.md", "She held the locket."),
            chapter("Harbor", "two.md", "Ships filled the harbor."),
        ];
        let mut index = Index::default();
        index
            .refresh(&chapters, Breadth::Normal, &embedder)
            .unwrap();
        let hits = index
            .similar("something about a locket", 2, &embedder)
            .unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].passage.file, "one.md");
    }

    #[test]
    fn relevant_prior_retrieves_the_bearing_chapter_and_excludes_the_changed_one() {
        // The retrieval-backed incremental pass (#929): a changed chapter re-raises
        // the locket; retrieval must surface the *prior* locket scene as context and
        // never re-send the changed chapter itself (it's already in the request).
        let embedder = FakeEmbedder {
            keywords: vec!["locket", "harbor", "sword"],
        };
        let chapters = vec![
            chapter(
                "The Locket",
                "one.md",
                "She clutched the silver locket tightly.",
            ),
            chapter(
                "The Harbor",
                "two.md",
                "Ships crowded the busy harbor at dawn.",
            ),
            chapter(
                "The Duel",
                "three.md",
                // The changed chapter: a sword scene that also mentions the locket.
                "He drew his sword, the locket at his throat.",
            ),
        ];
        let mut index = Index::default();
        index
            .refresh(&chapters, Breadth::Normal, &embedder)
            .unwrap();

        // Chapter three changed; its own prose is the query. The prior locket scene
        // (one.md) is the passage that bears on the change and must rank first; the
        // changed chapter (three.md) is excluded even though it's maximally similar
        // to itself; the unrelated harbor (two.md) scores 0.
        let query = "He drew his sword, the locket at his throat.";
        let changed = vec!["three.md".to_string()];
        let hits = index.relevant_prior(query, &changed, 5, &embedder).unwrap();
        assert!(
            hits.iter().all(|h| h.passage.file != "three.md"),
            "the changed chapter is never in its own retrieved context: {hits:?}"
        );
        assert_eq!(
            hits[0].passage.file, "one.md",
            "the prior locket scene ranks first: {hits:?}"
        );
        assert_eq!(hits[0].passage.title, "The Locket", "citation preserved");
    }

    #[test]
    fn relevant_prior_respects_top_k_after_excluding_changed() {
        // Excluding the changed chapters happens before truncation, so `top_k`
        // still yields that many *prior* hits when the index holds them.
        let embedder = FakeEmbedder {
            keywords: vec!["a"],
        };
        let chapters = vec![
            chapter("Ch1", "one.md", "aaa aaa"),
            chapter("Ch2", "two.md", "aaa"),
            chapter("Ch3", "three.md", "aaa aaa aaa"), // the changed chapter
        ];
        let mut index = Index::default();
        index
            .refresh(&chapters, Breadth::Normal, &embedder)
            .unwrap();
        let changed = vec!["three.md".to_string()];
        let hits = index.relevant_prior("aaa", &changed, 2, &embedder).unwrap();
        assert_eq!(hits.len(), 2, "two prior hits despite the excluded chapter");
        assert!(hits.iter().all(|h| h.passage.file != "three.md"));
    }

    #[test]
    fn relevant_prior_on_an_empty_index_returns_nothing() {
        // No index (embeddings unavailable / never built) → no retrieved context,
        // and the caller falls back to the whole-book incremental path (#929).
        let embedder = FakeEmbedder {
            keywords: vec!["x"],
        };
        let index = Index::default();
        assert!(index
            .relevant_prior("anything", &["a.md".into()], 5, &embedder)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn echo_pairs_flags_near_duplicates_above_threshold() {
        // Echo detection (#920): three chapters, two of which are near-identical.
        // A high threshold flags only the near-duplicate; a lower one also catches
        // the softer echo, and an unrelated passage is never paired.
        // Vectors chosen directly so similarity is exact and threshold behavior is
        // unambiguous.
        let passages = [
            ("dup1.md", vec![1.0, 0.0, 0.0]),   // near-duplicate of dup2
            ("dup2.md", vec![0.99, 0.14, 0.0]), // ~0.99 cosine with dup1
            ("echo.md", vec![0.7, 0.7, 0.0]),   // a softer echo of dup1 (~0.7)
            ("far.md", vec![0.0, 0.0, 1.0]),    // orthogonal to all — never paired
        ]
        .into_iter()
        .enumerate()
        .map(|(i, (file, mut vector))| {
            normalize(&mut vector);
            IndexedPassage {
                passage: Passage {
                    title: format!("Ch {i}"),
                    file: file.into(),
                    text: format!("passage {i}"),
                },
                digest: "d".into(),
                vector,
            }
        })
        .collect();
        let index = Index {
            model_tag: "fake".into(),
            breadth: Breadth::Normal,
            passages,
        };

        // High threshold: only the near-duplicate pair (dup1 ↔ dup2).
        let near = index.echo_pairs(0.95);
        assert_eq!(
            near.len(),
            1,
            "only the near-duplicate clears 0.95: {near:?}"
        );
        let files = [near[0].a.file.as_str(), near[0].b.file.as_str()];
        assert!(files.contains(&"dup1.md") && files.contains(&"dup2.md"));
        assert!(near[0].score >= 0.95);

        // Lower threshold: the soft echo (echo.md) also pairs, and the highest-
        // similarity pair sorts first.
        let soft = index.echo_pairs(0.6);
        assert!(soft.len() >= 3, "soft echoes surface at 0.6: {soft:?}");
        assert!(
            soft[0].score >= soft[1].score,
            "pairs are ordered by descending similarity"
        );
        // The orthogonal passage is never in any pair.
        assert!(
            soft.iter()
                .all(|p| p.a.file != "far.md" && p.b.file != "far.md"),
            "an unrelated passage is never flagged: {soft:?}"
        );
    }

    #[test]
    fn echo_pairs_skips_within_chapter_repeats() {
        // Two identical passages in the *same* chapter (a deliberate in-scene
        // refrain) are not flagged; the same text across chapters is.
        let make = |file: &str, text: &str, v: Vec<f32>| {
            let mut vector = v;
            normalize(&mut vector);
            IndexedPassage {
                passage: Passage {
                    title: file.into(),
                    file: file.into(),
                    text: text.into(),
                },
                digest: "d".into(),
                vector,
            }
        };
        let index = Index {
            model_tag: "fake".into(),
            breadth: Breadth::Normal,
            passages: vec![
                make("a.md", "the refrain", vec![1.0, 0.0]),
                make("a.md", "the refrain again", vec![1.0, 0.0]), // same chapter → skipped
                make("b.md", "the refrain elsewhere", vec![1.0, 0.0]), // cross-chapter → flagged
            ],
        };
        let pairs = index.echo_pairs(0.9);
        // Only the cross-chapter pairs (a↔b, twice) are flagged; the a↔a pair isn't.
        assert!(
            pairs.iter().all(|p| p.a.file != p.b.file),
            "no within-chapter pair is flagged: {pairs:?}"
        );
        assert_eq!(
            pairs.len(),
            2,
            "both a.md passages pair with b.md: {pairs:?}"
        );
    }

    /// End-to-end retrieval with the *real* candle embedding model (#831): download
    /// MiniLM via `hf-hub`, index three chapters, and confirm a semantic query
    /// retrieves the right chapter — including a **paraphrase** the words of which
    /// don't appear in the passage, the thing lexical search misses. Ignored
    /// (needs network + disk); not hermetic, so it's kept out of the default and
    /// `--features local` suites (#531). Run with:
    /// `cargo test --features local live_retrieval -- --ignored --nocapture`
    #[test]
    #[cfg(feature = "local")]
    #[ignore = "integration: downloads the embedding model; not hermetic (#831)"]
    fn live_retrieval_finds_the_paraphrased_chapter() {
        let model = crate::ai::model_select::EmbedModel::default_model();
        let embedder = LocalEmbedder::for_model(model).expect("download + load MiniLM");

        let chapters = vec![
            chapter(
                "The Harbor",
                "one.md",
                "Fishing boats rocked against the pier as gulls wheeled overhead.",
            ),
            chapter(
                "The Duel",
                "two.md",
                "Steel rang on steel as the two swordsmen circled each other.",
            ),
            chapter(
                "The Feast",
                "three.md",
                "Long tables groaned under roast venison, warm bread, and spiced wine.",
            ),
        ];
        let mut index = Index::default();
        let embedded = index
            .refresh(&chapters, Breadth::Normal, &embedder)
            .expect("index");
        assert_eq!(embedded, 3);

        // A paraphrase — none of these words appear in the passage — should still
        // retrieve "The Feast" first, the semantic recall lexical search can't do.
        let hits = index
            .search("a big meal with lots of food", 3, &embedder)
            .expect("search");
        println!(
            "LIVE RETRIEVAL: {:?}",
            hits.iter()
                .map(|h| (&h.passage.title, h.score))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            hits[0].passage.title, "The Feast",
            "paraphrase retrieves the feast chapter: {hits:?}"
        );

        // A refresh over the unchanged manuscript re-embeds nothing (incremental #90).
        assert_eq!(
            index
                .refresh(&chapters, Breadth::Normal, &embedder)
                .expect("refresh"),
            0
        );
    }
}
