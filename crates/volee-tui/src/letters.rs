//! Jump letters: `'` then a letter selects an agent.
//!
//! Each agent gets a letter the first time the interface sees it and keeps
//! it while it exists, so muscle memory works. Letters are mnemonic when
//! possible: the initials of the name's words first (`invoice-page` tries
//! `i` then `p`), then its other letters, then any free letter.

use std::collections::{BTreeMap, HashMap};

use volee_core::AgentId;

/// The letters in use.
#[derive(Clone, Debug, Default)]
pub(crate) struct Letters {
    by_agent: HashMap<AgentId, char>,
    by_letter: BTreeMap<char, AgentId>,
}

impl Letters {
    /// Gives `agent` a letter, unless it has one or none is left.
    pub(crate) fn assign(&mut self, agent: AgentId, name: &str) {
        if self.by_agent.contains_key(&agent) {
            return;
        }
        let initials = name
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter_map(|word| word.chars().next());
        let candidates = initials
            .chain(name.chars())
            .filter(char::is_ascii_alphabetic)
            .map(|c| c.to_ascii_lowercase())
            .chain('a'..='z')
            .chain('A'..='Z');
        let mut candidates = candidates;
        if let Some(letter) = candidates.find(|letter| !self.by_letter.contains_key(letter)) {
            self.by_agent.insert(agent, letter);
            self.by_letter.insert(letter, agent);
        }
    }

    /// Frees the letter of an agent that is gone.
    pub(crate) fn release(&mut self, agent: AgentId) {
        if let Some(letter) = self.by_agent.remove(&agent) {
            self.by_letter.remove(&letter);
        }
    }

    /// The letter of an agent.
    pub(crate) fn of(&self, agent: AgentId) -> Option<char> {
        self.by_agent.get(&agent).copied()
    }

    /// The agent of a letter.
    pub(crate) fn agent(&self, letter: char) -> Option<AgentId> {
        self.by_letter.get(&letter).copied()
    }
}

#[cfg(test)]
mod tests {
    use ulid::Ulid;

    use super::*;

    fn id(n: u128) -> AgentId {
        AgentId::from_ulid(Ulid::from_parts(1, n))
    }

    #[test]
    fn letters_prefer_initials_then_name_letters() {
        let mut letters = Letters::default();
        letters.assign(id(1), "tower");
        letters.assign(id(2), "invoicing");
        letters.assign(id(3), "invoice-schema");
        letters.assign(id(4), "invoice-tools");
        letters.assign(id(5), "invoice-page");
        assert_eq!(letters.of(id(1)), Some('t'));
        assert_eq!(letters.of(id(2)), Some('i'));
        assert_eq!(letters.of(id(3)), Some('s'));
        assert_eq!(letters.of(id(4)), Some('n'));
        assert_eq!(letters.of(id(5)), Some('p'));
        assert_eq!(letters.agent('s'), Some(id(3)));
    }

    #[test]
    fn letters_are_stable_and_reused_after_release() {
        let mut letters = Letters::default();
        letters.assign(id(1), "alpha");
        letters.assign(id(1), "renamed");
        assert_eq!(letters.of(id(1)), Some('a'));
        letters.release(id(1));
        assert_eq!(letters.agent('a'), None);
        letters.assign(id(2), "atlas");
        assert_eq!(letters.of(id(2)), Some('a'));
    }

    #[test]
    fn letters_run_out_after_fifty_two_agents() {
        let mut letters = Letters::default();
        for n in 0..60 {
            letters.assign(id(n), "same");
        }
        assert!(letters.of(id(51)).is_some());
        assert_eq!(letters.of(id(52)), None);
    }
}
