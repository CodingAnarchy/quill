//! The AI worker: backend resolution, request spawning on the worker thread,
//! incremental-scope plumbing (#90), and the `poll_ai` outcome router. Extracted
//! from `assist.rs` (#315), then split by concern into three cohesive submodules
//! (#839): [`download`] (model download / status against the `hf-hub` cache),
//! [`backend`] (backend spec + request spawning + the poll router), and
//! [`bible`] (persistent-bible tracker passes + incremental planning). This
//! `mod.rs` re-exports the public surface so every caller is unaffected — the
//! `impl App` methods land on `App` from whichever submodule defines them.

mod backend;
mod bible;
mod download;

pub(in crate::app) use backend::BackendSpec;
pub(in crate::app) use bible::bible_tracker;
pub(in crate::app) use download::{
    download_model, embed_model_downloaded, model_downloaded, remove_model_files, LOCAL_UNAVAILABLE,
};

#[cfg(feature = "local")]
pub(in crate::app) use download::recommended_model;
