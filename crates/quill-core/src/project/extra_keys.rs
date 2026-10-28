//! Carrying `book.toml`'s unmodeled top-level keys through a manifest save.
//!
//! The manifest is a typed struct re-serialized whole on every structural save
//! ([`Project::save_manifest`](super::Project::save_manifest)), so a top-level key
//! the struct doesn't model would otherwise be dropped. That silently erased the
//! `[quill]` table — the per-project config overrides (`[quill.editor]`,
//! `[quill.snippets]`, …, #710) live there and no manifest type models them — and
//! any additive key a newer 1.x wrote (the forward-compat promise, #1097).
//!
//! The fix is a two-step merge rather than `#[serde(flatten)]` (which would push
//! the whole manifest through serde's buffered map path and lose the struct's
//! "scalars before tables" field order): [`unknown`] picks the keys a manifest
//! shape doesn't declare out of the parsed file, and [`append`] re-emits them
//! around the struct's own serialization — unknown scalars ahead of it, unknown
//! tables after it — so the output stays valid TOML. A manifest with nothing
//! unknown serializes byte-for-byte as before.
//!
//! The known-key set comes straight from the serde derive ([`field_names`]), so a
//! field added to `Manifest` or the series file is recognized with no list to
//! keep in sync.

use serde::de::{self, DeserializeOwned, Deserializer, Visitor};
use serde::forward_to_deserialize_any;
use toml::{Table, Value};

/// A throwaway [`Deserializer`] that records the field list a derived
/// `Deserialize` impl passes to `deserialize_struct`, then bails. Only struct
/// requests are meaningful; anything else errors.
#[derive(Default)]
struct FieldCapture(Option<&'static [&'static str]>);

impl<'de> Deserializer<'de> for &mut FieldCapture {
    type Error = de::value::Error;

    fn deserialize_any<V: Visitor<'de>>(self, _visitor: V) -> Result<V::Value, Self::Error> {
        Err(de::Error::custom("field capture: not a struct"))
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        fields: &'static [&'static str],
        _visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.0 = Some(fields);
        Err(de::Error::custom("field capture: done"))
    }

    forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf option unit unit_struct newtype_struct seq tuple
        tuple_struct map enum identifier ignored_any
    }
}

/// The serialized field names of a `#[derive(Deserialize)]` struct — the
/// top-level keys it models. `#[serde(skip)]` fields aren't keys and aren't
/// listed. Cheap: the derive hands the list over before reading any input.
pub fn field_names<T: DeserializeOwned>() -> &'static [&'static str] {
    let mut capture = FieldCapture::default();
    // The capture always errors (it never builds a `T`); the field list is the
    // side effect we want.
    let _ = T::deserialize(&mut capture);
    capture.0.unwrap_or_default()
}

/// The entries of `doc` whose keys aren't in `known` — what a typed manifest
/// save would drop.
pub fn unknown(mut doc: Table, known: &[&str]) -> Table {
    doc.retain(|key, _| !known.contains(&key));
    doc
}

/// Whether `value` serializes as a TOML table section (`[t]` or `[[t]]`) rather
/// than a `key = value` line: a table, or a non-empty array of only tables.
fn is_section(value: &Value) -> bool {
    match value {
        Value::Table(_) => true,
        Value::Array(items) => !items.is_empty() && items.iter().all(Value::is_table),
        _ => false,
    }
}

/// Re-attach `extra` (the file's unknown top-level keys) to `text`, the typed
/// manifest's own serialization. Keys in `known` — the shape being written — are
/// skipped so the typed value always wins and no key is emitted twice (e.g. a
/// stray key after converting between the flat and `[[books]]` shapes).
///
/// Ordering keeps the file valid TOML: unknown `key = value` lines go *ahead* of
/// `text` (a top-level key may not follow a table header), unknown tables go
/// *after* it. With nothing to add, `text` is returned unchanged — byte-for-byte
/// the old output.
pub fn append(text: String, extra: &Table, known: &[&str]) -> Result<String, toml::ser::Error> {
    let mut values = Table::new();
    let mut sections = Table::new();
    for (key, value) in extra {
        if known.contains(&key.as_str()) {
            continue;
        }
        let side = if is_section(value) {
            &mut sections
        } else {
            &mut values
        };
        side.insert(key.clone(), value.clone());
    }
    if values.is_empty() && sections.is_empty() {
        return Ok(text);
    }
    let mut out = String::new();
    if !values.is_empty() {
        out.push_str(&toml::to_string_pretty(&values)?);
    }
    out.push_str(&text);
    if !sections.is_empty() {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n');
        out.push_str(&toml::to_string_pretty(&sections)?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;
    use toml::{Table, Value};

    use super::{append, field_names, unknown};

    #[derive(Deserialize)]
    #[allow(dead_code, reason = "only the derive's field list is under test")]
    struct Shape {
        title: String,
        #[serde(skip)]
        cache: usize,
        #[serde(default)]
        chapters: Vec<String>,
    }

    #[test]
    fn field_names_come_from_the_derive_and_skip_skipped_fields() {
        assert_eq!(field_names::<Shape>(), ["title", "chapters"]);
    }

    #[test]
    fn unknown_keeps_only_unmodeled_keys() {
        let doc: Table =
            toml::from_str("title = \"T\"\nfuture = 1\n[quill.editor]\nvim = true\n").unwrap();
        let extra = unknown(doc, field_names::<Shape>());
        let keys: Vec<&str> = extra.keys().map(String::as_str).collect();
        assert_eq!(keys, ["future", "quill"]);
    }

    #[test]
    fn nothing_unknown_returns_the_text_unchanged() {
        let text = "title = \"T\"\n".to_string();
        assert_eq!(append(text.clone(), &Table::new(), &[]).unwrap(), text);
    }

    // Unknown scalars go ahead of the typed text and unknown tables (including an
    // array of tables) after it, so the merged file parses and keeps every value.
    #[test]
    fn append_keeps_the_merged_document_valid() {
        let typed = "title = \"T\"\n\n[[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n";
        let extra: Table = toml::from_str(
            "future = \"x\"\nlist = [1, 2]\nempty = []\n\
             [quill.editor]\ntypewriter = true\n\
             [[future_rows]]\nn = 1\n[[future_rows]]\nn = 2\n",
        )
        .unwrap();
        let out = append(typed.to_string(), &extra, &["title", "chapters"]).unwrap();
        let doc: Table = toml::from_str(&out).unwrap_or_else(|e| panic!("{e}\n{out}"));
        assert_eq!(doc["future"].as_str(), Some("x"));
        assert_eq!(doc["list"].as_array().map(Vec::len), Some(2));
        assert_eq!(doc["empty"].as_array().map(Vec::len), Some(0));
        assert_eq!(doc["quill"]["editor"]["typewriter"].as_bool(), Some(true));
        assert_eq!(doc["future_rows"].as_array().map(Vec::len), Some(2));
        assert_eq!(doc["chapters"][0]["file"].as_str(), Some("one.md"));
        assert!(
            out.contains("[[chapters]]") && out.contains("[[future_rows]]"),
            "{out}"
        );
    }

    // A key the shape being written models is never emitted from the leftovers —
    // the typed value wins and the file never carries a duplicate key.
    #[test]
    fn append_skips_keys_the_written_shape_models() {
        let mut extra = Table::new();
        extra.insert("books".into(), Value::Array(Vec::new()));
        let out = append("title = \"T\"\n".to_string(), &extra, &["title", "books"]).unwrap();
        assert_eq!(out, "title = \"T\"\n");
    }
}
