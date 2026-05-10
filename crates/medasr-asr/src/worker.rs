//! Dedicated OS-thread ASR worker.
//!
//! sherpa-onnx + ONNX Runtime are CPU-bound and not async-friendly. The
//! orchestrator owns one of these and feeds it work over a `std::sync::mpsc`
//! channel. Cancellation is plumbed via `CancellationToken` so that a
//! hotkey-release-during-transcribe can abort within ~100 ms.

use std::sync::mpsc;
use std::thread::JoinHandle;

use medasr_secure_buffer::SecureBuffer;
use medasr_types::AsrResult;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

use crate::recognizer::{Asr, AsrError, ModelPaths};

pub enum AsrCommand {
    /// Transcribe the supplied audio and reply on `reply`.
    Transcribe {
        samples: SecureBuffer<i16>,
        cancel: CancellationToken,
        reply: mpsc::Sender<AsrResponse>,
    },
    /// Shut the worker thread down. The worker drops the recognizer and
    /// exits.
    Shutdown,
}

pub type AsrResponse = Result<AsrResult, AsrError>;

/// Handle held by the orchestrator. Drop to shut down (this sends
/// `Shutdown` on a best-effort basis and joins the thread).
pub struct AsrWorkerHandle {
    cmd_tx: Option<mpsc::Sender<AsrCommand>>,
    join: Option<JoinHandle<()>>,
}

impl AsrWorkerHandle {
    pub fn sender(&self) -> mpsc::Sender<AsrCommand> {
        self.cmd_tx
            .as_ref()
            .expect("worker handle still active")
            .clone()
    }

    pub fn shutdown(mut self) {
        if let Some(tx) = self.cmd_tx.take() {
            let _ = tx.send(AsrCommand::Shutdown);
        }
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

impl Drop for AsrWorkerHandle {
    fn drop(&mut self) {
        if let Some(tx) = self.cmd_tx.take() {
            let _ = tx.send(AsrCommand::Shutdown);
        }
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

/// Spawn the ASR worker thread. Returns once `Asr::new` has completed
/// (including warmup) so the caller knows transcription is ready.
pub fn spawn_worker(paths: ModelPaths) -> Result<AsrWorkerHandle, AsrError> {
    let (cmd_tx, cmd_rx) = mpsc::channel::<AsrCommand>();
    let (init_tx, init_rx) = mpsc::sync_channel::<Result<(), AsrError>>(1);
    let join = std::thread::Builder::new()
        .name("medasr-asr".into())
        .spawn(move || run(paths, cmd_rx, init_tx))
        .map_err(|e| AsrError::ModelMissing(std::path::PathBuf::from(format!("(spawn: {e})"))))?;

    match init_rx.recv() {
        Ok(Ok(())) => Ok(AsrWorkerHandle {
            cmd_tx: Some(cmd_tx),
            join: Some(join),
        }),
        Ok(Err(e)) => {
            // Worker exited; let it complete cleanly.
            let _ = join.join();
            Err(e)
        }
        Err(_) => {
            let _ = join.join();
            Err(AsrError::Init)
        }
    }
}

fn run(
    paths: ModelPaths,
    cmd_rx: mpsc::Receiver<AsrCommand>,
    init_tx: mpsc::SyncSender<Result<(), AsrError>>,
) {
    let warmup_cancel = CancellationToken::new();
    let asr = match Asr::new(&paths, &warmup_cancel) {
        Ok(a) => a,
        Err(e) => {
            let _ = init_tx.send(Err(e));
            return;
        }
    };
    let _ = init_tx.send(Ok(()));
    info!("ASR worker ready");

    while let Ok(cmd) = cmd_rx.recv() {
        match cmd {
            AsrCommand::Transcribe {
                samples,
                cancel,
                reply,
            } => {
                let result = asr.transcribe(samples.as_slice(), &cancel);
                if let Err(send_err) = reply.send(result) {
                    error!("asr reply channel closed: {send_err}");
                }
                // `samples` SecureBuffer drops here -> zeroized.
            }
            AsrCommand::Shutdown => break,
        }
    }
    info!("ASR worker shut down");
}
