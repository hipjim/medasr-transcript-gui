use tracing::{debug, warn};

use crate::events::{AbortReason, ErrorClass, Event};

/// Coarse-grained app state. Carried side data (FocusTarget, audio
/// buffers, error context) lives on the orchestrator; the machine only
/// tracks the discriminator so transitions are reasoning-clean.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Uninitialized,
    EulaPending,
    PermissionsPending,
    ModelMissing,
    Downloading,
    Verifying,
    Warming,
    Ready,
    Recording,
    Transcribing,
    Injecting,
    /// Soft error/interruption: no toast, return to Ready on
    /// `Acknowledged`.
    Aborted(AbortReason),
    /// Hard error: toast shown.
    Error(ErrorClass),
    Quitting,
}

/// Side-effect hint emitted by `Machine::on_event`. The orchestrator
/// reads this and dispatches the corresponding side-effect (start audio,
/// kick the ASR worker, type the transcript, surface a toast, etc.).
///
/// We deliberately keep this narrow: the machine never owns IO; it just
/// declares what should happen, and the orchestrator does it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionEffect {
    /// Nothing — same state, no side-effect.
    None,
    /// Pin focus + start cpal capture + start the recording timer.
    StartRecording,
    /// Stop capture, ship buffer to ASR worker.
    StopRecordingAndTranscribe,
    /// Discard the audio buffer (PHI zeroize).
    DropAudioBuffer,
    /// Run post-processing then dispatch to injector.
    PostProcessAndInject,
    /// User acknowledged a soft abort or error toast.
    ReturnToReady,
    /// Show toast `class`.
    ShowErrorToast(ErrorClass),
    /// Show non-blocking abort note.
    ShowAbortNote(AbortReason),
    /// Begin model download flow.
    BeginDownload,
}

#[derive(Debug)]
pub struct Machine {
    state: State,
}

impl Machine {
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: State::Uninitialized,
        }
    }

    pub fn state(&self) -> State {
        self.state
    }

    /// Apply an event and return a `TransitionEffect`.
    ///
    /// Invalid (state, event) pairs are logged at WARN and ignored — the
    /// machine never panics on unexpected events. This is per the plan:
    /// "invalid transitions log + ignore".
    pub fn on_event(&mut self, event: Event) -> TransitionEffect {
        let (next, effect) = match (self.state, event) {
            // Onboarding
            (State::Uninitialized, _) => (State::EulaPending, TransitionEffect::None),
            (State::EulaPending, Event::EulaAccepted) => {
                (State::PermissionsPending, TransitionEffect::None)
            }
            (State::EulaPending, Event::EulaDeclined) => (State::Quitting, TransitionEffect::None),
            (State::PermissionsPending, Event::PermissionsGranted) => {
                (State::ModelMissing, TransitionEffect::None)
            }
            (State::ModelMissing, Event::ModelDownloadStarted) => {
                (State::Downloading, TransitionEffect::BeginDownload)
            }
            (State::Downloading, Event::ModelDownloadComplete) => {
                (State::Verifying, TransitionEffect::None)
            }
            (State::Verifying, Event::ModelVerifyOk) => (State::Warming, TransitionEffect::None),
            (State::Warming, Event::WarmupComplete) => (State::Ready, TransitionEffect::None),

            // Push-to-talk
            (State::Ready, Event::HotkeyPressed) => {
                (State::Recording, TransitionEffect::StartRecording)
            }
            (State::Recording, Event::HotkeyReleased { .. }) => (
                State::Transcribing,
                TransitionEffect::StopRecordingAndTranscribe,
            ),
            (State::Recording, Event::Aborted(r)) => {
                (State::Aborted(r), TransitionEffect::DropAudioBuffer)
            }
            (State::Transcribing, Event::TranscriptReady) => {
                (State::Injecting, TransitionEffect::PostProcessAndInject)
            }
            (State::Transcribing, Event::Aborted(r)) => {
                (State::Aborted(r), TransitionEffect::DropAudioBuffer)
            }
            (State::Injecting, Event::InjectionComplete) => (State::Ready, TransitionEffect::None),
            (State::Injecting, Event::Error(c)) => {
                (State::Error(c), TransitionEffect::ShowErrorToast(c))
            }

            // No-speech short-circuit: VAD said quiet → stay in Ready.
            (State::Transcribing, Event::NoSpeech) => (
                State::Aborted(AbortReason::NoSpeechDetected),
                TransitionEffect::DropAudioBuffer,
            ),

            // Sleep / wake
            (State::Recording, Event::OsSleeping) => (
                State::Aborted(AbortReason::OsSleep),
                TransitionEffect::DropAudioBuffer,
            ),

            // Errors from anywhere active that wasn't already matched
            // above (Injecting + Error is handled earlier).
            (
                State::Recording
                | State::Transcribing
                | State::Warming
                | State::Downloading
                | State::Verifying
                | State::Ready,
                Event::Error(c),
            ) => (State::Error(c), TransitionEffect::ShowErrorToast(c)),

            // Acknowledgement of a soft abort or error toast.
            (State::Aborted(r), Event::Acknowledged) => {
                debug!(?r, "abort acknowledged");
                (State::Ready, TransitionEffect::ReturnToReady)
            }
            (State::Error(c), Event::Acknowledged) => {
                debug!(?c, "error acknowledged");
                (State::Ready, TransitionEffect::ReturnToReady)
            }
            (State::Aborted(r), _) => {
                // Allow soft-abort-show before acknowledgement (idempotent).
                return TransitionEffect::ShowAbortNote(r);
            }

            // Anything else: invalid in this state. Don't panic.
            (s, e) => {
                warn!(?s, ?e, "ignored invalid transition");
                return TransitionEffect::None;
            }
        };
        let prev = self.state;
        self.state = next;
        if prev != next {
            debug!(?prev, ?next, "state transition");
        }
        effect
    }
}

