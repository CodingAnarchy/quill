//! Writing sprints (#299): a timed session with a live countdown and a running
//! count of words written since it began — the word-war / Pomodoro ritual a lot
//! of writers build momentum with.
//!
//! A sprint is a thin layer over machinery Quill already has: the word count
//! ([`App::project_word_count`]) gives the baseline at start and the live total
//! each frame, so "words written" is just their difference. Elapsed time is a
//! monotonic [`Instant`] (immune to wall-clock changes), ticked from the event
//! loop. It's session state — never persisted — and shows in the status bar, so
//! it never blocks typing. Start/stop are registry actions, so a sprint can be a
//! workflow step or bound to a key.

use std::time::{Duration, Instant};

use super::*;

/// An in-progress writing sprint: when it began, how long it runs, and the word
/// count at the start to diff against.
pub(crate) struct Sprint {
    start: Instant,
    duration: Duration,
    baseline_words: usize,
}

impl Sprint {
    /// Begin a sprint of `minutes`, baselining the current word count.
    fn new(minutes: u64, baseline_words: usize) -> Self {
        Self {
            start: Instant::now(),
            duration: Duration::from_secs(minutes * 60),
            baseline_words,
        }
    }

    /// Time left before the sprint ends (zero once it's over).
    pub(crate) fn remaining(&self) -> Duration {
        self.duration.saturating_sub(self.start.elapsed())
    }

    /// Time spent so far.
    fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }

    /// Whether the sprint's time is up.
    fn finished(&self) -> bool {
        self.start.elapsed() >= self.duration
    }

    /// Net words written since the start (negative if the author cut more than
    /// they added) — the same total the status bar and report count, diffed.
    pub(crate) fn words(&self, current_words: usize) -> i64 {
        current_words as i64 - self.baseline_words as i64
    }
}

impl App {
    /// Begin a writing sprint (#299): ask how many minutes to run.
    pub(crate) fn start_sprint(&mut self) {
        self.open_prompt(
            Pending::SprintDuration,
            "Writing sprint — minutes:",
            "25",
            None,
        );
    }

    /// Start the sprint once the author has chosen a length. A blank or unparseable
    /// entry (or zero) is rejected; an absurd length is clamped to a day.
    pub(crate) fn begin_sprint(&mut self, minutes: &str) {
        self.mode = self.editing_mode();
        let Ok(minutes) = minutes.trim().parse::<u64>() else {
            self.message = Some("Enter the sprint length in whole minutes.".into());
            return;
        };
        if minutes == 0 {
            self.message = Some("A sprint needs at least a minute.".into());
            return;
        }
        let minutes = minutes.min(24 * 60);
        self.sprint = Some(Sprint::new(minutes, self.project_word_count()));
        self.message = Some(format!("Sprint started — {minutes} min. Write!"));
    }

    /// Stop the sprint early and report the result.
    pub(crate) fn stop_sprint(&mut self) {
        match self.sprint.take() {
            Some(s) => {
                let words = s.words(self.project_word_count());
                self.message = Some(sprint_result("Sprint stopped", words, s.elapsed()));
            }
            None => self.message = Some("No sprint is running.".into()),
        }
    }

    /// Drive the countdown from the event loop: when the time is up, end the sprint
    /// and report the result. Cheap when no sprint is running. The live countdown
    /// and word count are read fresh by the status bar each frame, so this only
    /// handles the finish.
    pub(crate) fn tick_sprint(&mut self) {
        if self.sprint.as_ref().is_some_and(Sprint::finished) {
            if let Some(s) = self.sprint.take() {
                let words = s.words(self.project_word_count());
                self.message = Some(sprint_result("Sprint over", words, s.duration));
            }
        }
    }
}

/// The end-of-sprint summary: words written, elapsed time, and words-per-minute
/// when the author came out ahead.
fn sprint_result(prefix: &str, words: i64, elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    let (mins, rem) = (secs / 60, secs % 60);
    if words > 0 {
        let wpm = f64::from(words as u32) * 60.0 / secs.max(1) as f64;
        format!("{prefix}: +{words} words in {mins}m {rem:02}s ({wpm:.0} wpm)")
    } else {
        format!("{prefix}: {words} words in {mins}m {rem:02}s")
    }
}

/// The status-bar segment for a running sprint (#299): the countdown and the live
/// net word count, e.g. "⏱ 12:34 · +312w".
pub(crate) fn status_segment(remaining: Duration, words: i64) -> String {
    let secs = remaining.as_secs();
    let (mins, rem) = (secs / 60, secs % 60);
    let sign = if words < 0 { "" } else { "+" };
    format!("⏱ {mins}:{rem:02} · {sign}{words}w")
}

/// A sprint that's already over (started two minutes ago, one-minute length), so
/// the event-loop tick (#299) can be tested without waiting on the clock.
#[cfg(test)]
pub(crate) fn test_finished_sprint(baseline_words: usize) -> Sprint {
    Sprint {
        start: Instant::now() - Duration::from_secs(120),
        duration: Duration::from_secs(60),
        baseline_words,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_written_is_the_net_delta_from_the_baseline() {
        let s = Sprint::new(25, 1000);
        assert_eq!(s.words(1312), 312, "added words count up");
        assert_eq!(s.words(950), -50, "cuts go negative");
        assert_eq!(s.words(1000), 0);
    }

    #[test]
    fn a_back_dated_start_is_finished_and_has_no_time_left() {
        // Start a 30s sprint a full minute ago: it's over.
        let s = Sprint {
            start: Instant::now() - Duration::from_secs(60),
            duration: Duration::from_secs(30),
            baseline_words: 0,
        };
        assert!(s.finished());
        assert_eq!(s.remaining(), Duration::ZERO);
    }

    #[test]
    fn a_fresh_sprint_is_running_with_time_left() {
        let s = Sprint::new(25, 0);
        assert!(!s.finished());
        assert!(
            s.remaining() > Duration::from_secs(24 * 60),
            "≈25 min left just after starting"
        );
        assert!(s.remaining() <= Duration::from_secs(25 * 60));
    }

    #[test]
    fn the_result_shows_words_time_and_wpm_when_ahead() {
        let r = sprint_result("Sprint over", 300, Duration::from_secs(600)); // 10 min
        assert!(r.contains("+300 words"), "{r}");
        assert!(r.contains("10m 00s"), "{r}");
        assert!(r.contains("30 wpm"), "{r}"); // 300 words / 10 min
    }

    #[test]
    fn the_result_omits_wpm_when_no_words_were_gained() {
        let r = sprint_result("Sprint stopped", -12, Duration::from_secs(125));
        assert!(r.contains("-12 words"), "{r}");
        assert!(r.contains("2m 05s"), "{r}");
        assert!(!r.contains("wpm"), "no rate when behind: {r}");
    }

    #[test]
    fn the_status_segment_is_compact() {
        assert_eq!(
            status_segment(Duration::from_secs(754), 312),
            "⏱ 12:34 · +312w"
        );
        assert_eq!(status_segment(Duration::from_secs(5), -7), "⏱ 0:05 · -7w");
    }
}
