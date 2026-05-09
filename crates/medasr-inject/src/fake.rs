//! `FakeBackend` — records every `type_unicode_string` call so tests can
//! assert deterministic chunking behavior at the post-process → injector
//! seam without booting a real desktop session.

use std::sync::{Arc, Mutex};

use crate::backend::{BackendError, KeystrokeBackend};

#[derive(Default, Clone)]
pub struct FakeBackend {
    calls: Arc<Mutex<Vec<String>>>,
}

impl FakeBackend {
    pub fn new() -> Self { Self::default() }

    /// All chunks the injector dispatched, in order.
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().expect("fake backend mutex poisoned").clone()
    }

    /// Reconstructed full string the caller intended to type.
    pub fn typed(&self) -> String {
        self.calls().concat()
    }

    pub fn call_count(&self) -> usize {
        self.calls.lock().expect("fake backend mutex poisoned").len()
    }
}

impl KeystrokeBackend for FakeBackend {
    fn type_unicode_string(&mut self, text: &str) -> Result<(), BackendError> {
        self.calls
            .lock()
            .expect("fake backend mutex poisoned")
            .push(text.to_owned());
        Ok(())
    }
}
