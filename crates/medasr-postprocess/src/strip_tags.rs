//! Strip MedASR's structural-section tags such as `[EXAM TYPE]`,
//! `[INDICATION]`, `[FINDINGS]`, `[NAME]` from the transcript.
//!
//! Heuristic: anything inside `[...]` whose body is exclusively
//! uppercase ASCII letters, spaces, slashes, hyphens, and digits is
//! considered a tag. Mixed-case bracketed text (e.g. `[Figure 1]`,
//! `[abnormal]`) is left intact.

use crate::pipeline::Stage;

#[derive(Default)]
pub struct StripTagsStage;

impl Stage for StripTagsStage {
    fn apply(&self, input: &str, out: &mut String) {
        let bytes = input.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'[' {
                // Look ahead for the matching ']' within reasonable bounds.
                let rest = &input[i + 1..];
                if let Some(close) = rest.find(']') {
                    if close <= 64 {
                        let body = &rest[..close];
                        if is_structural_tag(body) {
                            // Drop the tag and any single trailing space.
                            let mut next = i + 1 + close + 1;
                            if next < bytes.len() && bytes[next] == b' ' {
                                next += 1;
                            }
                            i = next;
                            continue;
                        }
                    }
                }
            }
            // Push current char (UTF-8 safe — input is &str).
            let c_end = next_utf8_boundary(bytes, i);
            out.push_str(&input[i..c_end]);
            i = c_end;
        }
        // Tidy: collapse a leading run of spaces / blank lines that the
        // strip might have left behind.
        let trimmed_start = out.trim_start_matches([' ', '\t']).to_owned();
        out.clear();
        out.push_str(&trimmed_start);
    }
}

fn is_structural_tag(body: &str) -> bool {
    // Empty bodies aren't tags ([]).
    if body.is_empty() {
        return false;
    }
    // Require at least one uppercase letter.
    if !body.chars().any(|c| c.is_ascii_uppercase()) {
        return false;
    }
    // All chars must be uppercase / space / slash / hyphen / digit.
    body.chars()
        .all(|c| c.is_ascii_uppercase() || c == ' ' || c == '/' || c == '-' || c.is_ascii_digit())
}

fn next_utf8_boundary(bytes: &[u8], i: usize) -> usize {
    if i >= bytes.len() {
        return bytes.len();
    }
    let b = bytes[i];
    // ASCII (< 0x80) and continuation/invalid lead bytes (< 0xC0) both
    // advance one byte; the latter to make progress on malformed input
    // rather than looping.
    let len = if b < 0xC0 {
        1
    } else if b < 0xE0 {
        2
    } else if b < 0xF0 {
        3
    } else {
        4
    };
    (i + len).min(bytes.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(s: &str) -> String {
        let mut out = String::new();
        StripTagsStage.apply(s, &mut out);
        out
    }

    #[test]
    fn strips_simple_tag() {
        assert_eq!(run("[NAME] testing 1 2 3"), "testing 1 2 3");
    }

    #[test]
    fn strips_multiple_tags() {
        assert_eq!(
            run("[EXAM TYPE] CT chest period [FINDINGS] colon clear"),
            "CT chest period colon clear"
        );
    }

    #[test]
    fn keeps_mixed_case_brackets() {
        assert_eq!(run("see [Figure 1]"), "see [Figure 1]");
    }

    #[test]
    fn keeps_lowercase_brackets() {
        assert_eq!(run("[note]"), "[note]");
    }

    #[test]
    fn keeps_unbalanced_brackets() {
        assert_eq!(run("normal [unfinished"), "normal [unfinished");
    }

    #[test]
    fn empty_brackets_are_kept() {
        assert_eq!(run("see []"), "see []");
    }

    #[test]
    fn handles_dash_and_slash_in_tag() {
        assert_eq!(run("[CT/MRI-CONTRAST] enhanced"), "enhanced");
    }
}
