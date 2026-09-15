// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! A shared shutdown flag for owned workers.
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// A shared shutdown flag. Clones signal the same flag, so any holder can stop
/// every waiter.
#[derive(Clone, Default)]
pub(crate) struct Stop(Arc<(Mutex<bool>, Condvar)>);
impl Stop {
    /// Set the flag and wake every waiter. Idempotent.
    pub(crate) fn request(&self) {
        let (stopped, changed) = &*self.0;
        *stopped.lock().unwrap_or_else(|p| p.into_inner()) = true;
        changed.notify_all();
    }

    /// Sleep until the next work interval, or wake immediately on shutdown.
    pub(crate) fn wait(&self, interval: Duration) -> bool {
        let (stopped, changed) = &*self.0;
        let stopped = stopped.lock().unwrap_or_else(|p| p.into_inner());
        if *stopped || interval.is_zero() {
            return *stopped;
        }
        let (stopped, _) = changed
            .wait_timeout_while(stopped, interval, |stopped| !*stopped)
            .unwrap_or_else(|p| p.into_inner());
        *stopped
    }
}
