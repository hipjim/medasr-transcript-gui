//! "five point three centimeters" -> "5.3 cm".
//!
//! Walks the input scanning for runs of number-words, optionally followed
//! by "point <digit-word>+" and an optional unit word. Everything else —
//! including whitespace and punctuation — passes through verbatim.

use crate::pipeline::Stage;

#[derive(Default)]
pub struct NumbersStage;

const ONES: &[(&str, u64)] = &[
    ("zero", 0), ("oh", 0),
    ("one", 1), ("two", 2), ("three", 3), ("four", 4),
    ("five", 5), ("six", 6), ("seven", 7), ("eight", 8), ("nine", 9),
];
const TEENS: &[(&str, u64)] = &[
    ("ten", 10), ("eleven", 11), ("twelve", 12), ("thirteen", 13),
    ("fourteen", 14), ("fifteen", 15), ("sixteen", 16),
    ("seventeen", 17), ("eighteen", 18), ("nineteen", 19),
];
const TENS: &[(&str, u64)] = &[
    ("twenty", 20), ("thirty", 30), ("forty", 40), ("fifty", 50),
    ("sixty", 60), ("seventy", 70), ("eighty", 80), ("ninety", 90),
];
const SCALES: &[(&str, u64)] = &[("hundred", 100), ("thousand", 1_000)];

const UNITS: &[(&str, &str)] = &[
    ("millimeter", "mm"), ("millimeters", "mm"),
    ("centimeter", "cm"), ("centimeters", "cm"),
    ("meter", "m"),       ("meters", "m"),
    ("milliliter", "mL"), ("milliliters", "mL"),
    ("liter", "L"),       ("liters", "L"),
    ("milligram", "mg"),  ("milligrams", "mg"),
    ("gram", "g"),        ("grams", "g"),
    ("kilogram", "kg"),   ("kilograms", "kg"),
    ("microgram", "mcg"), ("micrograms", "mcg"),
    ("degree", "°"),      ("degrees", "°"),
    ("percent", "%"),
];

fn ones(w: &str) -> Option<u64> { ONES.iter().find(|(k, _)| *k == w).map(|&(_, v)| v) }
fn teens(w: &str) -> Option<u64> { TEENS.iter().find(|(k, _)| *k == w).map(|&(_, v)| v) }
fn tens(w: &str) -> Option<u64> { TENS.iter().find(|(k, _)| *k == w).map(|&(_, v)| v) }
fn scale(w: &str) -> Option<u64> { SCALES.iter().find(|(k, _)| *k == w).map(|&(_, v)| v) }
fn unit(w: &str) -> Option<&'static str> { UNITS.iter().find(|(k, _)| *k == w).map(|&(_, v)| v) }

#[derive(Debug, Clone)]
struct Token {
    word: String,           // lowercased
    start: usize,           // byte offset in input
    end: usize,             // byte offset in input (exclusive)
    trailing_punct: String, // any "." "," etc. attached after the word
}

