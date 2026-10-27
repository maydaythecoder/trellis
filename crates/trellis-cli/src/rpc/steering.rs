//! One run's steering channel and the server's record of what was sent on
//! it, so what was not delivered is known however the run ended.

use super::{STEER_ENDED, STEER_WITHDRAWN};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex, PoisonError};
use trellis_loop::Steering;

/// One run's steering channel, and the server's own record of it.
///
/// The record exists because an aborted run returns no transcript: `abort`
/// drops the run's future, and the loop's [`trellis_loop::SteeringLog`]
/// goes with it. So the server keeps every message it sent, in order, with
/// the command id that sent it, and the run reports each request's
/// delivered steers here as it makes it (through [`Watched`]). What was
/// sent and not delivered is then known however the run ended.
///
/// Sending, recording and closing happen under one lock, so a message that
/// races the run's end is either recorded (and handed back undelivered) or
/// refused; never taken and lost.
pub(crate) struct SteeringPort {
    state: Mutex<PortState>,
    receiver: Mutex<Option<trellis_loop::SteeringRx>>,
    /// The loop's record of what became of each message, by its place in
    /// the channel's order: where `withdraw` and the drain decide which
    /// came first (ADR 0058 §3). Taken under `state` when both are.
    fates: Arc<trellis_loop::SteeringFates>,
}

struct PortState {
    /// `None` once the run has ended.
    tx: Option<tokio::sync::mpsc::UnboundedSender<Steering>>,
    sent: Vec<Sent>,
    /// Steers that have reached a model request. The channel is FIFO and
    /// the loop delivers every steer it takes before a request, so these
    /// are the first `delivered` steers in `sent`; a withdrawn steer is
    /// never delivered and leaves `sent` when withdrawn, so that holds.
    delivered: usize,
    /// How many messages have gone on the channel: the next one's place in
    /// its order, which the loop counts the same way (`SteeringFates`).
    next: u64,
}

/// A steer's answer: the step it was delivered after, or why it was not.
pub(super) type SteerAnswer = Result<usize, String>;

struct Sent {
    id: Option<String>,
    what: Steering,
    /// Its place in the channel's order.
    seq: u64,
    /// Set for a steer until it is answered.
    reply: Option<tokio::sync::oneshot::Sender<SteerAnswer>>,
}

/// A message that was sent and never delivered, as `abort` reports it.
pub(super) struct Unsent {
    pub(super) id: Option<String>,
    pub(super) what: Steering,
}

impl Unsent {
    pub(super) fn json(&self) -> Value {
        let (kind, message) = match &self.what {
            Steering::Steer(m) => ("steer", m),
            Steering::FollowUp(m) => ("follow_up", m),
        };
        json!({"id": self.id, "type": kind, "message": message})
    }
}

