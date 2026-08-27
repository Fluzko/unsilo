//! How output is dressed, and when it is not.
//!
//! One rule governs everything here: **colour encodes meaning that is already in
//! the text.** Strip it and nothing is lost. That keeps the output usable in a
//! pipe, in a log file, and for a reader who cannot tell red from green.
//!
//! Which is also why it turns itself off. Colour that leaks into a file someone
//! is parsing is worse than no colour at all, so the decision is made once, from
//! whether stdout is a terminal, and honoured everywhere.

use anstyle::{AnsiColor, Color, Style as Ansi};

/// The width of a section rule. Fixed rather than measured: a predictable line is
/// worth more than one that reflows when the window changes, and nothing here
/// depends on fitting content inside a box.
const RULE_WIDTH: usize = 74;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColorChoice {
    #[default]
    Auto,
    Always,
    Never,
}

impl ColorChoice {
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "auto" => Some(ColorChoice::Auto),
            "always" => Some(ColorChoice::Always),
            "never" => Some(ColorChoice::Never),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Style {
    color: bool,
    unicode: bool,
}

impl Style {
    /// Plain text, for anything that will be parsed rather than read.
    #[must_use]
    pub fn plain() -> Self {
        Self { color: false, unicode: false }
    }

    /// `NO_COLOR` is honoured whatever its value, per the convention: its
    /// presence is the signal.
    #[must_use]
    pub fn resolve(
        choice: ColorChoice,
        no_color_set: bool,
        is_terminal: bool,
        unicode: bool,
    ) -> Self {
        let color = match choice {
            ColorChoice::Never => false,
            ColorChoice::Always => true,
            ColorChoice::Auto => is_terminal && !no_color_set,
        };
        Self { color, unicode }
    }

    #[must_use]
    pub fn with_unicode(mut self, unicode: bool) -> Self {
        self.unicode = unicode;
        self
    }

    #[must_use]
    pub fn is_colored(self) -> bool {
        self.color
    }

    fn paint(self, style: Ansi, text: &str) -> String {
        if !self.color {
            return text.to_owned();
        }
        format!("{}{text}{}", style.render(), style.render_reset())
    }

    fn fg(color: AnsiColor) -> Ansi {
        Ansi::new().fg_color(Some(Color::Ansi(color)))
    }

    /// Something was added, or is allowed.
    #[must_use]
    pub fn ok(self, text: &str) -> String {
        self.paint(Self::fg(AnsiColor::Green), text)
    }

    /// Something was removed, or is refused.
    #[must_use]
    pub fn bad(self, text: &str) -> String {
        self.paint(Self::fg(AnsiColor::Red), text)
    }

    /// Something needs attention but is not a failure: kept because it changed,
    /// inferred rather than stated, a warning.
    #[must_use]
    pub fn warn(self, text: &str) -> String {
        self.paint(Self::fg(AnsiColor::Yellow), text)
    }

    /// An identifier: a uuid, a path, an email.
    #[must_use]
    pub fn id(self, text: &str) -> String {
        self.paint(Self::fg(AnsiColor::Cyan), text)
    }

    /// Secondary: units, counts that support a number rather than being it,
    /// explanatory asides.
    #[must_use]
    pub fn dim(self, text: &str) -> String {
        self.paint(Ansi::new().dimmed(), text)
    }

    #[must_use]
    pub fn bold(self, text: &str) -> String {
        self.paint(Ansi::new().bold(), text)
    }

    /// A titled rule opening a section.
    ///
    /// A rule rather than a box: a box has to be as wide as its widest line, and
    /// these sections carry paths that can be 200 characters. A box would either
    /// clip them or stretch off the screen.
    #[must_use]
    pub fn section(self, title: &str) -> String {
        let dash = if self.unicode { '\u{2500}' } else { '-' };
        let used = title.chars().count() + 4;
        let tail: String = std::iter::repeat_n(dash, RULE_WIDTH.saturating_sub(used)).collect();
        let rule = format!("{dash}{dash} {title} {tail}");
        self.paint(Ansi::new().bold(), &rule)
    }

