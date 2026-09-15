// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Bounded peer output with replaceable periodic state.
use crate::host::Outbound;
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};

/// One queued message and whether a newer one may take its place.
struct Frame {
    message: Arc<Outbound>,
    /// Marked for periodic state only, never for a confirmation or reliable
    /// message.
    replaceable: bool,
}

/// Queue contents and the liveness of both ends.
struct State {
    queue: VecDeque<Frame>,
    capacity: usize,
    senders: usize,
    receiver: bool,
}

/// The mutex and the wakeup that guards it.
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

/// Producer half of the outbox. A host worker and every clone of it push into
/// the same queue.
pub(crate) struct Sender {
    shared: Arc<Shared>,
}

/// Consumer half of the outbox. The socket writer drains it, so the host worker
/// never blocks on a slow peer.
pub(crate) struct Receiver {
    shared: Arc<Shared>,
}

/// Create a bounded outbox holding at most `capacity` frames, and panic unless
/// `capacity` is positive.
///
/// A push drops the replaceable tail first when the new frame is itself
/// replaceable or when the queue is full. Therefore periodic state can never
/// queue behind another periodic frame, a reliable frame can always evict an
/// unsent periodic one, and a confirmation or reliable frame is never replaced.
/// A push that cannot make room fails, so a slow consumer cannot grow the
/// queue; the peer path treats that failure as a disconnect.
pub(crate) fn channel(capacity: usize) -> (Sender, Receiver) {
    assert!(capacity > 0, "outbox capacity must be positive");
    let shared = Arc::new(Shared {
        state: Mutex::new(State {
            queue: VecDeque::with_capacity(capacity),
            capacity,
            senders: 1,
            receiver: true,
        }),
        changed: Condvar::new(),
    });
    (
        Sender {
            shared: shared.clone(),
        },
        Receiver { shared },
    )
}

impl Clone for Sender {
    /// Count one more sender, which keeps the channel open.
    fn clone(&self) -> Self {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .senders += 1;
        Self {
            shared: self.shared.clone(),
        }
    }
}

impl Sender {
    /// Enqueue in order. A replaceable frame may replace only the replaceable
    /// tail, so it can never cross a reliable confirmation barrier.
    pub(crate) fn push(&self, message: Arc<Outbound>, replaceable: bool) -> Result<(), ()> {
        let mut state = self.shared.state.lock().unwrap_or_else(|p| p.into_inner());
        if !state.receiver {
            return Err(());
        }
        if state.queue.back().is_some_and(|frame| frame.replaceable)
            && (replaceable || state.queue.len() == state.capacity)
        {
            // Replace periodic state, or make room for a reliable message.
            state.queue.pop_back();
        }
        if state.queue.len() == state.capacity {
            return Err(());
        }
        state.queue.push_back(Frame {
            message,
            replaceable,
        });
        drop(state);
        self.shared.changed.notify_one();
        Ok(())
    }
}

impl Drop for Sender {
    /// Count one fewer sender and wake a blocked receiver when the last one goes.
    fn drop(&mut self) {
        let mut state = self.shared.state.lock().unwrap_or_else(|p| p.into_inner());
        state.senders -= 1;
        let closed = state.senders == 0;
        drop(state);
        if closed {
            self.shared.changed.notify_one();
        }
    }
}

impl Receiver {
    /// Reactor-side dequeue, with no wait on an empty queue.
    pub(crate) fn try_recv(&self) -> Option<Arc<Outbound>> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .queue
            .pop_front()
            .map(|frame| frame.message)
    }

    /// Block until one message is available. `None` means every sender dropped
    /// and the queue is empty. Production drains with [`Receiver::try_recv`]
    /// from a polling reactor; this is the blocking form the tests exercise.
    #[cfg(test)]
    pub(crate) fn recv(&self) -> Option<Arc<Outbound>> {
        let mut state = self.shared.state.lock().unwrap_or_else(|p| p.into_inner());
        loop {
            if let Some(frame) = state.queue.pop_front() {
                return Some(frame.message);
            }
            if state.senders == 0 {
                return None;
            }
            state = self
                .shared
                .changed
                .wait(state)
                .unwrap_or_else(|p| p.into_inner());
        }
    }

    #[cfg(test)]
    pub(crate) fn queued(&self) -> usize {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .queue
            .len()
    }
}

impl Drop for Receiver {
    fn drop(&mut self) {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .receiver = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::ServerMessage;

    fn notice(text: &str) -> Arc<Outbound> {
        Arc::new(Outbound::new(ServerMessage::Notice(text.into())))
    }

    #[test]
    fn periodic_tail_is_replaced_before_capacity() {
        let (sender, receiver) = channel(2);
        sender.push(notice("reliable"), false).unwrap();
        for index in 0..10_000 {
            sender
                .push(notice(&format!("state {index}")), true)
                .unwrap();
        }
        assert_eq!(receiver.queued(), 2);
        assert!(
            matches!(receiver.recv().unwrap().message(), ServerMessage::Notice(s) if s == "reliable")
        );
        assert!(
            matches!(receiver.recv().unwrap().message(), ServerMessage::Notice(s) if s == "state 9999")
        );
    }

    #[test]
    fn reliable_frames_are_ordered_barriers() {
        let (sender, receiver) = channel(5);
        sender.push(notice("periodic 1"), true).unwrap();
        sender.push(notice("snapshot barrier"), false).unwrap();
        sender.push(notice("pong"), false).unwrap();
        sender.push(notice("periodic 2"), true).unwrap();
        sender.push(notice("periodic 3"), true).unwrap();
        for expected in ["periodic 1", "snapshot barrier", "pong", "periodic 3"] {
            assert!(
                matches!(receiver.recv().unwrap().message(), ServerMessage::Notice(s) if s == expected)
            );
        }
    }

    #[test]
    fn reliable_overflow_fails_but_can_evict_obsolete_tail() {
        let (sender, receiver) = channel(2);
        sender.push(notice("reliable 1"), false).unwrap();
        sender.push(notice("periodic"), true).unwrap();
        sender.push(notice("reliable 2"), false).unwrap();
        assert!(sender.push(notice("overflow"), false).is_err());
        assert_eq!(receiver.queued(), 2);
    }

    #[test]
    fn concurrent_receiver_drains_then_wakes_when_last_sender_drops() {
        let (sender, receiver) = channel(2);
        let producer = sender.clone();
        let consumer = std::thread::spawn(move || {
            assert!(
                matches!(receiver.recv().unwrap().message(), ServerMessage::Notice(s) if s == "sent")
            );
            assert!(receiver.recv().is_none());
        });
        let producer = std::thread::spawn(move || {
            producer.push(notice("sent"), false).unwrap();
        });
        drop(sender);
        producer.join().unwrap();
        consumer.join().unwrap();
    }

    #[test]
    fn sender_rejects_after_receiver_is_dropped() {
        let (sender, receiver) = channel(1);
        drop(receiver);
        assert!(sender.push(notice("orphaned"), false).is_err());
    }
}
