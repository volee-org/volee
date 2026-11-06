//! Harness versions.

use std::fmt;
use std::str::FromStr;

/// A `major.minor.patch` version of a harness, compared numerically.
///
/// Pre-release and build suffixes (`-beta.1`, `+abc`) are ignored: volee
/// only compares versions against the minimum a feature needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    /// Incremented for incompatible changes.
    pub major: u64,
    /// Incremented for new features.
    pub minor: u64,
    /// Incremented for fixes.
    pub patch: u64,
}

impl Version {
    /// A version from its three numbers.
    pub const fn new(major: u64, minor: u64, patch: u64) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// The first version number in a banner such as `2.1.319 (Claude Code)`.
    pub fn find_in(text: &str) -> Option<Self> {
        text.split_whitespace().find_map(|word| word.parse().ok())
    }
}

/// The error returned when a string is not a `major.minor.patch` version.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("`{0}` is not a major.minor.patch version")]
pub struct ParseVersionError(String);

impl FromStr for Version {
    type Err = ParseVersionError;

    /// Parses `2.1.319`, also accepting a leading `v` and a pre-release or
    /// build suffix.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let error = || ParseVersionError(text.to_owned());
        let core = text.strip_prefix('v').unwrap_or(text);
        let core = core.split(['-', '+']).next().unwrap_or(core);
        let mut numbers = core.split('.').map(|part| {
            if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            part.parse::<u64>().ok()
        });
        match (
            numbers.next(),
            numbers.next(),
            numbers.next(),
            numbers.next(),
        ) {
            (Some(Some(major)), Some(Some(minor)), Some(Some(patch)), None) => {
                Ok(Self::new(major, minor, patch))
            }
            _ => Err(error()),
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_version_in_the_claude_banner() {
        assert_eq!(
            Version::find_in("2.1.319 (Claude Code)\n"),
            Some(Version::new(2, 1, 319))
        );
        assert_eq!(
            Version::find_in("Claude Code v2.2.0"),
            Some(Version::new(2, 2, 0))
        );
        assert_eq!(Version::find_in("no version here"), None);
        assert_eq!(Version::find_in(""), None);
    }

    #[test]
    fn parsing_ignores_suffixes_and_rejects_garbage() {
        assert_eq!("2.1.300-beta.1".parse(), Ok(Version::new(2, 1, 300)));
        assert_eq!("2.1.300+build.7".parse(), Ok(Version::new(2, 1, 300)));
        for bad in ["2.1", "2.1.3.4", "2..3", "a.b.c", "2.1.+3", "-2.1.3", ""] {
            assert!(bad.parse::<Version>().is_err(), "{bad} parsed");
        }
    }

    #[test]
    fn versions_compare_numerically() {
        assert!(Version::new(2, 1, 319) > Version::new(2, 1, 300));
        assert!(Version::new(2, 1, 99) < Version::new(2, 1, 300));
        assert!(Version::new(3, 0, 0) > Version::new(2, 99, 999));
    }

    #[test]
    fn display_round_trips() {
        let version = Version::new(2, 1, 319);
        assert_eq!(version.to_string(), "2.1.319");
        assert_eq!(version.to_string().parse(), Ok(version));
    }
}
