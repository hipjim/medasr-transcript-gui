//! Synthesized-keystroke text injection. Implementation lands in Unit 4.5
//! (skeleton) and Unit 6 (Citrix-aware chunking, Secure Input mid-stream).
//!
//! The PHI flow is: post-processed transcript (in `SecureBuffer<u8>`) →
//! injector chunks per `FocusTarget::chunking_policy` → backend dispatches
//! synthesized keystrokes → buffer is dropped (zero-on-drop).
//!
//! Crucially: this path **never** reads or writes the system clipboard.
#![forbid(unsafe_code)]
