//! A small interactive selection prompt with fuzzy filtering and tab completion.
//!
//! This mirrors the look and key bindings of `inquire::Select` but additionally
//! supports completing the filter input with `Tab` up to the point where the
//! remaining options start to differ.

use std::io::{stderr, Write};

use anyhow::{anyhow, Result};
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    queue,
    style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor},
    terminal::{self, Clear, ClearType},
};
use fuzzy_matcher::{skim::SkimMatcherV2, FuzzyMatcher};

const HELP: &str = "[↑↓ to move, enter to select, tab to complete, type to filter]";

/// Puts the terminal into raw mode and restores it when dropped.
struct RawMode;
impl RawMode {
    fn enable() -> Result<Self> {
        terminal::enable_raw_mode()?;
        Ok(Self)
    }
}
impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
        let mut out = stderr();
        let _ = queue!(out, cursor::Show);
        let _ = out.flush();
    }
}

/// Interactive prompt state
struct Prompt<'a> {
    message: &'a str,
    options: &'a [String],
    input: String,
    /// indices into `options` that match the current input, ordered by score
    filtered: Vec<usize>,
    /// position of the highlighted option inside `filtered`
    cursor: usize,
    page_size: usize,
    /// number of lines drawn by the last render call
    drawn_lines: u16,
    matcher: SkimMatcherV2,
}

impl<'a> Prompt<'a> {
    fn new(message: &'a str, options: &'a [String], page_size: usize) -> Self {
        let mut p = Self {
            message,
            options,
            input: String::new(),
            filtered: vec![],
            cursor: 0,
            page_size: page_size.max(1),
            drawn_lines: 0,
            matcher: SkimMatcherV2::default(),
        };
        p.filter();
        p
    }

    /// Recompute the filtered options for the current input.
    fn filter(&mut self) {
        if self.input.is_empty() {
            self.filtered = (0..self.options.len()).collect();
        } else {
            let mut scored: Vec<(usize, i64)> = self
                .options
                .iter()
                .enumerate()
                .filter_map(|(i, opt)| {
                    self.matcher
                        .fuzzy_match(opt, &self.input)
                        .map(|score| (i, score))
                })
                .collect();
            // stable sort keeps the original order for equal scores
            scored.sort_by_key(|(_, score)| std::cmp::Reverse(*score));
            self.filtered = scored.into_iter().map(|(i, _)| i).collect();
        }
        self.cursor = 0;
    }

    /// Complete the input up to the first character that differs between the
    /// remaining options.
    ///
    /// Only options that start with the typed text are considered, options that
    /// are merely fuzzy matched are ignored. Comparison is case insensitive.
    fn complete(&mut self) {
        let input_lower = self.input.to_lowercase();
        let mut names = self
            .filtered
            .iter()
            .map(|&i| self.options[i].as_str())
            .filter(|name| name.to_lowercase().starts_with(&input_lower));
        let Some(first) = names.next() else {
            return;
        };
        let mut prefix: Vec<char> = first.chars().collect();
        for name in names {
            let common = prefix
                .iter()
                .zip(name.chars())
                .take_while(|(a, b)| a.to_lowercase().eq(b.to_lowercase()))
                .count();
            prefix.truncate(common);
            if prefix.is_empty() {
                return;
            }
        }
        let prefix: String = prefix.into_iter().collect();
        if prefix != self.input {
            self.input = prefix;
            self.filter();
        }
    }

    fn move_cursor(&mut self, delta: isize, wrap: bool) {
        let len = self.filtered.len() as isize;
        if len == 0 {
            return;
        }
        let pos = self.cursor as isize + delta;
        self.cursor = if wrap {
            pos.rem_euclid(len) as usize
        } else {
            pos.clamp(0, len - 1) as usize
        };
    }

