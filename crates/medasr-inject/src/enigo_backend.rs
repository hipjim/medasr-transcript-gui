//! Default `KeystrokeBackend` impl backed by the `enigo` crate.

use enigo::{Enigo, Keyboard, Settings};

use crate::backend::{BackendError, KeystrokeBackend};

pub struct EnigoBackend {
    enigo: Enigo,
}

impl EnigoBackend {
    pub fn new() -> Result<Self, BackendError> {
        let enigo = Enigo::new(&Settings::default())
            .map_err(|e| BackendError::Init(format!("{e}")))?;
        Ok(Self { enigo })
    }
}

impl KeystrokeBackend for EnigoBackend {
    fn type_unicode_string(&mut self, text: &str) -> Result<(), BackendError> {
        self.enigo
            .text(text)
            .map_err(|e| BackendError::Type(format!("{e}")))
    }
}
