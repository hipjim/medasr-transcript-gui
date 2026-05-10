use crate::pipeline::Stage;

/// Voice-command -> structural punctuation. Word-boundary, case-insensitive.
///
/// Two formats are recognised:
///
/// 1. **MedASR canonical tokens** (curly-brace form): the model emits
///    `{period}`, `{comma}`, `{colon}`, `{semicolon}`, `{question mark}`,
///    `{exclamation point}`, `{new line}`, `{new paragraph}`,
///    `{open paren}`, `{close paren}`, `{open quote}`, `{close quote}`,
///    `{dash}`. This is the format observed empirically from
///    `csukuangfj/sherpa-onnx-medasr-ctc-en-int8-2025-12-25` on the test
///    wav fixtures.
///
/// 2. **Spelled-word fallback** (legacy form): some users / models emit
///    "period", "comma", etc. as plain words. Kept for backward
///    compatibility and for the case when MedASR's CTC fails to emit the
///    `{}` token wrapping but still emits the word.
///
/// Per R6 the stage covers structural / typographic commands only. Domain
/// macros ("normal chest" -> boilerplate paragraph) are out of scope.
#[derive(Default)]
pub struct CommandsStage {
    quote_open: bool,
}

const COMMANDS: &[(&str, Replacement)] = &[
    ("period", Replacement::Punct(".")),
    ("full stop", Replacement::Punct(".")),
    ("comma", Replacement::Punct(",")),
    ("colon", Replacement::Punct(":")),
    ("semicolon", Replacement::Punct(";")),
    ("question mark", Replacement::Punct("?")),
    ("exclamation point", Replacement::Punct("!")),
    ("exclamation mark", Replacement::Punct("!")),
    ("new line", Replacement::Plain("\n")),
    ("new paragraph", Replacement::Plain("\n\n")),
    ("open paren", Replacement::OpenBracket("(")),
    ("close paren", Replacement::CloseBracket(")")),
    ("open bracket", Replacement::OpenBracket("[")),
    ("close bracket", Replacement::CloseBracket("]")),
    ("open quote", Replacement::OpenQuote),
    ("close quote", Replacement::CloseQuote),
    ("dash", Replacement::Punct("—")),
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
        // First pass: replace MedASR's `{<command>}` tokens with the
        // spelled-word form so the existing word-tokenizer below catches
        // them too. This keeps both code paths through one matcher.
        let unwrapped = unwrap_brace_tokens(input);
        let mut quote_open = self.quote_open;
        // Tokenise on whitespace so "periodical" doesn't match "period"
        // (it's a single non-matching token).
        let mut tokens: Vec<&str> = unwrapped.split_whitespace().collect();

        // Multi-word commands need a longest-match pass first.
        let mut i = 0usize;
        let mut buf = String::with_capacity(input.len());
        while i < tokens.len() {
            let mut matched = None;
            // Try 2-word, then 1-word matches.
            for window in (1..=2).rev() {
                if i + window > tokens.len() {
                    continue;
                }
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
                if !buf.is_empty()
                    && !buf.ends_with(|c: char| c.is_whitespace())
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
        out.push_str(buf.trim_start_matches(' '));
    }
}

fn lookup(phrase: &str) -> Option<Replacement> {
    for &(k, v) in COMMANDS {
        if k == phrase {
            return Some(v);
        }
    }
    None
}

/// Strip the `{...}` wrapping that MedASR emits around its command tokens.
/// `{period}` -> `period`, `{new paragraph}` -> `new paragraph`, etc.
/// Anything that isn't a known command stays wrapped so we don't corrupt
/// real bracketed content.
fn unwrap_brace_tokens(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c != '{' {
            out.push(c);
            continue;
        }
        // Find the matching '}'. Bound the search at 32 chars to keep this
        // cheap and avoid pathological inputs.
        let rest = &input[i + 1..];
        if let Some(close) = rest.find('}') {
            if close <= 32 {
                let inside = &rest[..close];
                if lookup(&inside.to_ascii_lowercase()).is_some() {
                    out.push_str(inside);
                    // Advance the iterator past the close brace.
                    let target = i + 1 + close + 1;
                    while let Some(&(j, _)) = chars.peek() {
                        if j >= target {
                            break;
                        }
                        chars.next();
                    }
                    continue;
                }
            }
        }
        out.push(c);
    }
    out
}

fn needs_leading_space(buf: &str, _next: &str) -> bool {
    if let Some(last) = buf.chars().last() {
        // After "(" or "[" or "\n", no leading space.
        if last == '(' || last == '[' || last == '\n' {
            return false;
        }
        // After a quote we just opened, no leading space.
        if last == '"' {
            return false;
        }
        return true;
    }
    false
}

fn emit(buf: &mut String, repl: Replacement, quote_open: &mut bool) {
    match repl {
        Replacement::Punct(s) => {
            // Strip any trailing space before the punctuation.
            while buf.ends_with(' ') {
                buf.pop();
            }
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
            while buf.ends_with(' ') {
                buf.pop();
            }
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
            while buf.ends_with(' ') {
                buf.pop();
            }
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
        assert_eq!(
            run("there is comma additionally a finding"),
            "there is, additionally a finding"
        );
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

    #[test]
    fn medasr_brace_period() {
        assert_eq!(run("the lungs are clear {period}"), "the lungs are clear.");
    }

    #[test]
    fn medasr_brace_new_paragraph() {
        assert_eq!(
            run("findings normal {new paragraph} impression"),
            "findings normal\n\nimpression"
        );
    }

    #[test]
    fn medasr_brace_colon_inline() {
        assert_eq!(run("impression {colon} normal"), "impression: normal");
    }

    #[test]
    fn unknown_braced_content_stays_wrapped() {
        // We don't strip braces around non-commands.
        assert_eq!(run("see figure {1}"), "see figure {1}");
    }
}