    fn selected(&self) -> Option<&'a String> {
        self.filtered.get(self.cursor).map(|&i| &self.options[i])
    }

    /// Range of `filtered` that is currently visible.
    fn page(&self) -> std::ops::Range<usize> {
        let total = self.filtered.len();
        let size = self.page_size.min(total);
        if size == 0 {
            return 0..0;
        }
        // keep the cursor centered where possible
        let half = size / 2;
        let start = self
            .cursor
            .saturating_sub(half)
            .min(total.saturating_sub(size));
        start..start + size
    }

    fn render(&mut self) -> Result<()> {
        let mut out = stderr();
        let width = terminal::size().map(|(w, _)| w as usize).unwrap_or(80);
        self.clear(&mut out)?;
        let mut lines: u16 = 0;
        // prompt line(s), the input is appended to the last line of the message
        queue!(
            out,
            SetForegroundColor(Color::Green),
            Print("? "),
            ResetColor
        )?;
        let mut msg_lines = self.message.lines().peekable();
        while let Some(line) = msg_lines.next() {
            if msg_lines.peek().is_some() {
                queue!(out, Print(truncate(line, width)), Print("\r\n"))?;
            } else {
                queue!(
                    out,
                    Print(truncate(
                        &format!("{line} {}", self.input),
                        width.saturating_sub(2)
                    )),
                    Print("\r\n"),
                )?;
            }
            lines += 1;
        }
        // options
        for pos in self.page() {
            let name = truncate(&self.options[self.filtered[pos]], width.saturating_sub(2));
            if pos == self.cursor {
                queue!(
                    out,
                    SetForegroundColor(Color::Cyan),
                    Print("> "),
                    Print(name),
                    ResetColor,
                    Print("\r\n")
                )?;
            } else {
                queue!(out, Print("  "), Print(name), Print("\r\n"))?;
            }
            lines += 1;
        }
        // help
        queue!(
            out,
            SetAttribute(Attribute::Dim),
            Print(truncate(HELP, width)),
            SetAttribute(Attribute::Reset),
            Print("\r"),
        )?;
        lines += 1;
        out.flush()?;
        self.drawn_lines = lines;
        Ok(())
    }

    /// Remove everything drawn by the last render call.
    fn clear(&mut self, out: &mut impl Write) -> Result<()> {
        if self.drawn_lines > 1 {
            queue!(out, cursor::MoveUp(self.drawn_lines - 1))?;
        }
        queue!(
            out,
            cursor::MoveToColumn(0),
            Clear(ClearType::FromCursorDown)
        )?;
        self.drawn_lines = 0;
        Ok(())
    }

    /// Print the final line after the prompt finished.
    fn finish(&mut self, answer: Option<&str>) -> Result<()> {
        let mut out = stderr();
        self.clear(&mut out)?;
        match answer {
            Some(answer) => queue!(
                out,
                SetForegroundColor(Color::Green),
                Print("> "),
                ResetColor,
                Print(self.message),
                Print(" "),
                SetForegroundColor(Color::Cyan),
                Print(answer),
                ResetColor,
                Print("\r\n")
            )?,
            None => queue!(
                out,
                SetForegroundColor(Color::Green),
                Print("? "),
                ResetColor,
                Print(self.message),
                Print(" "),
                SetAttribute(Attribute::Dim),
                Print("<canceled>"),
                SetAttribute(Attribute::Reset),
                Print("\r\n")
            )?,
        }
        out.flush()?;
        Ok(())
    }
}

/// Cut a string to `width` characters.
fn truncate(s: &str, width: usize) -> String {
    s.chars().take(width).collect()
}

/// Show a selection prompt for `options`.
///
/// Returns `Ok(None)` if the prompt was canceled with `Esc` and an error if it
/// was interrupted with `Ctrl+C`.
pub fn select(message: &str, options: &[String], page_size: usize) -> Result<Option<String>> {
    let _raw = RawMode::enable()?;
    let mut prompt = Prompt::new(message, options, page_size);
    queue!(stderr(), cursor::Hide)?;
    loop {
        prompt.render()?;
        let Event::Key(KeyEvent {
            code,
            modifiers,
            kind,
            ..
        }) = event::read()?
        else {
            continue;
        };
        if kind == KeyEventKind::Release {
            continue;
        }
        let ctrl = modifiers.contains(KeyModifiers::CONTROL);
        match code {
            KeyCode::Enter | KeyCode::Char('j') if ctrl || code == KeyCode::Enter => {
                if let Some(answer) = prompt.selected().cloned() {
                    prompt.finish(Some(&answer))?;
                    return Ok(Some(answer));
                }
            }
            KeyCode::Esc | KeyCode::Char('g') | KeyCode::Char('d')
                if ctrl || code == KeyCode::Esc =>
            {
                prompt.finish(None)?;
                return Ok(None);
            }
            KeyCode::Char('c') if ctrl => {
                prompt.finish(None)?;
                return Err(anyhow!("operation was interrupted by the user"));
            }
            KeyCode::Tab => prompt.complete(),
            KeyCode::Up => prompt.move_cursor(-1, true),
            KeyCode::Down => prompt.move_cursor(1, true),
            KeyCode::Char('p') if ctrl => prompt.move_cursor(-1, true),
            KeyCode::Char('n') if ctrl => prompt.move_cursor(1, true),
            KeyCode::PageUp => prompt.move_cursor(-(prompt.page_size as isize), false),
            KeyCode::PageDown => prompt.move_cursor(prompt.page_size as isize, false),
            KeyCode::Home => prompt.move_cursor(isize::MIN / 2, false),
            KeyCode::End => prompt.move_cursor(isize::MAX / 2, false),
            KeyCode::Backspace if ctrl => {
                delete_word(&mut prompt.input);
                prompt.filter();
            }
            KeyCode::Backspace => {
                if prompt.input.pop().is_some() {
                    prompt.filter();
                }
            }
            KeyCode::Char('w') if ctrl => {
                delete_word(&mut prompt.input);
                prompt.filter();
            }
            KeyCode::Char('u') if ctrl => {
                prompt.input.clear();
                prompt.filter();
            }
            KeyCode::Char(c) if !ctrl => {
                prompt.input.push(c);
                prompt.filter();
            }
            _ => (),
        }
    }
}