    /// The one line that matters, framed so it is not read past.
    #[must_use]
    pub fn headline(self, text: &str) -> String {
        let (tl, tr, bl, br, h, v) = if self.unicode {
            ('\u{250c}', '\u{2510}', '\u{2514}', '\u{2518}', '\u{2500}', '\u{2502}')
        } else {
            ('+', '+', '+', '+', '-', '|')
        };
        let width = text.chars().count() + 2;
        let bar: String = std::iter::repeat_n(h, width).collect();
        let boxed = format!("{tl}{bar}{tr}\n{v} {text} {v}\n{bl}{bar}{br}");
        self.paint(Ansi::new().bold(), &boxed)
    }

    /// The marker a plan line opens with, coloured by what it means.
    #[must_use]
    pub fn marker(self, mark: Mark) -> String {
        match mark {
            Mark::Added => self.ok("+"),
            Mark::Removed => self.bad("-"),
            Mark::Unchanged => self.dim("="),
            Mark::Kept => self.warn("!"),
            Mark::Missing => self.dim("."),
            Mark::Newer => self.dim(">"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Added,
    Removed,
    Unchanged,
    /// Left alone because it changed after we wrote it.
    Kept,
    Missing,
    /// The local copy is the same history further along.
    Newer,
}

/// Pads to a column width counting characters, not bytes, and ignoring the
/// escape sequences colour adds.
///
/// `{:<20}` counts bytes and would see a coloured cell as twenty characters of
/// escape codes plus the text, wrecking every column after it.
#[must_use]
pub fn pad(text: &str, width: usize) -> String {
    let visible = visible_len(text);
    let padding: String = std::iter::repeat_n(' ', width.saturating_sub(visible)).collect();
    format!("{text}{padding}")
}

/// Characters a reader would see, skipping ANSI escape sequences.
#[must_use]
pub fn visible_len(text: &str) -> usize {
    visible(text).count()
}

/// The text with escape sequences removed.
#[must_use]
pub fn strip(text: &str) -> String {
    visible(text).collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scan {
    Text,
    /// Just saw the escape byte; a CSI sequence has `[` next.
    AfterEscape,
    /// Inside the parameters; the sequence ends at the first byte in `@..=~`.
    ///
    /// The bracket itself falls inside that range, which is why it has to be
    /// consumed before the search for the final byte starts. Treating it as the
    /// terminator leaves the parameters behind as if they were text.
    InSequence,
}

fn visible(text: &str) -> impl Iterator<Item = char> + '_ {
    let mut scan = Scan::Text;
    text.chars().filter(move |ch| match scan {
        Scan::Text => {
            if *ch == '\u{1b}' {
                scan = Scan::AfterEscape;
                return false;
            }
            true
        }
        Scan::AfterEscape => {
            scan = if *ch == '[' { Scan::InSequence } else { Scan::Text };
            false
        }
        Scan::InSequence => {
            if ('@'..='~').contains(ch) {
                scan = Scan::Text;
            }
            false
        }
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    const ESC: char = '\u{1b}';

    #[test]
    fn auto_colours_a_terminal_and_nothing_else() {
        assert!(Style::resolve(ColorChoice::Auto, false, true, true).is_colored());
        assert!(!Style::resolve(ColorChoice::Auto, false, false, true).is_colored());
    }

    #[test]
    fn no_color_wins_over_auto_but_not_over_an_explicit_request() {
        assert!(!Style::resolve(ColorChoice::Auto, true, true, true).is_colored());
        assert!(Style::resolve(ColorChoice::Always, true, true, true).is_colored());
    }

    #[test]
    fn always_colours_even_a_pipe_and_never_colours_nothing() {
        assert!(Style::resolve(ColorChoice::Always, false, false, true).is_colored());
        assert!(!Style::resolve(ColorChoice::Never, false, true, true).is_colored());
    }

    #[test]
    fn plain_output_contains_no_escapes_at_all() {
        let plain = Style::plain();
        for text in [plain.ok("+"), plain.bad("-"), plain.warn("!"), plain.id("x"), plain.dim("y")]
        {
            assert!(!text.contains(ESC), "{text:?}");
        }
        assert_eq!(plain.ok("added"), "added");
    }

    #[test]
    fn stripping_colour_leaves_the_same_text() {
        // The property the whole module rests on: colour adds nothing the reader
        // needs, so removing it loses nothing.
        // Same unicode setting on both sides: this is about colour, and box
        // drawing is a separate choice.
        let colored = Style::resolve(ColorChoice::Always, false, true, true);
        let plain = Style::resolve(ColorChoice::Never, false, true, true);
        for (a, b) in [
            (colored.ok("added"), plain.ok("added")),
            (colored.warn("kept"), plain.warn("kept")),
            (colored.section("layout"), plain.section("layout")),
            (colored.headline("2 not visible"), plain.headline("2 not visible")),
        ] {
            assert_eq!(strip(&a), b);
        }
    }

    #[test]
    fn a_section_rule_is_one_line_of_a_fixed_width() {
        let style = Style::plain();
        let rule = style.section("accounts");
        assert!(!rule.contains('\n'));
        assert_eq!(visible_len(&rule), RULE_WIDTH);
        assert!(rule.contains("accounts"));
    }

    #[test]
    fn a_very_long_title_does_not_produce_a_negative_width() {
        let rule = Style::plain().section(&"x".repeat(200));
        assert!(!rule.contains('\n'));
    }

    #[test]
    fn ascii_mode_avoids_box_drawing() {
        let ascii = Style::plain().with_unicode(false);
        for text in [ascii.section("layout"), ascii.headline("2 not visible")] {
            assert!(text.is_ascii(), "{text:?}");
        }
        let unicode = Style::plain().with_unicode(true);
        assert!(!unicode.section("layout").is_ascii());
    }

    #[test]
    fn a_headline_is_a_box_that_fits_its_text() {
        let headline = Style::plain().headline("2 not visible");
        let lines: Vec<&str> = headline.lines().collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(visible_len(lines[0]), visible_len(lines[1]));
        assert_eq!(visible_len(lines[1]), visible_len(lines[2]));
        assert!(lines[1].contains("2 not visible"));
    }

    #[test]
    fn the_bracket_of_an_escape_sequence_is_not_its_terminator() {
        // "\x1b[32m" ends at the m, not at the [. Reading the bracket as the
        // final byte leaves "32m" behind and every column after it shifts.
        let raw = format!("{ESC}[32mgreen{ESC}[0m");
        assert_eq!(strip(&raw), "green");
        assert_eq!(visible_len(&raw), 5);
    }

    #[test]
    fn padding_counts_what_a_reader_sees_not_what_a_byte_counter_does() {
        let colored = Style::resolve(ColorChoice::Always, false, true, true);
        let cell = colored.id("abc");
        assert!(cell.len() > 3, "the escapes are really there");
        assert_eq!(visible_len(&pad(&cell, 10)), 10, "the column still lines up");
        assert_eq!(visible_len(&pad("abc", 10)), 10);
        assert_eq!(pad("toolongforthis", 4), "toolongforthis", "never truncates");
    }

    #[test]
    fn markers_carry_their_meaning_and_survive_stripping() {
        let plain = Style::plain();
        assert_eq!(plain.marker(Mark::Added), "+");
        assert_eq!(plain.marker(Mark::Removed), "-");
        assert_eq!(plain.marker(Mark::Kept), "!");
        assert_eq!(plain.marker(Mark::Unchanged), "=");
    }
}
