//! Ordered event delivery with bounded queues.
//!
//! Each subscriber has a bounded queue. A subscriber that stops reading never blocks the engine: when its queue is full,
//! new events are dropped for it and counted. Nothing is lost silently: the reader gets `Lagged { missed }` in order as
//! soon as there is room, or when it has drained everything, and the event numbers (which rise by one) show the gap.
//! After `Lagged` the UI asks for a snapshot instead of replaying history.

use crate::api::EngineEvent;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    mpsc::{
        sync_channel, Receiver, RecvError, RecvTimeoutError, SyncSender, TryRecvError, TrySendError,
    },
    Arc,
};
use std::time::Duration;

pub const SUBSCRIBER_QUEUE: usize = 256;

struct Subscriber {
    tx: SyncSender<(u64, EngineEvent)>,
    missed: Arc<AtomicU64>,
}

pub struct EventHub {
    seq: u64,
    subscribers: Vec<Subscriber>,
    /// What was published since the diagnostic log last looked: (event, particulars), with nothing that identifies anyone.
    tap: Vec<(String, String)>,
    tap_on: bool,
}

impl Default for EventHub {
    fn default() -> Self {
        Self {
            seq: 0,
            subscribers: Vec::new(),
            tap: Vec::new(),
            tap_on: true,
        }
    }
}

/// The event's name and the few particulars that help (never names, texts, codes or files).
fn describe(e: &EngineEvent) -> (String, String) {
    let debug = format!("{e:?}");
    let name: String = debug
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect();
    let detail = match e {
        EngineEvent::DialRequested { peer, initiator } => {
            format!("peer={peer} initiator={initiator}")
        }
        EngineEvent::DeviceLink { stage, .. } => format!("stage={stage}"),
        EngineEvent::FileProgress { percent, .. } => format!("percent={percent}"),
        EngineEvent::MessageStatusChanged { status, .. } => format!("status={status:?}"),
        EngineEvent::MessageAdded { own, .. } => format!("own={own}"),
        EngineEvent::ContactAdded { uid }
        | EngineEvent::ContactRemoved { uid }
        | EngineEvent::InviteRequest { uid } => {
            format!("friend={uid}")
        }
        EngineEvent::SignalReceived { peer, message } => {
            let kind: String = format!("{message:?}")
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect();
            format!("peer={peer} kind={kind}")
        }
        _ => String::new(),
    };
    (name, detail)
}

/// One subscriber's view of the event stream.
#[derive(Debug)]
pub struct EventStream {
    rx: Receiver<(u64, EngineEvent)>,
    missed: Arc<AtomicU64>,
    last_seq: u64,
}

impl EventStream {
    fn lagged(&mut self) -> Option<(u64, EngineEvent)> {
        let missed = self.missed.swap(0, Ordering::SeqCst);
        (missed > 0).then_some((self.last_seq, EngineEvent::Lagged { missed }))
    }

    pub fn try_recv(&mut self) -> Result<(u64, EngineEvent), TryRecvError> {
        match self.rx.try_recv() {
            Ok(item) => {
                self.last_seq = item.0;
                Ok(item)
            }
            Err(e) => self.lagged().ok_or(e),
        }
    }

    pub fn recv(&mut self) -> Result<(u64, EngineEvent), RecvError> {
        match self.rx.recv() {
            Ok(item) => {
                self.last_seq = item.0;
                Ok(item)
            }
            Err(e) => self.lagged().ok_or(e),
        }
    }

    pub fn recv_timeout(
        &mut self,
        timeout: Duration,
    ) -> Result<(u64, EngineEvent), RecvTimeoutError> {
        match self.rx.recv_timeout(timeout) {
            Ok(item) => {
                self.last_seq = item.0;
                Ok(item)
            }
            Err(e) => self.lagged().ok_or(e),
        }
    }
}

impl std::fmt::Debug for EventHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventHub")
            .field("seq", &self.seq)
            .field("subscribers", &self.subscribers.len())
            .finish()
    }
}

impl EventHub {
    pub fn subscribe(&mut self) -> EventStream {
        let (tx, rx) = sync_channel(SUBSCRIBER_QUEUE);
        let missed = Arc::new(AtomicU64::new(0));
        self.subscribers.push(Subscriber {
            tx,
            missed: missed.clone(),
        });
        EventStream {
            rx,
            missed,
            last_seq: self.seq,
        }
    }

    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// Starts or stops noting events for the diagnostic log.
    pub fn set_tap(&mut self, on: bool) {
        self.tap_on = on;
        if !on {
            self.tap.clear();
        }
    }

    /// What was noted since the last call.
    pub fn take_tap(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.tap)
    }

    pub fn publish(&mut self, event: EngineEvent) {
        if self.tap_on && self.tap.len() < 400 {
            self.tap.push(describe(&event));
        }
        self.seq += 1;
        let seq = self.seq;
        self.subscribers.retain_mut(|s| {
            // If this subscriber fell behind and there is room again, say so first so the marker stays in order.
            let pending = s.missed.load(Ordering::SeqCst);
            if pending > 0 {
                match s
                    .tx
                    .try_send((seq, EngineEvent::Lagged { missed: pending }))
                {
                    Ok(()) => {
                        s.missed.fetch_sub(
                            pending.min(s.missed.load(Ordering::SeqCst)),
                            Ordering::SeqCst,
                        );
                    }
                    Err(TrySendError::Full(_)) => {
                        s.missed.fetch_add(1, Ordering::SeqCst);
                        return true;
                    }
                    Err(TrySendError::Disconnected(_)) => return false,
                }
            }
            match s.tx.try_send((seq, event.clone())) {
                Ok(()) => true,
                Err(TrySendError::Full(_)) => {
                    s.missed.fetch_add(1, Ordering::SeqCst);
                    true
                }
                Err(TrySendError::Disconnected(_)) => false,
            }
        });
    }
}
