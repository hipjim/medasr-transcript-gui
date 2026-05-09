use crate::pipeline::Stage;

/// Voice-command -> structural punctuation. Word-boundary, case-insensitive.
///
/// Note: this stage is intentionally narrow per R6 — only structural /
/// typographic commands. Domain expansions ("normal chest" -> boilerplate
/// paragraph) are radiology macros and are out of scope for v1.
#[derive(Default)]
pub struct CommandsStage {
    quote_open: bool,
}

const COMMANDS: &[(&str, Replacement)] = &[
    ("period",            Replacement::Punct(".")),
    ("full stop",         Replacement::Punct(".")),
    ("comma",             Replacement::Punct(",")),
    ("colon",             Replacement::Punct(":")),
    ("semicolon",         Replacement::Punct(";")),
    ("question mark",     Replacement::Punct("?")),
    ("exclamation point", Replacement::Punct("!")),
    ("exclamation mark",  Replacement::Punct("!")),
    ("new line",          Replacement::Plain("\n")),
    ("new paragraph",     Replacement::Plain("\n\n")),
    ("open paren",        Replacement::OpenBracket("(")),
    ("close paren",       Replacement::CloseBracket(")")),
    ("open bracket",      Replacement::OpenBracket("[")),
    ("close bracket",     Replacement::CloseBracket("]")),
    ("open quote",        Replacement::OpenQuote),
    ("close quote",       Replacement::CloseQuote),
    ("dash",              Replacement::Punct("—")),
];

#[derive(Clone, Copy)]
enum Replacement {
    /// Sticks to the preceding word (no leading space): the phrase
    /// "lungs are clear period" becomes "lungs are clear.".
    Punct(&'static str),
    /// Plain replacement preserving spacing.
    Plain(&'static str),
    /// "(", "[" — sticks to the FOLLOWING word, no trailing space.
    OpenBracket(&'static str),
    /// ")", "]" — sticks to the PRECEDING word, no leading space.
    CloseBracket(&'static str),
    /// Toggling open / close quote.
    OpenQuote,
    CloseQuote,
}

impl Stage for CommandsStage {
    fn apply(&self, input: &str, out: &mut String) {
        let mut quote_open = self.quote_open;
        // Tokenise on whitespace so "periodical" doesn't match "period"
        // (it's a single non-matching token).
        let mut tokens: Vec<&str> = input.split_whitespace().collect();

        // Multi-word commands need a longest-match pass first.
        let mut i = 0usize;
        let mut buf = String::with_capacity(input.len());
        while i < tokens.len() {
            let mut matched = None;
            // Try 2-word, then 1-word matches.
            for window in (1..=2).rev() {
                if i + window > tokens.len() { continue; }
                let phrase = tokens[i..i + window].join(" ").to_ascii_lowercase();
                if let Some(repl) = lookup(&phrase) {
                    matched = Some((window, repl));
                    break;
                }
            }
            if let Some((win, repl)) = matched {
                emit(&mut buf, repl, &mut quote_open);
                i += win;
            } else {
                if !buf.is_empty() && !buf.ends_with(|c: char| c.is_whitespace())
                    && needs_leading_space(&buf, tokens[i])
                {
                    buf.push(' ');
                }
                buf.push_str(tokens[i]);
                i += 1;
            }
        }

        // Replace the loop-local tokens vec to release the borrow on input
        // before writing to `out`. (lint clean-up.)
        tokens.clear();
        out.push_str(buf.trim_start_matches(|c: char| c == ' '));
    }
}

fn lookup(phrase: &str) -> Option<Replacement> {
    for &(k, v) in COMMANDS { if k == phrase { return Some(v); } }
    None
}

fn needs_leading_space(buf: &str, _next: &str) -> bool {
    if let Some(last) = buf.chars().last() {
        // After "(" or "[" or "\n", no leading space.
        if last == '(' || last == '[' || last == '\n' { return false; }
        // After a quote we just opened, no leading space.
        if last == '"' { return false; }
        return true;
    }
    false
}

fn emit(buf: &mut String, repl: Replacement, quote_open: &mut bool) {
    match repl {
        Replacement::Punct(s) => {
            // Strip any trailing space before the punctuation.
            while buf.ends_with(' ') { buf.pop(); }
            buf.push_str(s);
        }
        Replacement::Plain(s) => buf.push_str(s),
        Replacement::OpenBracket(s) => {
            if !buf.is_empty() && !buf.ends_with(char::is_whitespace) {
                buf.push(' ');
            }
            buf.push_str(s);
        }
        Replacement::CloseBracket(s) => {
            while buf.ends_with(' ') { buf.pop(); }
            buf.push_str(s);
        }
        Replacement::OpenQuote => {
            if !buf.is_empty() && !buf.ends_with(char::is_whitespace) {
                buf.push(' ');
            }
            buf.push('"');
            *quote_open = true;
        }
        Replacement::CloseQuote => {
            while buf.ends_with(' ') { buf.pop(); }
            buf.push('"');
            *quote_open = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(s: &str) -> String {
        let mut out = String::new();
        CommandsStage::default().apply(s, &mut out);
        out
    }

    #[test]
    fn period_tail() {
        assert_eq!(run("the lungs are clear period"), "the lungs are clear.");
    }

    #[test]
    fn comma_then_word() {
        assert_eq!(run("there is comma additionally a finding"),
                   "there is, additionally a finding");
    }

    #[test]
    fn full_stop_alias() {
        assert_eq!(run("done full stop"), "done.");
    }

    #[test]
    fn new_paragraph() {
        assert_eq!(
            run("no acute findings new paragraph impression colon normal"),
            "no acute findings\n\nimpression: normal"
        );
    }

    #[test]
    fn periodical_is_not_period() {
        // "periodical" stays a literal word — only the bare token "period"
        // is replaced.
        assert_eq!(run("periodical review"), "periodical review");
    }

    #[test]
    fn open_close_paren() {
        assert_eq!(
            run("the heart open paren normal close paren is fine"),
            "the heart (normal) is fine"
        );
    }

    #[test]
    fn consecutive_commands() {
        // The plan calls out "period period period" -> "..." as expected.
        assert_eq!(run("period period period"), "...");
    }
}
