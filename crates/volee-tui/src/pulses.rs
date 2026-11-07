//! Message pulses: when a message is posted, a glyph travels along the
//! network's edge from the sender to each recipient.
//!
//! A pulse is a pure function of the message's time and the current time,
//! so drawing never changes state and a frame at a given time always looks
//! the same. Messages older than one pulse when the interface learns about
//! them (the history replayed at start) do not pulse.

use std::collections::VecDeque;

use volee_core::message::{Message, MessageKind, Party};
use volee_core::time::Timestamp;

/// How long a pulse takes from the sender to a recipient.
pub(crate) const PULSE_MILLIS: i64 = 1_000;

/// One message on its way to one recipient.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Pulse {
    /// Who sent it.
    pub(crate) from: Party,
    /// Who receives it.
    pub(crate) to: Party,
    /// When it was sent: the pulse leaves then.
    pub(crate) at: Timestamp,
    /// What it is for.
    pub(crate) kind: MessageKind,
}

impl Pulse {
    /// How far the pulse went at `now`, from 0 (it just left) to 1 (it
    /// arrived), eased so it starts and lands softly; `None` before it
    /// leaves and once it arrived.
    pub(crate) fn progress(&self, now: Timestamp) -> Option<f64> {
        let elapsed = now.as_millis() - self.at.as_millis();
        if !(0..PULSE_MILLIS).contains(&elapsed) {
            return None;
        }
        let linear = ratio(elapsed, PULSE_MILLIS);
        Some(linear * linear * (3.0 - 2.0 * linear))
    }
}

/// `part / whole` for small non-negative millisecond counts.
fn ratio(part: i64, whole: i64) -> f64 {
    let as_f64 = |value: i64| f64::from(u32::try_from(value.max(0)).unwrap_or(u32::MAX));
    as_f64(part) / as_f64(whole).max(1.0)
}

/// The pulses that are on their way or about to leave.
#[derive(Clone, Debug, Default)]
pub(crate) struct Pulses {
    pulses: VecDeque<Pulse>,
}

impl Pulses {
    /// Adds a pulse per recipient of a message the interface just learned
    /// about at `now`, unless it is too old to still be travelling.
    pub(crate) fn record(&mut self, message: &Message, now: Timestamp) {
        if message.at.plus_millis(PULSE_MILLIS) <= now {
            return;
        }
        for to in message.recipients() {
            self.pulses.push_back(Pulse {
                from: message.from,
                to,
                at: message.at,
                kind: message.kind,
            });
        }
    }

    /// Forgets the pulses that arrived before `now`.
    pub(crate) fn prune(&mut self, now: Timestamp) {
        self.pulses
            .retain(|pulse| pulse.at.plus_millis(PULSE_MILLIS) > now);
    }

    /// The pulses travelling at `now`, with how far each went.
    pub(crate) fn active(&self, now: Timestamp) -> impl Iterator<Item = (&Pulse, f64)> {
        self.pulses
            .iter()
            .filter_map(move |pulse| pulse.progress(now).map(|progress| (pulse, progress)))
    }

    /// Whether a pulse travels or waits to leave, so the screen must be
    /// redrawn soon.
    pub(crate) fn is_moving(&self, now: Timestamp) -> bool {
        self.pulses
            .iter()
            .any(|pulse| pulse.at.plus_millis(PULSE_MILLIS) > now)
    }
}

#[cfg(test)]
mod tests {
    use ulid::Ulid;
    use volee_core::message::Receipt;
    use volee_core::{AgentId, MessageId, ThreadId};

    use super::*;

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_millis(1_800_000_000_000 + millis)
    }

    fn agent(n: u128) -> Party {
        Party::Agent(AgentId::from_ulid(Ulid::from_parts(1, n)))
    }

    fn message(sent: i64, to: &[Party]) -> Message {
        Message {
            id: MessageId::from_ulid(Ulid::from_parts(2, 1)),
            thread: ThreadId::from_ulid(Ulid::from_parts(3, 1)),
            from: agent(1),
            kind: MessageKind::Question,
            body: "Is total_cents stored?".into(),
            refs: vec![],
            in_reply_to: None,
            at: at(sent),
            receipts: to
                .iter()
                .map(|party| Receipt {
                    to: *party,
                    delivered_at: None,
                    read_at: None,
                })
                .collect(),
        }
    }

    #[test]
    fn a_pulse_travels_for_one_second_and_eases() {
        let mut pulses = Pulses::default();
        pulses.record(&message(0, &[agent(2)]), at(0));
        let progress = |now| pulses.active(at(now)).map(|(_, p)| p).next();
        assert_eq!(progress(-1), None, "not left yet");
        assert!(pulses.is_moving(at(-1)), "about to leave");
        assert_eq!(progress(0), Some(0.0));
        assert_eq!(progress(500), Some(0.5));
        let early = progress(100).unwrap();
        assert!(early > 0.0 && early < 0.1, "slow start: {early}");
        let late = progress(900).unwrap();
        assert!(late > 0.9 && late < 1.0, "soft landing: {late}");
        assert_eq!(progress(PULSE_MILLIS), None, "arrived");
        assert!(!pulses.is_moving(at(PULSE_MILLIS)));
    }

    #[test]
    fn every_recipient_gets_a_pulse() {
        let mut pulses = Pulses::default();
        pulses.record(&message(0, &[agent(2), agent(3), Party::Human]), at(200));
        let targets: Vec<Party> = pulses.active(at(300)).map(|(p, _)| p.to).collect();
        assert_eq!(targets, [agent(2), agent(3), Party::Human]);
    }

    #[test]
    fn old_messages_do_not_pulse_and_arrived_ones_are_pruned() {
        let mut pulses = Pulses::default();
        pulses.record(&message(0, &[agent(2)]), at(PULSE_MILLIS));
        assert!(!pulses.is_moving(at(PULSE_MILLIS)));
        pulses.record(&message(0, &[agent(2)]), at(400));
        pulses.prune(at(999));
        assert_eq!(pulses.active(at(999)).count(), 1);
        pulses.prune(at(1_000));
        assert_eq!(pulses.active(at(500)).count(), 0);
    }
}
