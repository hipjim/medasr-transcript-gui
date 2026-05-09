/// One stage in the post-processing pipeline. Pure transform: take a
/// borrowed string, write the transformed result into the supplied
/// String. The stage owns whatever regex / map state it needs.
pub trait Stage: Send + Sync {
    fn apply(&self, input: &str, out: &mut String);
}

/// Composite synchronous post-processor.
pub struct Pipeline {
    stages: Vec<Box<dyn Stage>>,
}

impl Pipeline {
    #[must_use]
    pub fn new() -> Self { Self { stages: Vec::new() } }

    #[must_use]
    pub fn with<S: Stage + 'static>(mut self, stage: S) -> Self {
        self.stages.push(Box::new(stage));
        self
    }

    /// Run the pipeline.
    #[must_use]
    pub fn run(&self, input: &str) -> String {
        let mut src = input.to_owned();
        let mut buf = String::with_capacity(input.len());
        for stage in &self.stages {
            buf.clear();
            stage.apply(&src, &mut buf);
            std::mem::swap(&mut src, &mut buf);
        }
        src
    }
}

impl Default for Pipeline {
    fn default() -> Self { Self::new() }
}