impl Default for Machine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn drive_to_ready(m: &mut Machine) {
        let _ = m.on_event(Event::EulaAccepted); // -> EulaPending (Uninit-> EulaPending happens on first event)
        let _ = m.on_event(Event::EulaAccepted); // EulaPending -> PermissionsPending
        let _ = m.on_event(Event::PermissionsGranted);
        let _ = m.on_event(Event::ModelDownloadStarted);
        let _ = m.on_event(Event::ModelDownloadComplete);
        let _ = m.on_event(Event::ModelVerifyOk);
        let _ = m.on_event(Event::WarmupComplete);
        assert_eq!(m.state(), State::Ready);
    }

    #[test]
    fn full_dictation_cycle() {
        let mut m = Machine::new();
        drive_to_ready(&mut m);
        assert_eq!(
            m.on_event(Event::HotkeyPressed),
            TransitionEffect::StartRecording
        );
        assert_eq!(m.state(), State::Recording);
        assert_eq!(
            m.on_event(Event::HotkeyReleased {
                held: Duration::from_secs(2)
            }),
            TransitionEffect::StopRecordingAndTranscribe
        );
        assert_eq!(m.state(), State::Transcribing);
        assert_eq!(
            m.on_event(Event::TranscriptReady),
            TransitionEffect::PostProcessAndInject
        );
        assert_eq!(m.state(), State::Injecting);
        assert_eq!(m.on_event(Event::InjectionComplete), TransitionEffect::None);
        assert_eq!(m.state(), State::Ready);
    }

    #[test]
    fn no_speech_yields_aborted() {
        let mut m = Machine::new();
        drive_to_ready(&mut m);
        m.on_event(Event::HotkeyPressed);
        m.on_event(Event::HotkeyReleased {
            held: Duration::from_millis(50),
        });
        assert_eq!(
            m.on_event(Event::NoSpeech),
            TransitionEffect::DropAudioBuffer
        );
        assert!(matches!(
            m.state(),
            State::Aborted(AbortReason::NoSpeechDetected)
        ));
        // Acknowledge -> Ready.
        m.on_event(Event::Acknowledged);
        assert_eq!(m.state(), State::Ready);
    }

    #[test]
    fn os_sleep_during_recording_aborts() {
        let mut m = Machine::new();
        drive_to_ready(&mut m);
        m.on_event(Event::HotkeyPressed);
        let eff = m.on_event(Event::OsSleeping);
        assert_eq!(eff, TransitionEffect::DropAudioBuffer);
        assert!(matches!(m.state(), State::Aborted(AbortReason::OsSleep)));
    }

    #[test]
    fn cap_exceeded_during_recording() {
        let mut m = Machine::new();
        drive_to_ready(&mut m);
        m.on_event(Event::HotkeyPressed);
        let eff = m.on_event(Event::Aborted(AbortReason::CapExceeded));
        assert_eq!(eff, TransitionEffect::DropAudioBuffer);
        assert!(matches!(
            m.state(),
            State::Aborted(AbortReason::CapExceeded)
        ));
    }

    #[test]
    fn invalid_transitions_are_ignored_not_panicking() {
        let mut m = Machine::new();
        drive_to_ready(&mut m);
        // HotkeyReleased while Ready is invalid; should be a no-op.
        let eff = m.on_event(Event::HotkeyReleased {
            held: Duration::from_secs(1),
        });
        assert_eq!(eff, TransitionEffect::None);
        assert_eq!(m.state(), State::Ready);
    }

    #[test]
    fn eula_decline_quits() {
        let mut m = Machine::new();
        m.on_event(Event::EulaDeclined); // Uninit -> EulaPending
        m.on_event(Event::EulaDeclined); // EulaPending -> Quitting
        assert_eq!(m.state(), State::Quitting);
    }

    #[test]
    fn error_during_injection_surfaces_toast() {
        let mut m = Machine::new();
        drive_to_ready(&mut m);
        m.on_event(Event::HotkeyPressed);
        m.on_event(Event::HotkeyReleased {
            held: Duration::from_secs(2),
        });
        m.on_event(Event::TranscriptReady);
        let eff = m.on_event(Event::Error(ErrorClass::TargetWindowLost));
        assert_eq!(
            eff,
            TransitionEffect::ShowErrorToast(ErrorClass::TargetWindowLost)
        );
        assert!(matches!(
            m.state(),
            State::Error(ErrorClass::TargetWindowLost)
        ));
    }
}
