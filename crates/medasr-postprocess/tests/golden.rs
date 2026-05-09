//! Golden-file tests for the full pipeline (commands → numbers → caps).
//!
//! These cover the integration shape the orchestrator will see: raw model
//! output `text` -> typed text. Curated radiology-flavoured fixtures from
//! the plan's R6 / Unit 8 spec.

use medasr_postprocess::default_pipeline;

fn run(input: &str) -> String {
    default_pipeline().run(input)
}

#[test]
fn radiology_clear_lungs() {
    assert_eq!(
        run("the lungs are clear period"),
        "The lungs are clear."
    );
}

#[test]
fn impression_with_paragraph() {
    assert_eq!(
        run("no acute findings new paragraph impression colon normal"),
        "No acute findings\n\nImpression: normal"
    );
}

#[test]
fn measurement_with_unit() {
    assert_eq!(
        run("nodule measures five point three centimeters period"),
        "Nodule measures 5.3 cm."
    );
}

#[test]
fn integer_count_with_word() {
    // "five years old" — "years" isn't a unit, so it stays plain.
    assert_eq!(
        run("the patient is five years old period"),
        "The patient is 5 years old."
    );
}

#[test]
fn paragraph_then_capitalized_next_sentence() {
    assert_eq!(
        run("findings comma normal heart period new paragraph impression colon no acute disease"),
        "Findings, normal heart.\n\nImpression: no acute disease"
    );
}
