//! The terminal rendering of AI results: a Q&A answer as overlay lines. Kept
//! apart from the rest of `ai`, which is data and requests only, because this is
//! the one piece that draws with `ratatui` (#1229).

use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::theme;

use super::{Answer, Usage};

/// Render a Q&A answer's prose as overlay lines: the answer, then a dimmed usage
/// line. Long lines are wrapped by the renderer (the overlay enables wrapping).
/// Citations are surfaced separately as jumpable entries (#609).
pub fn answer_to_lines(answer: &Answer, usage: &Usage) -> Vec<Line<'static>> {
    let mut out: Vec<Line> = answer
        .answer
        .lines()
        .map(|l| Line::raw(l.to_string()))
        .collect();
    if out.is_empty() {
        out.push(Line::raw("(no answer)"));
    }
    out.push(Line::raw(""));
    out.push(Line::from(Span::styled(
        format!(
            "{} tokens in ({} cached) · {} out",
            usage.input_tokens, usage.cache_read_tokens, usage.output_tokens
        ),
        Style::default().fg(theme::current().dim),
    )));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answer_to_lines_renders_answer_and_usage() {
        let answer = Answer {
            answer: "Chapter One introduces the locket.".into(),
            citations: vec!["One".into()],
            ..Answer::default()
        };
        let usage = Usage {
            input_tokens: 1200,
            output_tokens: 30,
            cache_read_tokens: 1000,
        };
        let lines = answer_to_lines(&answer, &usage);
        let text: String = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("introduces the locket"));
        assert!(text.contains("1,200 tokens in") || text.contains("1200 tokens in"));
    }
}
