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
pub struct Sprint {
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
    pub(crate) const fn words(&self, current_words: usize) -> i64 {
        current_words as i64 - self.baseline_words as i64
    }
}

/// An optional per-session word target (#1011): a goal to write toward this
/// session with a live indicator that fills as words are added. Like a sprint it
/// baselines the word count at the start and diffs the live total, but it carries
/// a target rather than a clock — the author writes to a word count, not against
/// time. Session state, never persisted.
pub struct SessionTarget {
    baseline_words: usize,
    target: usize,
    /// Whether the target has already been reached this session, so it's signaled
    /// once (non-disruptive) rather than every tick after.
    reached: bool,
}

impl SessionTarget {
    /// Begin a session target of `target` words, baselining the current count.
    const fn new(target: usize, baseline_words: usize) -> Self {
        Self {
            baseline_words,
            target,
            reached: false,
        }
    }

    /// The chosen target — how many words to write this session.
    pub(crate) const fn target(&self) -> usize {
        self.target
    }

    /// Net words written toward the target since it was set (never negative — a
    /// cut below the baseline just shows no progress, not a filled-backwards bar).
    pub(crate) const fn written(&self, current_words: usize) -> usize {
        current_words.saturating_sub(self.baseline_words)
    }

    /// Whether the live count has met (or passed) the target.
    pub(crate) const fn is_met(&self, current_words: usize) -> bool {
        self.written(current_words) >= self.target
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

    /// Set a per-session word target (#1011): ask how many words to write.
    pub(crate) fn set_session_target(&mut self) {
        self.open_prompt(
            Pending::SessionTarget,
            "Session word target (blank clears):",
            "500",
            None,
        );
    }

    /// Begin (or clear) the session target once the author has chosen a count. A
    /// blank entry clears any running target; a zero or unparseable entry is
    /// rejected. Baselines against the current word count so the indicator starts
    /// empty and fills as words are written.
    pub(crate) fn begin_session_target(&mut self, target: &str) {
        self.mode = self.editing_mode();
        let target = target.trim();
        if target.is_empty() {
            self.session_target = None;
            self.message = Some("Session target cleared.".into());
            return;
        }
        let Ok(target) = target.parse::<usize>() else {
            self.message = Some("Enter the session target in whole words.".into());
            return;
        };
        if target == 0 {
            self.message = Some("A session target needs at least one word.".into());
            return;
        }
        self.session_target = Some(SessionTarget::new(target, self.project_word_count()));
        self.message = Some(format!("Session target set — {target} words. Write!"));
    }

    /// Signal reaching the session target (#1011), non-disruptively: driven from
    /// the event loop like [`Self::tick_sprint`], it posts a one-time status
    /// message the first tick the live count meets the target, then leaves the
    /// target in place (the status-bar indicator stays filled). Cheap when no
    /// target is set. The live indicator itself is read fresh by the status bar
    /// each frame from cached counts, so this only handles the crossing.
    pub(crate) fn tick_session_target(&mut self) {
        // Count only while a target is waiting to be met: the count resolves the
        // active chapter (an O(chapters) lookup), and this runs every loop tick.
        if !self.session_target.as_ref().is_some_and(|t| !t.reached) {
            return;
        }
        let current = self.project_word_count();
        if let Some(t) = self.session_target.as_mut() {
            if t.is_met(current) {
                t.reached = true;
                let target = t.target;
                self.message = Some(format!("Session target reached — {target} words ✓"));
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
pub fn status_segment(remaining: Duration, words: i64) -> String {
    let secs = remaining.as_secs();
    let (mins, rem) = (secs / 60, secs % 60);
    let sign = if words < 0 { "" } else { "+" };
    format!("⏱ {mins}:{rem:02} · {sign}{words}w")
}

/// A compact 8-cell fill bar (#1011) for a live goal indicator: the fraction
/// `done / total` rounded to eighths, e.g. `▓▓▓░░░░░` at ~40%. A met goal is all
/// full blocks. `total` of zero (no goal) yields an empty bar. Pure string work,
/// no allocation beyond the returned bar — safe to build every frame.
fn fill_bar(done: usize, total: usize) -> String {
    const CELLS: usize = 8;
    if total == 0 {
        return "░".repeat(CELLS);
    }
    // Round to the nearest eighth so the bar reflects progress smoothly, capped at
    // full once the goal is met.
    let filled = ((done * CELLS) / total).min(CELLS);
    format!("{}{}", "▓".repeat(filled), "░".repeat(CELLS - filled))
}

/// The status-bar segment for the active chapter's stored word goal (#1011): a
/// live fill bar and the count toward the goal, e.g. "◆ ▓▓▓░░░░░ 620/1500". A met
/// goal ends in a ✓ (the same signal the chapter browser uses). `words` is the
/// live active-chapter count; `goal` its cached front-matter goal — no file read.
pub fn chapter_goal_segment(words: usize, goal: usize) -> String {
    let met = if words >= goal { " ✓" } else { "" };
    format!("◆ {} {words}/{goal}{met}", fill_bar(words, goal))
}

/// The status-bar segment for the per-session word target (#1011): a live fill bar
/// and the words written toward it, e.g. "◇ ▓▓▓▓░░░░ 240/500". A met target ends in
/// a ✓. `written` is the net words since the target was set (from cached counts);
/// `target` the chosen goal — no file read.
pub fn session_target_segment(written: usize, target: usize) -> String {
    let met = if written >= target { " ✓" } else { "" };
    format!("◇ {} {written}/{target}{met}", fill_bar(written, target))
}

/// A sprint that's already over (started two minutes ago, one-minute length), so
/// the event-loop tick (#299) can be tested without waiting on the clock.
#[cfg(test)]
pub fn test_finished_sprint(baseline_words: usize) -> Sprint {
    Sprint {
        start: Instant::now()
            .checked_sub(Duration::from_secs(120))
            .unwrap(),
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
            start: Instant::now().checked_sub(Duration::from_secs(60)).unwrap(),
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

    #[test]
    fn the_fill_bar_reflects_the_fraction_in_eighths() {
        assert_eq!(fill_bar(0, 8), "░░░░░░░░", "empty at the start");
        assert_eq!(fill_bar(4, 8), "▓▓▓▓░░░░", "half fills half the cells");
        assert_eq!(fill_bar(8, 8), "▓▓▓▓▓▓▓▓", "a met goal is full");
        assert_eq!(fill_bar(12, 8), "▓▓▓▓▓▓▓▓", "over the goal stays full");
        assert_eq!(fill_bar(5, 0), "░░░░░░░░", "no goal is an empty bar");
    }

    #[test]
    fn a_session_target_written_is_the_baseline_delta_floored_at_zero() {
        let t = SessionTarget::new(500, 1000);
        assert_eq!(t.written(1240), 240, "added words fill toward the target");
        assert_eq!(t.written(1000), 0, "no writing is no progress");
        assert_eq!(
            t.written(950),
            0,
            "a cut below the baseline doesn't go negative"
        );
    }

    #[test]
    fn a_session_target_is_met_once_the_delta_reaches_it() {
        let t = SessionTarget::new(500, 1000);
        assert!(!t.is_met(1499), "one short is not met");
        assert!(t.is_met(1500), "exactly the target is met");
        assert!(t.is_met(1600), "past the target stays met");
    }

    #[test]
    fn the_chapter_goal_segment_fills_and_marks_when_reached() {
        let below = chapter_goal_segment(620, 1500);
        assert!(below.starts_with("◆ "), "{below}");
        assert!(below.contains("620/1500"), "{below}");
        assert!(!below.contains('✓'), "not marked below the goal: {below}");
        let met = chapter_goal_segment(1500, 1500);
        assert!(
            met.contains("1500/1500 ✓"),
            "a reached goal is marked: {met}"
        );
        assert!(
            met.contains("▓▓▓▓▓▓▓▓"),
            "a reached goal fills the bar: {met}"
        );
    }

    #[test]
    fn the_session_target_segment_fills_and_marks_when_reached() {
        let below = session_target_segment(240, 500);
        assert!(below.starts_with("◇ "), "{below}");
        assert!(below.contains("240/500"), "{below}");
        assert!(!below.contains('✓'), "not marked below the target: {below}");
        let met = session_target_segment(500, 500);
        assert!(
            met.contains("500/500 ✓"),
            "a reached target is marked: {met}"
        );
    }
}