fn tokenize(input: &str) -> Vec<Token> {
    let bytes = input.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        // Skip non-word bytes (whitespace + punctuation).
        if !is_word_byte(bytes[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && is_word_byte(bytes[i]) {
            i += 1;
        }
        let end = i;
        // Strip trailing punctuation immediately after this word.
        let mut trailing_start = i;
        while trailing_start < bytes.len()
            && matches!(bytes[trailing_start], b'.' | b',' | b';' | b':' | b'?' | b'!')
        {
            trailing_start += 1;
        }
        let trailing = &input[end..trailing_start];
        out.push(Token {
            word: input[start..end].to_ascii_lowercase(),
            start,
            end,
            trailing_punct: trailing.to_owned(),
        });
        i = trailing_start;
    }
    out
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'\''
}

impl Stage for NumbersStage {
    fn apply(&self, input: &str, out: &mut String) {
        // Split on newlines so a number sequence cannot span paragraph
        // boundaries. The CommandsStage emits "\n\n" for "new paragraph";
        // that's a hard break in the dictation, never the middle of a
        // spelled number.
        let mut first_line = true;
        for line in input.split_inclusive('\n') {
            if !first_line {
                // split_inclusive keeps the '\n' on the previous chunk; we
                // need to do nothing here.
            }
            first_line = false;
            apply_line(line, out);
        }
    }
}

fn apply_line(input: &str, out: &mut String) {
    let tokens = tokenize(input);
    let mut consumed_until: usize = 0;
    let mut i = 0;
    while i < tokens.len() {
        if let Some((value, dec, words)) = consume_number(&tokens, i) {
            let after = i + words;
            let (unit_short, unit_words) = match tokens.get(after) {
                Some(t) => match unit(&t.word) {
                    Some(u) => (Some(u), 1),
                    None => (None, 0),
                },
                None => (None, 0),
            };
            let consumed_words = words + unit_words;
            let last_token = &tokens[i + consumed_words - 1];
            out.push_str(&input[consumed_until..tokens[i].start]);
            let formatted = format_number(value, dec);
            out.push_str(&formatted);
            if let Some(u) = unit_short {
                out.push(' ');
                out.push_str(u);
            }
            out.push_str(&last_token.trailing_punct);
            consumed_until = last_token.end + last_token.trailing_punct.len();
            i += consumed_words;
        } else {
            i += 1;
        }
    }
    out.push_str(&input[consumed_until..]);
}

fn consume_number(tokens: &[Token], start: usize) -> Option<(u64, Option<u64>, usize)> {
    let mut total: u64 = 0;
    let mut current: u64 = 0;
    let mut i = start;
    let mut consumed_any = false;
    while i < tokens.len() {
        let w = &tokens[i].word;
        if let Some(v) = ones(w) {
            current = current.checked_add(v).unwrap_or(current);
            consumed_any = true;
        } else if let Some(v) = teens(w) {
            current = current.checked_add(v).unwrap_or(current);
            consumed_any = true;
        } else if let Some(v) = tens(w) {
            current = current.checked_add(v).unwrap_or(current);
            consumed_any = true;
        } else if let Some(s) = scale(w) {
            if !consumed_any { return None; }
            if s == 1_000 {
                total += if current == 0 { 1_000 } else { current * 1_000 };
                current = 0;
            } else {
                current = if current == 0 { 100 } else { current * 100 };
            }
        } else {
            break;
        }
        i += 1;
        // Stop scanning if the previous token had trailing punctuation —
        // that punctuation ends the number sequence cleanly.
        if !tokens[i - 1].trailing_punct.is_empty() { break; }
    }
    if !consumed_any { return None; }
    let int_part = total + current;

    if i + 1 < tokens.len() && tokens[i].word == "point" && tokens[i].trailing_punct.is_empty() {
        let mut decimal_digits: u64 = 0;
        let mut digits_consumed = 0;
        let mut j = i + 1;
        while j < tokens.len() {
            if let Some(v) = ones(&tokens[j].word) {
                decimal_digits = decimal_digits * 10 + v;
                digits_consumed += 1;
                if !tokens[j].trailing_punct.is_empty() { j += 1; break; }
                j += 1;
            } else { break; }
        }
        if digits_consumed > 0 {
            return Some((int_part, Some(decimal_digits), j - start));
        }
    }
    Some((int_part, None, i - start))
}

fn format_number(int_part: u64, decimal: Option<u64>) -> String {
    match decimal {
        None => format!("{int_part}"),
        Some(d) => format!("{int_part}.{d}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(s: &str) -> String {
        let mut out = String::new();
        NumbersStage::default().apply(s, &mut out);
        out
    }

    #[test]
    fn simple_decimal_with_unit() {
        assert_eq!(run("five point three centimeters"), "5.3 cm");
    }

    #[test]
    fn integer_with_unit() {
        assert_eq!(run("twelve millimeters"), "12 mm");
    }

    #[test]
    fn thousands() {
        assert_eq!(run("five thousand two hundred"), "5200");
    }

    #[test]
    fn no_unit_keeps_word() {
        assert_eq!(run("five years old"), "5 years old");
    }

    #[test]
    fn passthrough_for_non_numbers() {
        assert_eq!(run("the lungs are clear"), "the lungs are clear");
    }

    #[test]
    fn degrees() {
        assert_eq!(run("ninety eight point six degrees"), "98.6 °");
    }

    #[test]
    fn percent() {
        assert_eq!(run("twenty percent"), "20 %");
    }

    #[test]
    fn preserves_newlines_around_numbers() {
        assert_eq!(
            run("one\n\ntwo millimeters"),
            "1\n\n2 mm"
        );
    }

    #[test]
    fn unit_with_trailing_period() {
        assert_eq!(run("five centimeters."), "5 cm.");
    }
}
