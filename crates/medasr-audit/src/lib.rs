//! Local diagnostic event log. Implementation lands in Unit 9A.
//!
//! Per the personal/research distribution posture chosen at scaffold time,
//! v1 ships a plain rotating event log without an HMAC chain or per-install
//! keystore. The schema is still versioned so a future deployment that
//! actually requires HIPAA audit-control can layer the chain on top.
#![forbid(unsafe_code)]
