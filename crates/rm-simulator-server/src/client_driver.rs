// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>

use super::{ClientInbox, ClientMessage, QueuedCommand, Welcome};
use crate::pacing::Datagram;
use crate::udp_codec::{ClientCodec, ClientEvent, MAX_INPUT_FRAMES};
use std::io;
use std::sync::{Arc, mpsc};
use std::time::Instant;

#[derive(Clone, Copy, Default)]
pub(super) struct PumpBudget {
    pub(super) commands: Option<usize>,
    pub(super) packets: Option<usize>,
}

pub(super) struct ClientDriver {
    codec: ClientCodec,
    inbox: Arc<ClientInbox>,
    commands: mpsc::Receiver<Option<QueuedCommand>>,
    budget: PumpBudget,
    seat: Option<u32>,
}

impl ClientDriver {
    pub(super) fn new(
        epoch: Instant,
        rate_bytes_per_s: u32,
        raw: bool,
        inbox: Arc<ClientInbox>,
        commands: mpsc::Receiver<Option<QueuedCommand>>,
        budget: PumpBudget,
    ) -> Self {
        Self {
            codec: ClientCodec::new(epoch, rate_bytes_per_s, MAX_INPUT_FRAMES, raw),
            inbox,
            commands,
            budget,
            seat: None,
        }
    }

    pub(super) fn receive(&mut self, payload: &[u8], now: Instant) -> io::Result<Option<Welcome>> {
        let started = self.inbox.time.now();
        let decoded = self.codec.receive(payload, now);
        self.inbox
            .observer
            .work("client_decode", self.inbox.time.since(started));
        match decoded? {
            Some(ClientEvent::Welcome(welcome)) => {
                self.seat = Some(welcome.client_id);
                Ok(Some(*welcome))
            }
            Some(ClientEvent::Anchor(anchor)) => {
                self.inbox
                    .data
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .owner_anchor = Some(*anchor);
                Ok(None)
            }
            Some(ClientEvent::Message(message)) => {
                self.inbox.publish(message)?;
                Ok(None)
            }
            None => Ok(None),
        }
    }

    pub(super) fn stats(&self) -> crate::network_stats::TransportStats {
        self.codec.stats()
    }

    pub(super) fn pinned_baselines(&self) -> usize {
        self.codec.pinned_baselines()
    }

    pub(super) fn submit_pending(
        &mut self,
        mut now: impl FnMut() -> Instant,
        mut before_submit: impl FnMut() -> io::Result<()>,
    ) -> io::Result<bool> {
        self.codec.acknowledge(now())?;
        for _ in 0..self.budget.commands.unwrap_or(usize::MAX) {
            let queued = match self.commands.try_recv() {
                Ok(Some(queued)) => queued,
                Ok(None) | Err(mpsc::TryRecvError::Disconnected) => return Ok(false),
                Err(mpsc::TryRecvError::Empty) => break,
            };
            before_submit()?;
            if let Some(command) = queued.command {
                self.inbox.observer.client(
                    "dequeue",
                    self.seat,
                    &ClientMessage::Command(command),
                    None,
                );
            }
            let started = now();
            let result = self.codec.submit(queued, started);
            self.inbox
                .observer
                .work("client_encode", self.inbox.time.since(started));
            result?;
        }
        Ok(true)
    }

    pub(super) fn send_pending(
        &mut self,
        mut now: impl FnMut() -> Instant,
        mut send: impl FnMut(Datagram) -> io::Result<()>,
    ) -> io::Result<()> {
        for _ in 0..self.budget.packets.unwrap_or(usize::MAX) {
            let Some(packet) = self.codec.next(now())? else {
                break;
            };
            send(packet)?;
        }
        Ok(())
    }
}
