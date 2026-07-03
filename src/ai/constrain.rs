//! Constrained JSON decoding (#124): given a JSON Schema, decide whether a
//! partial string is still on a path to a schema-valid value. A local model
//! (candle, behind a follow-up feature) drives this to mask its logits each step
//! so it can only ever emit valid JSON — the same guarantee the Claude backend
//! gets from the API.
//!
//! This module is **pure and tokenizer-agnostic**: it works on accumulated text
//! and candidate token pieces, so all the hard logic is unit-testable offline
//! without a model. The decode loop ranks the vocabulary by logit and offers the
//! candidates to [`Constraint::pick`], which accepts the best one that keeps the
//! output viable; decoding stops when [`Constraint::is_complete`].
//!
//! ## Supported schema subset
//! The trackers' shapes: `object` (with `properties` + `required`), `array`
//! (`items`), `string` (with optional string `enum`), `integer` / `number`,
//! `boolean`, `null`, and a permissive "any value" when a schema has no
//! recognizable `type`/`enum`. Numbers complete when a delimiter follows (always
//! true inside objects/arrays); a bare top-level number is reported `Partial`.
//!
//! Its consumer is the candle decode loop in [`crate::ai::local`], so this module
//! is compiled only under `--features local` (gated at the `mod` declaration); the
//! logic is fully exercised by this module's tests in that profile.

use serde_json::Value;

/// Whether a partial string is a viable prefix of a schema-valid JSON value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prefix {
    /// No continuation can match the schema — this text must never be emitted.
    Invalid,
    /// Valid so far but incomplete; more tokens are needed.
    Partial,
    /// A complete value matching the schema; decoding can stop.
    Complete,
}

/// Classify `partial` against `schema`: is it invalid, a viable-but-incomplete
/// prefix, or already a complete matching value?
pub fn prefix_status(schema: &Value, partial: &str) -> Prefix {
    match consume(schema, partial) {
        Outcome::Invalid => Prefix::Invalid,
        Outcome::Partial => Prefix::Partial,
        // A complete value with only whitespace left is done; trailing junk isn't.
        Outcome::Complete(rest) if rest.trim_start().is_empty() => Prefix::Complete,
        Outcome::Complete(_) => Prefix::Invalid,
    }
}

/// Drives constrained greedy decoding: accumulates accepted text and picks the
/// best-ranked candidate token that keeps the output schema-valid.
pub struct Constraint<'a> {
    schema: &'a Value,
    text: String,
}

impl<'a> Constraint<'a> {
    pub const fn new(schema: &'a Value) -> Self {
        Self {
            schema,
            text: String::new(),
        }
    }

    /// Whether the accumulated output is already a complete schema-valid value.
    pub fn is_complete(&self) -> bool {
        prefix_status(self.schema, &self.text) == Prefix::Complete
    }

    /// From candidate `(id, piece)` pairs in **descending preference** (logit)
    /// order, accept the first whose `piece` keeps the output a viable prefix and
    /// return its id; `None` if none is viable (the decode loop then stops).
    pub fn pick<'p>(
        &mut self,
        candidates: impl IntoIterator<Item = (u32, &'p str)>,
    ) -> Option<u32> {
        for (id, piece) in candidates {
            let mut next = String::with_capacity(self.text.len() + piece.len());
            next.push_str(&self.text);
            next.push_str(piece);
            if prefix_status(self.schema, &next) != Prefix::Invalid {
                self.text = next;
                return Some(id);
            }
        }
        None
    }

    /// The accumulated output so far.
    pub fn text(&self) -> &str {
        &self.text
    }
}

/// The result of matching one value against a schema: a complete value (with the
/// unconsumed tail), a viable-but-incomplete prefix, or an impossible one.
enum Outcome<'s> {
    Complete(&'s str),
    Partial,
    Invalid,
}

fn skip_ws(s: &str) -> &str {
    s.trim_start_matches([' ', '\t', '\n', '\r'])
}

