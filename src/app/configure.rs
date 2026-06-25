//! Configure Quill by describing the change to the AI (#303): the author types a
//! plain-language request ("a warm dark theme", "turn on Vim mode", "narrow the
//! column"), the AI maps it onto concrete settings from a closed vocabulary, and
//! the proposal is validated and confirmed before anything is written. The
//! generalization of describe-a-workflow (#232): natural language in, a structured
//! proposal out, a confirm step before any write. The only write targets are the
//! config file's `[editor]` / `[theme]` sections — never manuscript prose.

use super::App;
use crate::ai::config_gen::{self, GeneratedConfig};
use crate::ai::Outcome;
use crate::config;
use crate::mode::Pending;
use crate::theme;

impl App {
    /// Begin a describe-to-config request (#303): only when AI is ready, then open
    /// the description prompt. Sends no manuscript, so there's no cost confirm.
    pub(crate) fn start_describe_config(&mut self) {
        match self.ai.check_ready() {
            Ok(_) => self.open_prompt(Pending::DescribeConfig, "Describe a settings change: ", "", None),
            Err(e) => self.message = Some(e.to_string()),
        }
    }

    /// Send the settings vocabulary plus the author's description to the model
    /// (no manuscript — cheap); the proposal is applied via [`poll_ai`](App::poll_ai).
    pub(crate) fn generate_config(&mut self, description: &str) {
        let description = description.trim().to_string();
        if description.is_empty() {
            return;
        }
        let vocab = settings_vocab();
        let model = self.ai.model.clone();
        self.spawn_outcome("⟳ Proposing settings…", move |backend| {
            let req = config_gen::build_request(&description, &vocab, &model);
            Outcome::Config(backend.complete(&req).and_then(|r| config_gen::parse(&r.text)))
        });
    }

    /// Handle a generated-config reply (#303): validate each proposed change
    /// against the real settings, report what couldn't be mapped or parsed, then
    /// stash the valid changes and open the y/N confirm — nothing is written until
    /// the author confirms.
    pub(crate) fn apply_generated_config(&mut self, result: Result<GeneratedConfig, crate::ai::AiError>) {
        let Some(gen) = self.ok_or_report(result) else {
            return;
        };
        // Each valid change becomes `(dotted_key, toml literal)`, ready to write.
        let mut valid: Vec<(String, String)> = Vec::new();
        let mut rejected: Vec<String> = gen.unmapped.clone();
        for change in &gen.changes {
            match validate_change(&change.key, &change.value) {
                Ok(literal) => valid.push((change.key.clone(), literal)),
                Err(why) => rejected.push(why),
            }
        }
        let note = if rejected.is_empty() {
            String::new()
        } else {
            format!("(couldn't map: {}) ", rejected.join(", "))
        };
        if valid.is_empty() {
            self.message = Some(if note.is_empty() {
                "Couldn't map that to any setting".into()
            } else {
                note.trim().to_string()
            });
            return;
        }
        let summary: Vec<String> = valid.iter().map(|(k, v)| format!("{k} = {v}")).collect();
        let label = format!("{note}Apply {}? (y/N) — {}", count(valid.len()), summary.join(", "));
        self.pending_config = Some(valid);
        self.open_prompt(Pending::ConfirmConfig, label, "", None);
    }

    /// Write the confirmed config changes (#303) to the config file, preserving
    /// the rest of it, then apply them live so the change is visible immediately.
    pub(crate) fn write_config_changes(&mut self) {
        let Some(changes) = self.pending_config.take() else {
            return;
        };
        // `changes` already holds `(dotted_key, toml literal)`; split the key into
        // `(section, field)` for the writer.
        let writes: Vec<(String, String, String)> = changes
            .iter()
            .filter_map(|(key, literal)| {
                let (section, field) = key.split_once('.')?;
                Some((section.to_string(), field.to_string(), literal.clone()))
            })
            .collect();
        match config::set_values(&writes) {
            Ok(path) => self.message = Some(format!("Applied {} to {}", count(writes.len()), path.display())),
            Err(e) => {
                self.message = Some(format!("Couldn't write config: {e}"));
                return;
            }
        }
        self.apply_config_live();
    }