impl SteeringPort {
    pub(super) fn new() -> Arc<Self> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        Arc::new(Self {
            state: Mutex::new(PortState {
                tx: Some(tx),
                sent: Vec::new(),
                delivered: 0,
                next: 0,
            }),
            receiver: Mutex::new(Some(rx)),
            fates: Arc::default(),
        })
    }

    /// The record the run's loop keeps of each message's fate, given to it
    /// with the receiver.
    pub(crate) fn fates(&self) -> Arc<trellis_loop::SteeringFates> {
        self.fates.clone()
    }

    fn state(&self) -> std::sync::MutexGuard<'_, PortState> {
        // No holder can panic between two writes it depends on, so a
        // poisoned lock still guards a consistent record.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The run's end of the channel, taken once by the run that uses it.
    pub(crate) fn take_receiver(&self) -> Option<trellis_loop::SteeringRx> {
        self.receiver
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }

    /// Records and sends one message. A steer comes back with the receiver
    /// its answer arrives on; a follow-up with none.
    pub(super) fn send(
        &self,
        id: Option<&str>,
        what: Steering,
    ) -> Result<Option<tokio::sync::oneshot::Receiver<SteerAnswer>>, String> {
        let mut st = self.state();
        let Some(tx) = &st.tx else {
            // Only between a run's end and the thread moving on (the next
            // follow-up, or finishing). A `prompt` read then waits for that
            // and starts unless a follow-up did (#495), but neither message
            // says so: the client sent this one to the run that ended.
            return Err(match what {
                Steering::Steer(_) => "the run has just ended; nothing was delivered".into(),
                Steering::FollowUp(_) => {
                    "the run has just ended; nothing was queued. Send it again once \
                     `get_state` shows no run in flight"
                        .into()
                }
            });
        };
        // A send fails only once the loop has dropped its receiver, at the
        // run's end. The record below still holds the message, and the
        // close that follows hands it back.
        let _ = tx.send(what.clone());
        // Numbered under the lock the send was made under, so the numbers
        // follow the channel's order.
        let seq = st.next;
        st.next += 1;
        let (reply, answer) = match what {
            Steering::Steer(_) => {
                let (t, r) = tokio::sync::oneshot::channel();
                (Some(t), Some(r))
            }
            Steering::FollowUp(_) => (None, None),
        };
        st.sent.push(Sent {
            id: id.map(str::to_string),
            what,
            seq,
            reply,
        });
        Ok(answer)
    }

    /// Called as each model request is made, with the steers it carries.
    /// Answers each steer the first time a request carries it.
    pub(super) fn observe(&self, delivered: &[trellis_loop::Steer]) {
        let mut st = self.state();
        let from = st.delivered;
        if delivered.len() <= from {
            return;
        }
        let steers = st
            .sent
            .iter_mut()
            .filter(|s| matches!(s.what, Steering::Steer(_)));
        for (s, d) in steers.skip(from).zip(&delivered[from..]) {
            if let Some(r) = s.reply.take() {
                let _ = r.send(Ok(d.after_step));
            }
        }
        st.delivered = delivered.len();
    }

    /// Everything sent and not yet delivered, in the order received, for
    /// `get_state` (#801). It takes nothing: it names what `close` would
    /// hand back now, by the same rule, so the two must agree. A steer is
    /// delivered once the loop has marked it, before its `steer delivered`
    /// event (#808), so a client that has read the event never finds it
    /// listed.
    pub(super) fn pending(&self) -> Vec<Value> {
        let st = self.state();
        let mut out = Vec::new();
        for s in &st.sent {
            if self.delivered(s).is_some() {
                continue;
            }
            out.push(
                Unsent {
                    id: s.id.clone(),
                    what: s.what.clone(),
                }
                .json(),
            );
        }
        out
    }

    /// Ends the channel and hands back everything sent and not delivered,
    /// in the order it was received. A steer among them is answered as not
    /// delivered.
    pub(super) fn close(&self) -> Vec<Unsent> {
        let mut st = self.state();
        st.tx = None;
        let mut unsent = Vec::new();
        for mut s in std::mem::take(&mut st.sent) {
            if let Some(after_step) = self.delivered(&s) {
                // `observe` answers it as the request carrying it is made,
                // with no await between that and the mark. Answered here
                // too, so a steer the model was sent is never told it was
                // not.
                if let Some(r) = s.reply.take() {
                    let _ = r.send(Ok(after_step));
                }
                continue;
            }
            if let Some(r) = s.reply.take() {
                let _ = r.send(Err(STEER_ENDED.into()));
            }
            unsent.push(Unsent {
                id: s.id,
                what: s.what,
            });
        }
        unsent
    }

    /// The step a steer followed once the loop has put it in a request;
    /// `None` for one still waiting, and always for a follow-up, which
    /// is never delivered into the run it was sent during.
    fn delivered(&self, s: &Sent) -> Option<usize> {
        match s.what {
            Steering::Steer(_) => self.fates.delivered(s.seq),
            Steering::FollowUp(_) => None,
        }
    }

    /// Takes back the first message sent with `id` that is still waiting
    /// (ADR 0058 §3), answering a steer's `steer` as withdrawn. Decided
    /// under the loop's record, so a steer the drain reaches first is
    /// refused as delivered and one withdrawn first never reaches it.
    pub(super) fn withdraw(&self, id: &str) -> Withdrawal {
        let mut st = self.state();
        let mut delivered = false;
        let mut found = None;
        for (i, s) in st.sent.iter().enumerate() {
            if s.id.as_deref() != Some(id) {
                continue;
            }
            match self.fates.withdraw(s.seq) {
                Ok(()) => {
                    found = Some(i);
                    break;
                }
                Err(trellis_loop::WithdrawRefused::AlreadyDelivered) => delivered = true,
            }
        }
        let Some(i) = found else {
            return if delivered {
                Withdrawal::Delivered
            } else {
                Withdrawal::NotHere
            };
        };
        let mut s = st.sent.remove(i);
        if let Some(r) = s.reply.take() {
            let _ = r.send(Err(STEER_WITHDRAWN.into()));
        }
        Withdrawal::Withdrawn(s.what)
    }
}

/// What [`SteeringPort::withdraw`] found.
pub(super) enum Withdrawal {
    Withdrawn(Steering),
    /// Every message sent with the id is a steer already in a request.
    Delivered,
    /// No message sent to this run carries the id.
    NotHere,
}