/// Match a single JSON value of `schema` at the start of `s` (after whitespace).
fn consume<'s>(schema: &Value, s: &'s str) -> Outcome<'s> {
    if let Some(Value::Array(options)) = schema.get("enum") {
        return consume_enum(options, s);
    }
    match schema.get("type").and_then(Value::as_str) {
        Some("object") => consume_object(schema, s),
        Some("array") => consume_array(schema, s),
        Some("string") => match scan_string(s, None) {
            StrOut::Complete { rest, .. } => Outcome::Complete(rest),
            StrOut::Partial => Outcome::Partial,
            StrOut::Invalid => Outcome::Invalid,
        },
        Some("integer") => consume_number(s, true),
        Some("number") => consume_number(s, false),
        Some("boolean") => consume_literals(s, &["true", "false"]),
        Some("null") => consume_literals(s, &["null"]),
        _ => consume_any(s), // no recognizable type → any JSON value
    }
}

/// A string `enum`: the value must equal one of the options (and, while typing,
/// be a prefix of one).
fn consume_enum<'s>(options: &[Value], s: &'s str) -> Outcome<'s> {
    let allowed: Vec<&str> = options.iter().filter_map(Value::as_str).collect();
    match scan_string(s, Some(&allowed)) {
        StrOut::Complete { rest, .. } => Outcome::Complete(rest),
        StrOut::Partial => Outcome::Partial,
        StrOut::Invalid => Outcome::Invalid,
    }
}

/// The result of structurally scanning a JSON string, optionally constrained to a
/// set of allowed full values (for enums and object keys).
enum StrOut<'s> {
    Complete { content: String, rest: &'s str },
    Partial,
    Invalid,
}

/// Scan a JSON string at the start of `s`. With `allowed`, the content so far must
/// stay a prefix of some option (and equal one when closed). Assumes allowed
/// values contain no escapes (true for the trackers' keys/enums).
fn scan_string<'s>(s: &'s str, allowed: Option<&[&str]>) -> StrOut<'s> {
    let s = skip_ws(s);
    let bytes = s.as_bytes();
    match bytes.first() {
        None => return StrOut::Partial, // could start a string
        Some(b'"') => {}
        Some(_) => return StrOut::Invalid,
    }
    let mut i = 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => {
                if i + 1 >= bytes.len() {
                    return partial_string(&s[1..i], allowed); // dangling escape
                }
                i += 2;
            }
            b'"' => {
                let content = &s[1..i];
                if let Some(allowed) = allowed {
                    if !allowed.contains(&content) {
                        return StrOut::Invalid;
                    }
                }
                return StrOut::Complete {
                    content: content.to_string(),
                    rest: &s[i + 1..],
                };
            }
            _ => i += 1,
        }
    }
    partial_string(&s[1..], allowed) // no closing quote yet
}

/// An unterminated string is `Partial` only if its content can still extend to an
/// allowed value (always, when unconstrained).
fn partial_string<'s>(content_so_far: &str, allowed: Option<&[&str]>) -> StrOut<'s> {
    match allowed {
        Some(allowed) if !allowed.iter().any(|a| a.starts_with(content_so_far)) => StrOut::Invalid,
        _ => StrOut::Partial,
    }
}

/// Match one of a fixed set of literals (`true`/`false`/`null`).
fn consume_literals<'s>(s: &'s str, literals: &[&str]) -> Outcome<'s> {
    let s = skip_ws(s);
    for lit in literals {
        if let Some(rest) = s.strip_prefix(lit) {
            return Outcome::Complete(rest);
        }
    }
    if literals.iter().any(|lit| lit.starts_with(s)) {
        Outcome::Partial // s is a prefix of a literal (incl. empty s)
    } else {
        Outcome::Invalid
    }
}

/// Match a JSON number. `integer` forbids a fraction/exponent. A number is
/// `Complete` only once a non-number character follows it; while the text is still
/// all number characters it is `Partial` (a delimiter is expected next, which
/// holds inside every object/array).
fn consume_number(s: &str, integer: bool) -> Outcome<'_> {
    let s = skip_ws(s);
    let b = s.as_bytes();
    let n = b.len();
    let mut i = 0;
    if i < n && b[i] == b'-' {
        i += 1;
    }
    let digits_start = i;
    while i < n && b[i].is_ascii_digit() {
        i += 1;
    }
    if i == digits_start {
        // no integer digits yet: "-" alone (or empty) at EOF is a viable prefix
        return if i == n {
            Outcome::Partial
        } else {
            Outcome::Invalid
        };
    }
    if i == n {
        return Outcome::Partial; // all digits so far; a delimiter is expected next
    }
    if !integer && b[i] == b'.' {
        i += 1;
        let frac_start = i;
        while i < n && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == frac_start {
            return if i == n {
                Outcome::Partial
            } else {
                Outcome::Invalid
            };
        }
        if i == n {
            return Outcome::Partial;
        }
    }
    if !integer && (b[i] == b'e' || b[i] == b'E') {
        i += 1;
        if i < n && (b[i] == b'+' || b[i] == b'-') {
            i += 1;
        }
        let exp_start = i;
        while i < n && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == exp_start {
            return if i == n {
                Outcome::Partial
            } else {
                Outcome::Invalid
            };
        }
        if i == n {
            return Outcome::Partial;
        }
    }
    Outcome::Complete(&s[i..]) // a non-number character ends the number
}

