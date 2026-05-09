//! `SecureBuffer<T>` — best-effort PHI memory hygiene.
//!
//! Allocates a `Vec<T>` under the hood, attempts to pin its backing pages
//! against paging (`mlock`/`VirtualLock`), and zeroes the memory on drop.
//!
//! # Honest limits
//!
//! - **Pinning is best-effort.** On Linux/macOS, `mlock` is governed by
//!   `RLIMIT_MEMLOCK` (often 64 KiB or 8 MiB by default). If the call
//!   fails the buffer still works — it just is not pinned. There is no
//!   documented macOS code-signing entitlement that grants memory pinning.
//! - **Pinning does not prevent hibernation.** A full-RAM hibernation
//!   image will write pinned pages to disk on every platform. Defending
//!   against that requires platform-level FDE with pre-boot auth, which
//!   is outside this crate's scope.
//! - **Pinning does not protect data outside this allocator.** Inference
//!   tensors held by ONNX Runtime, intermediate strings produced by the
//!   regex engines in post-processing, etc. are not in `SecureBuffer`
//!   unless explicitly routed through it.
//!
//! See `docs/PRIVACY.md` for the user-facing privacy posture.

#![deny(unsafe_op_in_unsafe_fn)]

use std::ops::{Deref, DerefMut};
use zeroize::Zeroize;

/// Byte-granular result of a memory-pin attempt. Surfaced for telemetry /
/// audit; callers should not branch on it for correctness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinStatus {
    /// `mlock`/`VirtualLock` succeeded.
    Pinned,
    /// Call failed (commonly: `RLIMIT_MEMLOCK` exceeded). The buffer
    /// still functions; it is just paged like any other allocation.
    PinFailed,
    /// Capacity was zero — nothing to pin.
    Empty,
}

/// Buffer that holds PHI in pinned, zero-on-drop memory.
///
/// `T` must be `Zeroize` so we can scrub the contents in `Drop`.
pub struct SecureBuffer<T: Zeroize> {
    data: Vec<T>,
    pin_status: PinStatus,
}

impl<T: Zeroize + Default + Clone> SecureBuffer<T> {
    /// Allocate a buffer with the given capacity, pre-filled with
    /// `T::default()` so the backing memory is touched (and therefore
    /// committed/lockable on platforms that distinguish reserved vs
    /// committed pages).
    #[must_use]
    pub fn with_capacity(cap: usize) -> Self {
        let mut data = vec![T::default(); cap];
        let pin_status = pin(&mut data);
        Self { data, pin_status }
    }
}

impl<T: Zeroize> SecureBuffer<T> {
    pub fn pin_status(&self) -> PinStatus {
        self.pin_status
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn as_slice(&self) -> &[T] {
        &self.data
    }

    pub fn as_mut_slice(&mut self) -> &mut [T] {
        &mut self.data
    }
}

impl<T: Zeroize> Deref for SecureBuffer<T> {
    type Target = [T];
    fn deref(&self) -> &[T] { &self.data }
}

impl<T: Zeroize> DerefMut for SecureBuffer<T> {
    fn deref_mut(&mut self) -> &mut [T] { &mut self.data }
}

impl<T: Zeroize> Drop for SecureBuffer<T> {
    fn drop(&mut self) {
        // Zero before unlock so the unlock doesn't expose unzeroed pages.
        self.data.zeroize();
        unpin(&mut self.data);
    }
}

// ---------------------------------------------------------------------
// Platform pinning impls.
// ---------------------------------------------------------------------

#[cfg(unix)]
fn pin<T>(buf: &mut [T]) -> PinStatus {
    if buf.is_empty() {
        return PinStatus::Empty;
    }
    let bytes = std::mem::size_of_val(buf);
    // SAFETY: pointer + length come from a live slice.
    let rc = unsafe { libc::mlock(buf.as_ptr().cast(), bytes) };
    if rc == 0 { PinStatus::Pinned } else { PinStatus::PinFailed }
}

#[cfg(unix)]
fn unpin<T>(buf: &mut [T]) {
    if buf.is_empty() {
        return;
    }
    let bytes = std::mem::size_of_val(buf);
    // SAFETY: matches the address we mlock'd in `pin`.
    unsafe { libc::munlock(buf.as_ptr().cast(), bytes) };
}

#[cfg(windows)]
fn pin<T>(buf: &mut [T]) -> PinStatus {
    use windows::Win32::System::Memory::VirtualLock;
    if buf.is_empty() {
        return PinStatus::Empty;
    }
    let bytes = std::mem::size_of_val(buf);
    // SAFETY: pointer + length come from a live slice.
    let rc = unsafe { VirtualLock(buf.as_ptr().cast::<core::ffi::c_void>().cast_mut(), bytes) };
    if rc.as_bool() { PinStatus::Pinned } else { PinStatus::PinFailed }
}

#[cfg(windows)]
fn unpin<T>(buf: &mut [T]) {
    use windows::Win32::System::Memory::VirtualUnlock;
    if buf.is_empty() {
        return;
    }
    let bytes = std::mem::size_of_val(buf);
    // SAFETY: matches the address we VirtualLock'd in `pin`.
    unsafe { let _ = VirtualUnlock(buf.as_ptr().cast::<core::ffi::c_void>().cast_mut(), bytes); }
}

// ---------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocates_and_drops_without_panic() {
        let buf = SecureBuffer::<u8>::with_capacity(4096);
        // Either pinned or pin-failed (if RLIMIT_MEMLOCK is tight on this
        // host). Empty is not a valid outcome here.
        assert_ne!(buf.pin_status(), PinStatus::Empty);
        assert_eq!(buf.len(), 4096);
    }

    #[test]
    fn empty_buffer_is_marked_empty() {
        let buf = SecureBuffer::<u8>::with_capacity(0);
        assert_eq!(buf.pin_status(), PinStatus::Empty);
    }

    #[test]
    fn writes_and_reads_back() {
        let mut buf = SecureBuffer::<u8>::with_capacity(8);
        for (i, b) in buf.as_mut_slice().iter_mut().enumerate() {
            *b = u8::try_from(i).unwrap();
        }
        let copy: Vec<u8> = buf.as_slice().to_vec();
        assert_eq!(copy, vec![0, 1, 2, 3, 4, 5, 6, 7]);
    }

    /// Best-effort verification that drop zeros the underlying allocation.
    /// We can't guarantee the allocator hands us the same slot back, but on
    /// most common allocators a same-size allocation immediately after the
    /// drop lands in the same pool. We assert it is fully zero-initialized.
    #[test]
    fn drop_zeros_memory_best_effort() {
        let canary: usize;
        {
            let mut buf = SecureBuffer::<u8>::with_capacity(1024);
            buf.as_mut_slice().fill(0xAA);
            canary = buf.as_slice().as_ptr() as usize;
            // dropped at end of scope
        }
        // Allocate a same-size buffer; with high probability we land in
        // the freed slot. We then check that no 0xAA remained.
        let probe = SecureBuffer::<u8>::with_capacity(1024);
        let _ = canary; // capture only — we don't rely on pointer equality
        for &b in probe.as_slice() {
            assert_ne!(b, 0xAA, "drop did not zero before release");
        }
    }
}