/// Ask for a line of text input.
pub fn text(message: &str) -> Result<String> {
    text_with_validator(message, |_| Ok(()))
}

/// Ask for a line of text input until `validate` accepts it.
///
/// Returns an error if the input is aborted (Esc or Ctrl+C) or stdin is closed.
pub fn text_with_validator(
    message: &str,
    validate: impl Fn(&str) -> std::result::Result<(), String>,
) -> Result<String> {
    let mut out = stderr();
    loop {
        queue!(
            out,
            SetForegroundColor(Color::Green),
            Print("? "),
            ResetColor,
            Print(message),
            Print(" ")
        )?;
        out.flush()?;
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line)? == 0 {
            return Err(anyhow!("input aborted"));
        }
        let line = line.trim_end_matches(['\r', '\n']).to_string();
        // redraw the line as answered
        queue!(
            out,
            cursor::MoveUp(1),
            cursor::MoveToColumn(0),
            Clear(ClearType::FromCursorDown),
        )?;
        match validate(&line) {
            Ok(()) => {
                queue!(
                    out,
                    SetForegroundColor(Color::Green),
                    Print("> "),
                    ResetColor,
                    Print(message),
                    Print(" "),
                    SetForegroundColor(Color::Cyan),
                    Print(&line),
                    ResetColor,
                    Print("\r\n")
                )?;
                out.flush()?;
                return Ok(line);
            }
            Err(err) => {
                queue!(
                    out,
                    SetForegroundColor(Color::Red),
                    Print("# "),
                    Print(&err),
                    ResetColor,
                    Print("\r\n")
                )?;
            }
        }
    }
}

/// Delete the last word (and trailing whitespace) from `input`.
fn delete_word(input: &mut String) {
    let trimmed = input.trim_end().len();
    input.truncate(trimmed);
    let cut = input
        .rfind(|c: char| c.is_whitespace() || c == '-' || c == '_' || c == '/')
        .map(|i| i + 1)
        .unwrap_or(0);
    if cut == input.len() && !input.is_empty() {
        // cursor sits right after a separator: remove the separator itself
        input.pop();
    } else {
        input.truncate(cut);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn completes_common_prefix() {
        let options = opts(&["backend-a", "backend-b", "other"]);
        let mut p = Prompt::new("m", &options, 10);
        p.input = "back".into();
        p.filter();
        p.complete();
        assert_eq!(p.input, "backend-");
        assert_eq!(p.filtered.len(), 2);
    }

    #[test]
    fn completes_single_option_fully() {
        let options = opts(&["backend-a", "backend-b"]);
        let mut p = Prompt::new("m", &options, 10);
        p.input = "backend-a".into();
        p.filter();
        p.complete();
        assert_eq!(p.input, "backend-a");
    }

    #[test]
    fn ignores_fuzzy_only_matches() {
        // "fer" fuzzy matches "some-ferry" but that does not start with "fer"
        let options = opts(&["ferrari", "ferris", "some-ferry"]);
        let mut p = Prompt::new("m", &options, 10);
        p.input = "fer".into();
        p.filter();
        assert_eq!(p.filtered.len(), 3);
        p.complete();
        assert_eq!(p.input, "ferr");
    }

    #[test]
    fn no_completion_if_nothing_starts_with_input() {
        let options = opts(&["backend-a", "backend-b"]);
        let mut p = Prompt::new("m", &options, 10);
        p.input = "end".into();
        p.filter();
        assert_eq!(p.filtered.len(), 2);
        p.complete();
        assert_eq!(p.input, "end");
    }

    #[test]
    fn completion_is_case_insensitive() {
        let options = opts(&["Backend-a", "backend-b"]);
        let mut p = Prompt::new("m", &options, 10);
        p.input = "bac".into();
        p.filter();
        p.complete();
        assert_eq!(p.input.to_lowercase(), "backend-");
    }

    #[test]
    fn only_prefix_matches_are_completed() {
        // "a" fuzzy matches "beta" as well, but only "alpha" starts with it
        let options = opts(&["alpha", "beta"]);
        let mut p = Prompt::new("m", &options, 10);
        p.input = "a".into();
        p.filter();
        assert_eq!(p.filtered.len(), 2);
        p.complete();
        assert_eq!(p.input, "alpha");
    }

    #[test]
    fn delete_word_works() {
        let mut s = String::from("backend-a");
        delete_word(&mut s);
        assert_eq!(s, "backend-");
        delete_word(&mut s);
        assert_eq!(s, "backend");
        delete_word(&mut s);
        assert_eq!(s, "");
    }
}