/// Match an object against its `properties` / `required`. Keys are constrained to
/// the schema's property names (not yet used); the closing `}` is only allowed
/// once every required key has appeared.
fn consume_object<'s>(schema: &Value, s: &'s str) -> Outcome<'s> {
    let s = skip_ws(s);
    let mut rest = match s.strip_prefix('{') {
        Some(r) => r,
        None if s.is_empty() => return Outcome::Partial,
        None => return Outcome::Invalid,
    };

    let empty = serde_json::Map::new();
    let props = schema
        .get("properties")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();

    let mut seen: Vec<&str> = Vec::new();
    let mut first = true;
    loop {
        rest = skip_ws(rest);
        if rest.is_empty() {
            return Outcome::Partial;
        }
        // `}` closes the object, but only when every required key is present.
        if first && rest.starts_with('}') {
            return close_object(rest, &required, &seen);
        }
        // A key: a string equal to one of the not-yet-seen property names.
        let remaining: Vec<&str> = props
            .keys()
            .map(String::as_str)
            .filter(|k| !seen.contains(k))
            .collect();
        let key = match scan_string(rest, Some(&remaining)) {
            StrOut::Complete { content, rest: r } => {
                // Borrow the matching property name (lives as long as `schema`).
                let name = match props.keys().find(|k| *k == &content) {
                    Some(k) => k.as_str(),
                    None => return Outcome::Invalid,
                };
                rest = r;
                name
            }
            StrOut::Partial => return Outcome::Partial,
            StrOut::Invalid => return Outcome::Invalid,
        };
        seen.push(key);

        rest = skip_ws(rest);
        rest = match rest.strip_prefix(':') {
            Some(r) => r,
            None if rest.is_empty() => return Outcome::Partial,
            None => return Outcome::Invalid,
        };

        match consume(&props[key], rest) {
            Outcome::Complete(r) => rest = r,
            Outcome::Partial => return Outcome::Partial,
            Outcome::Invalid => return Outcome::Invalid,
        }

        rest = skip_ws(rest);
        if rest.is_empty() {
            return Outcome::Partial;
        }
        if let Some(r) = rest.strip_prefix(',') {
            // No additional properties: once every property is used, a comma can't
            // be satisfied (no key would follow), so only `}` is valid.
            if seen.len() == props.len() {
                return Outcome::Invalid;
            }
            rest = r;
            first = false;
            continue;
        }
        if rest.starts_with('}') {
            return close_object(rest, &required, &seen);
        }
        return Outcome::Invalid;
    }
}

/// Resolve a `}`: complete only if every required key has been seen.
fn close_object<'s>(at_brace: &'s str, required: &[&str], seen: &[&str]) -> Outcome<'s> {
    if required.iter().all(|r| seen.contains(r)) {
        Outcome::Complete(&at_brace[1..])
    } else {
        Outcome::Invalid // would close before all required keys appear
    }
}

