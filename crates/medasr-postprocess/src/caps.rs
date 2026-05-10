//! Capitalization tidy-up.
//!
//! Two rules in v1:
//!  1. The first letter of a sentence is uppercase. A "sentence" starts at
//!     the document beginning, after `.`, `?`, `!`, or `\n` (with optional
//!     intervening whitespace).
//!  2. Standalone "i" is upper-cased to "I" (the model occasionally emits
//!     it lowercased after punctuation).

use crate::pipeline::Stage;

#[derive(Default)]
pub struct CapsStage;

impl Stage for CapsStage {
    fn apply(&self, input: &str, out: &mut String) {
        let mut at_sentence_start = true;
        let mut prev_char: Option<char> = None;
        for c in input.chars() {
            // Sentence boundaries.
            if at_sentence_start && c.is_alphabetic() {
                for upper in c.to_uppercase() {
                    out.push(upper);
                }
                at_sentence_start = false;
            } else if c.is_whitespace() {
                out.push(c);
                if matches!(prev_char, Some('.') | Some('?') | Some('!')) {
                    at_sentence_start = true;
                }
                if c == '\n' {
                    at_sentence_start = true;
                }
            } else if matches!(c, '.' | '?' | '!') {
                out.push(c);
                // The next non-space alphabetic char is a new sentence.
                at_sentence_start = true;
            } else {
                out.push(c);
                if at_sentence_start {
                    at_sentence_start = false;
                }
            }
            prev_char = Some(c);
        }
        // Standalone "i" -> "I". Done as a second pass for clarity.
        let pass1 = std::mem::take(out);
        let mut first = true;
        for tok in pass1.split(' ') {
            if !first {
                out.push(' ');
            }
            first = false;
            if tok == "i" {
                out.push('I');
            } else if tok.starts_with("i'") {
                let mut chars = tok.chars();
                if chars.next().is_some() {
                    out.push('I');
                    out.extend(chars);
                }
            } else {
                out.push_str(tok);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(s: &str) -> String {
        let mut out = String::new();
        CapsStage.apply(s, &mut out);
        out
    }

    #[test]
    fn first_word_capitalized() {
        assert_eq!(run("the lungs are clear"), "The lungs are clear");
    }

    #[test]
    fn after_period_capitalized() {
        assert_eq!(run("done. next item"), "Done. Next item");
    }

    #[test]
    fn after_question_mark() {
        assert_eq!(run("ok? yes"), "Ok? Yes");
    }

    #[test]
    fn after_paragraph() {
        assert_eq!(run("done.\n\nimpression"), "Done.\n\nImpression");
    }

    #[test]
    fn standalone_i_upper() {
        assert_eq!(run("i think i'm done"), "I think I'm done");
    }
}
