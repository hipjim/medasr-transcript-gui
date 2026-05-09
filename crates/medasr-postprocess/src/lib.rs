//! Post-processing of raw MedASR transcripts.
//!
//! Implementation lands in Unit 8. v1 is a synchronous in-process pipeline of
//! pure transforms (voice commands → numbers → caps). The trait seam stays
//! synchronous in v1; future sidecar implementations can wrap in an async
//! adapter at the orchestrator call-site, where `spawn_blocking` is already
//! present.

#![forbid(unsafe_code)]

/// Stage that transforms a transcript fragment.
pub trait Stage {
    fn apply(&self, input: &str, out: &mut String);
}

/// Composite post-processor; v1 holds an ordered Vec of `Stage` impls.
#[derive(Default)]
pub struct Pipeline {
    pub stages: Vec<Box<dyn Stage + Send + Sync>>,
}

impl Pipeline {
    pub fn run(&self, input: &str) -> String {
        let mut buf = String::with_capacity(input.len());
        let mut src = input.to_owned();
        for stage in &self.stages {
            buf.clear();
            stage.apply(&src, &mut buf);
            std::mem::swap(&mut src, &mut buf);
        }
        src
    }
}