/// Match an array against its `items` schema (no trailing comma; `]` closes it).
fn consume_array<'s>(schema: &Value, s: &'s str) -> Outcome<'s> {
    let s = skip_ws(s);
    let mut rest = match s.strip_prefix('[') {
        Some(r) => r,
        None if s.is_empty() => return Outcome::Partial,
        None => return Outcome::Invalid,
    };
    let items = schema.get("items").unwrap_or(&Value::Null);

    let mut need_value = false; // after a `,`, a value (not `]`) must follow
    loop {
        rest = skip_ws(rest);
        if rest.is_empty() {
            return Outcome::Partial;
        }
        if !need_value {
            if let Some(r) = rest.strip_prefix(']') {
                return Outcome::Complete(r);
            }
        }
        match consume(items, rest) {
            Outcome::Complete(r) => rest = r,
            Outcome::Partial => return Outcome::Partial,
            Outcome::Invalid => return Outcome::Invalid,
        }
        rest = skip_ws(rest);
        if rest.is_empty() {
            return Outcome::Partial;
        }
        if let Some(r) = rest.strip_prefix(',') {
            rest = r;
            need_value = true;
            continue;
        }
        if let Some(r) = rest.strip_prefix(']') {
            return Outcome::Complete(r);
        }
        return Outcome::Invalid;
    }
}

