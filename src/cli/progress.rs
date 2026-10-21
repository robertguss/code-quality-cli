//! Progress for a person watching a hook run: what is being checked, which
//! tools are running and how long each took. Written to stderr, and only when
//! stderr is a terminal and the format is text, so agents, CI and
//! `--format json` get the report alone. `QGATE_PROGRESS=1` forces it on and
//! `QGATE_PROGRESS=0` off.

use std::fmt::Display;
use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::Format;

static ENABLED: AtomicBool = AtomicBool::new(false);
/// stderr is a terminal, so a status line can be redrawn in place.
static LIVE: AtomicBool = AtomicBool::new(false);
static COLOR: AtomicBool = AtomicBool::new(false);
static STATE: Mutex<State> = Mutex::new(State {
    running: Vec::new(),
    drawn: false,
});

/// Longest status line drawn, so it never wraps and breaks the redraw.
const STATUS_CHARS: usize = 72;

struct State {
    /// Tools running now, with when each started.
    running: Vec<(String, Instant)>,
    /// A status line is on screen and must be cleared before the next line.
    drawn: bool,
}

pub fn init(format: Format) {
    let tty = std::io::stderr().is_terminal();
    let on = match std::env::var("QGATE_PROGRESS").as_deref() {
        Ok("1") => true,
        Ok("0") => false,
        _ => format == Format::Text && tty,
    };
    ENABLED.store(on, Ordering::Relaxed);
    LIVE.store(on && tty, Ordering::Relaxed);
    COLOR.store(
        on && tty && std::env::var_os("NO_COLOR").is_none(),
        Ordering::Relaxed,
    );
}

fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Print one line, above the status line when there is one.
pub fn line(msg: impl Display) {
    if !enabled() {
        return;
    }
    let mut s = state();
    let mut err = std::io::stderr().lock();
    clear(&mut s, &mut err);
    let _ = writeln!(err, "qgate: {msg}");
    draw(&mut s, &mut err);
}

/// A step has started; it shows on the status line until [`done`].
pub fn started(name: &str) {
    if enabled() {
        state().running.push((name.to_string(), Instant::now()));
    }
}

/// A step has finished. `problem` says why it failed, such as `exit 1`.
pub fn done(name: &str, problem: Option<String>, elapsed: Duration) {
    if !enabled() {
        return;
    }
    state().running.retain(|(n, _)| n != name);
    line(done_text(
        name,
        problem.as_deref(),
        elapsed,
        COLOR.load(Ordering::Relaxed),
    ));
}

fn done_text(name: &str, problem: Option<&str>, elapsed: Duration, color: bool) -> String {
    let (mark, paint) = match problem {
        None => ("ok", "32"),
        Some(_) => ("FAIL", "31"),
    };
    let mark = match color {
        true => format!("\x1b[{paint}m{mark}\x1b[0m"),
        false => mark.to_string(),
    };
    let why = problem.map(|p| format!(" ({p})")).unwrap_or_default();
    format!("  {mark} {name} {}{why}", secs(elapsed))
}

fn secs(d: Duration) -> String {
    format!("{:.1}s", d.as_secs_f64())
}

/// Redraws the status line every quarter second on a terminal until dropped,
/// then clears it.
pub struct StatusLine {
    stop: Arc<AtomicBool>,
    ticker: Option<thread::JoinHandle<()>>,
}

pub fn status_line() -> StatusLine {
    let stop = Arc::new(AtomicBool::new(false));
    let ticker = LIVE.load(Ordering::Relaxed).then(|| {
        let stop = Arc::clone(&stop);
        thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                {
                    let mut s = state();
                    let mut err = std::io::stderr().lock();
                    clear(&mut s, &mut err);
                    draw(&mut s, &mut err);
                }
                thread::sleep(Duration::from_millis(250));
            }
        })
    });
    StatusLine { stop, ticker }
}

impl Drop for StatusLine {
    fn drop(&mut self) {
        let Some(ticker) = self.ticker.take() else {
            return;
        };
        self.stop.store(true, Ordering::Relaxed);
        let _ = ticker.join();
        clear(&mut state(), &mut std::io::stderr().lock());
    }
}

fn clear(s: &mut State, err: &mut impl Write) {
    if s.drawn {
        let _ = write!(err, "\r\x1b[2K");
        let _ = err.flush();
        s.drawn = false;
    }
}

fn draw(s: &mut State, err: &mut impl Write) {
    if !LIVE.load(Ordering::Relaxed) || s.running.is_empty() {
        return;
    }
    let _ = write!(err, "{}", status_text(&s.running, Instant::now()));
    let _ = err.flush();
    s.drawn = true;
}

/// `  running tests 41s, clippy 12s`, longest-running first, cut to fit.
fn status_text(running: &[(String, Instant)], now: Instant) -> String {
    let mut by_age: Vec<_> = running.iter().collect();
    by_age.sort_by_key(|(_, start)| *start);
    let parts: Vec<String> = by_age
        .iter()
        .map(|(name, start)| format!("{name} {}s", now.duration_since(*start).as_secs()))
        .collect();
    let text = format!("  running {}", parts.join(", "));
    match text.char_indices().nth(STATUS_CHARS) {
        Some((i, _)) => format!("{}...", &text[..i]),
        None => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn done_lines_show_result_time_and_reason() {
        let d = Duration::from_millis(41_230);
        assert_eq!(done_text("tests", None, d, false), "  ok tests 41.2s");
        assert_eq!(
            done_text("tests", Some("exit 1"), d, false),
            "  FAIL tests 41.2s (exit 1)"
        );
        assert!(done_text("tests", None, d, true).contains("\x1b[32mok\x1b[0m"));
    }

    #[test]
    fn status_lists_longest_running_first_and_fits_one_line() {
        let now = Instant::now();
        let running = vec![
            ("clippy".to_string(), now - Duration::from_secs(12)),
            ("tests".to_string(), now - Duration::from_secs(41)),
        ];
        assert_eq!(
            status_text(&running, now),
            "  running tests 41s, clippy 12s"
        );
        let many: Vec<_> = (0..20).map(|i| (format!("tool-{i}"), now)).collect();
        assert!(status_text(&many, now).chars().count() <= STATUS_CHARS + 3);
    }
}