    /// Re-read the just-written config and apply it to the running editor (#303),
    /// so a settings change takes effect without a restart.
    fn apply_config_live(&mut self) {
        let editor = config::EditorConfig::load();
        self.vim = editor.vim;
        self.smart_typography = editor.smart_typography;
        self.focus_width = editor.column_width;
        let (theme, _) = config::load_theme();
        self.theme = theme;
        theme::set_current(theme);
        // The resting editor mode follows whether Vim mode is now on; the confirm
        // prompt's caller closes to it after this returns.
    }
}

/// The settings the AI may propose (#303): a closed vocabulary of real keys, each
/// with a human description (with allowed values) for the model to map against.
/// The keys are dotted `section.field`, matching the config file's layout.
fn settings_vocab() -> Vec<(String, String)> {
    let mut v = vec![
        ("editor.vim".to_string(), "Vim-style modal editing — true or false".to_string()),
        (
            "editor.smart_typography".to_string(),
            "Smart curly quotes, em-dashes, and ellipses as you type — true or false".to_string(),
        ),
        (
            "editor.column_width".to_string(),
            "Width of the centered text column, in characters (32–160; e.g. 64 is narrow, 90 is wide)".to_string(),
        ),
        ("theme.name".to_string(), "Base color theme — dark or light".to_string()),
    ];
    for field in theme::COLOR_FIELDS {
        v.push((
            format!("theme.{field}"),
            format!("Color of {} — an ANSI name, #hex, r,g,b, or 0–255 index", color_role(field)),
        ));
    }
    v
}

/// A short human description of what a theme color field controls, for the vocab.
fn color_role(field: &str) -> &'static str {
    match field {
        "heading" => "headings and accents",
        "on_accent" => "text on an accent/match background",
        "dim" => "secondary text and chrome (hints, dividers)",
        "muted" => "muted prose (epigraphs)",
        "match_current" => "the current search match background",
        "match_other" => "other search matches' background",
        "status_bg" => "the status bar background",
        "added" => "additions (diff +, gains)",
        "removed" => "removals (diff -, losses)",
        "revised" => "the 'revised' chapter status",
        "mode_insert" => "the Insert-mode indicator",
        "mode_normal" => "the Normal-mode indicator",
        "mode_visual" => "the Visual-mode indicator",
        _ => "a theme element",
    }
}

/// Validate a proposed `(key, value)` against the real settings and, on success,
/// return the TOML scalar literal to write for it. `Err` carries a short
/// "key (why)" phrase for the report. The key is already schema-constrained to
/// the vocabulary, so this only checks the value. theme.name and the colors are
/// quoted via the toml serializer so escaping always matches what the loader parses.
fn validate_change(key: &str, value: &str) -> Result<String, String> {
    let bad = |why: &str| Err(format!("{key} ({why})"));
    let quote = |s: &str| toml::Value::String(s.to_string()).to_string();
    match key {
        "editor.vim" | "editor.smart_typography" => match parse_bool(value) {
            Some(b) => Ok(b.to_string()),
            None => bad("expected true/false"),
        },
        "editor.column_width" => match value.trim().parse::<u16>() {
            Ok(w) if (32..=160).contains(&w) => Ok(w.to_string()),
            _ => bad("expected a width 32–160"),
        },
        "theme.name" => match value.trim().to_ascii_lowercase().as_str() {
            n @ ("dark" | "light") => Ok(quote(n)),
            _ => bad("expected dark or light"),
        },
        _ if key.starts_with("theme.") => {
            if theme::is_color(value) {
                Ok(quote(value.trim()))
            } else {
                bad("not a valid color")
            }
        }
        _ => bad("unknown setting"),
    }
}

/// Parse a permissive boolean (`true/false`, `on/off`, `yes/no`, `1/0`).
fn parse_bool(s: &str) -> Option<bool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "true" | "on" | "yes" | "1" => Some(true),
        "false" | "off" | "no" | "0" => Some(false),
        _ => None,
    }
}

/// "1 setting" / "N settings".
fn count(n: usize) -> String {
    if n == 1 {
        "1 setting".to_string()
    } else {
        format!("{n} settings")
    }
}