/// Match any JSON value (for a schema with no recognizable `type`/`enum`),
/// dispatching on the first non-whitespace character.
fn consume_any(s: &str) -> Outcome<'_> {
    let t = skip_ws(s);
    match t.as_bytes().first() {
        None => Outcome::Partial,
        Some(b'"') => match scan_string(t, None) {
            StrOut::Complete { rest, .. } => Outcome::Complete(rest),
            StrOut::Partial => Outcome::Partial,
            StrOut::Invalid => Outcome::Invalid,
        },
        Some(b'{') => consume_object(&serde_json::json!({"type": "object"}), t),
        Some(b'[') => consume_array(&serde_json::json!({"type": "array"}), t),
        Some(b't' | b'f') => consume_literals(t, &["true", "false"]),
        Some(b'n') => consume_literals(t, &["null"]),
        Some(c) if c.is_ascii_digit() || *c == b'-' => consume_number(t, false),
        Some(_) => Outcome::Invalid,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn status(schema: &Value, s: &str) -> Prefix {
        prefix_status(schema, s)
    }

    #[test]
    fn string_prefixes() {
        let s = json!({"type": "string"});
        assert_eq!(status(&s, ""), Prefix::Partial);
        assert_eq!(status(&s, "\"hel"), Prefix::Partial);
        assert_eq!(status(&s, "\"hello\""), Prefix::Complete);
        assert_eq!(status(&s, "hello"), Prefix::Invalid); // must be quoted
        assert_eq!(status(&s, "\"hi\" x"), Prefix::Invalid); // trailing junk
    }

    #[test]
    fn number_and_integer() {
        let n = json!({"type": "number"});
        assert_eq!(status(&n, "-"), Prefix::Partial);
        assert_eq!(status(&n, "42"), Prefix::Partial); // delimiter expected next
        assert_eq!(status(&n, "3.14"), Prefix::Partial);
        assert_eq!(status(&n, "1.2e-3"), Prefix::Partial);
        assert_eq!(status(&n, "1.x"), Prefix::Invalid);
        let i = json!({"type": "integer"});
        // an integer schema can't take a fraction: '.' ends the (complete) int,
        // leaving trailing junk → invalid as a standalone value.
        assert_eq!(status(&i, "12.5"), Prefix::Invalid);
        assert_eq!(status(&i, "7"), Prefix::Partial);
    }

    #[test]
    fn boolean_literal() {
        let b = json!({"type": "boolean"});
        assert_eq!(status(&b, ""), Prefix::Partial);
        assert_eq!(status(&b, "tr"), Prefix::Partial);
        assert_eq!(status(&b, "true"), Prefix::Complete);
        assert_eq!(status(&b, "false"), Prefix::Complete);
        assert_eq!(status(&b, "tru e"), Prefix::Invalid);
    }

    #[test]
    fn string_enum_constrains_to_options() {
        let e = json!({"enum": ["protagonist", "antagonist", "minor"]});
        assert_eq!(status(&e, "\"prot"), Prefix::Partial); // prefix of an option
        assert_eq!(status(&e, "\"protagonist\""), Prefix::Complete);
        assert_eq!(status(&e, "\"villain\""), Prefix::Invalid); // not an option
        assert_eq!(status(&e, "\"x"), Prefix::Invalid); // not a prefix of any
    }

    #[test]
    fn array_of_strings() {
        let a = json!({"type": "array", "items": {"type": "string"}});
        assert_eq!(status(&a, "["), Prefix::Partial);
        assert_eq!(status(&a, "[]"), Prefix::Complete);
        assert_eq!(status(&a, "[\"a\""), Prefix::Partial);
        assert_eq!(status(&a, "[\"a\", \"b\"]"), Prefix::Complete);
        assert_eq!(status(&a, "[\"a\",]"), Prefix::Invalid); // trailing comma
        assert_eq!(status(&a, "[1]"), Prefix::Invalid); // item type mismatch
    }

    #[test]
    fn object_keys_and_required() {
        let o = json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "role": {"enum": ["protagonist", "minor"]},
                "traits": {"type": "array", "items": {"type": "string"}}
            },
            "required": ["name", "role", "traits"]
        });
        assert_eq!(status(&o, "{"), Prefix::Partial);
        assert_eq!(status(&o, "{\"na"), Prefix::Partial); // valid key prefix
        assert_eq!(status(&o, "{\"zzz\""), Prefix::Invalid); // not a property
        assert_eq!(status(&o, "{\"name\": \"Mara\"}"), Prefix::Invalid); // missing required
        let full = "{\"name\": \"Mara\", \"role\": \"protagonist\", \"traits\": [\"brave\"]}";
        assert_eq!(status(&o, full), Prefix::Complete);
        // A complete object plus a half-typed next value stays valid as a prefix.
        assert_eq!(
            status(&o, "{\"name\": \"Mara\", \"role\": \"prot"),
            Prefix::Partial
        );
    }

    #[test]
    fn object_rejects_repeated_and_unknown_keys() {
        let o = json!({
            "type": "object",
            "properties": {"a": {"type": "string"}, "b": {"type": "string"}},
            "required": ["a"]
        });
        // "a" already seen → can't appear again.
        assert_eq!(status(&o, "{\"a\": \"x\", \"a"), Prefix::Invalid);
        // "b" is a real remaining key.
        assert_eq!(status(&o, "{\"a\": \"x\", \"b"), Prefix::Partial);
    }

    #[test]
    fn object_forbids_a_comma_once_all_properties_are_used() {
        let o = json!({
            "type": "object",
            "properties": {"a": {"type": "string"}},
            "required": ["a"]
        });
        // All properties present → only `}` may follow, not another `,` (no
        // additional properties), so the value can actually terminate.
        assert_eq!(status(&o, "{\"a\": \"x\""), Prefix::Partial);
        assert_eq!(status(&o, "{\"a\": \"x\"}"), Prefix::Complete);
        assert_eq!(status(&o, "{\"a\": \"x\","), Prefix::Invalid);
    }

    #[test]
    fn constraint_picks_the_first_viable_candidate() {
        let schema = json!({"type": "boolean"});
        let mut c = Constraint::new(&schema);
        // Ranked candidates: a bogus one first, then the valid "true".
        assert_eq!(c.pick([(0, "xyz"), (1, "true")]), Some(1));
        assert!(c.is_complete());
        assert_eq!(c.text(), "true");
        // Nothing viable extends a complete boolean.
        assert_eq!(c.pick([(2, "!")]), None);
    }

    #[test]
    fn constraint_builds_an_object_step_by_step() {
        let schema = json!({
            "type": "object",
            "properties": {"n": {"type": "string"}},
            "required": ["n"]
        });
        let mut c = Constraint::new(&schema);
        // Feed pieces the way a tokenizer would; only viable ones are accepted.
        for piece in ["{", "\"n\"", ":", " \"", "Mara", "\"", "}"] {
            assert_eq!(c.pick([(0, piece)]), Some(0), "rejected {piece:?}");
        }
        assert!(c.is_complete());
        let parsed: Value = serde_json::from_str(c.text()).expect("valid JSON");
        assert_eq!(parsed["n"], "Mara");
    }

    #[test]
    fn constraint_blocks_an_invalid_piece() {
        let schema = json!({"type": "array", "items": {"type": "integer"}});
        let mut c = Constraint::new(&schema);
        assert_eq!(c.pick([(0, "[")]), Some(0));
        // A string item is invalid; only a number/`]` is viable.
        assert_eq!(c.pick([(1, "\"oops\"")]), None);
        assert_eq!(c.pick([(2, "7")]), Some(2));
        assert_eq!(c.pick([(3, "]")]), Some(3));
        assert!(c.is_complete());
    }
}
