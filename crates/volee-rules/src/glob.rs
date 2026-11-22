//! The path globs the rules use.
//!
//! `*` matches any run of characters inside one path segment, `?` one
//! character, `**` any number of whole segments (including none). A pattern
//! without a `/` matches the file name at any depth (`Cargo.lock` matches
//! `crates/a/Cargo.lock`). A pattern ending in `/` matches everything below
//! that directory. Matching is case-sensitive and a leading `./` is ignored.

/// Whether `path` matches `pattern`.
pub fn glob_match(pattern: &str, path: &str) -> bool {
    let pattern = pattern.strip_prefix("./").unwrap_or(pattern);
    let path = path.strip_prefix("./").unwrap_or(path);
    let segments: Vec<&str> = path.split('/').collect();
    if !pattern.contains('/') {
        return segments
            .last()
            .is_some_and(|name| segment_match(pattern, name));
    }
    let mut pat: Vec<&str> = pattern.split('/').collect();
    if pat.last() == Some(&"") {
        pat.pop();
        pat.push("**");
    }
    segments_match(&pat, &segments)
}

fn segments_match(pat: &[&str], path: &[&str]) -> bool {
    match pat.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => (0..=path.len()).any(|skip| segments_match(rest, &path[skip..])),
        Some((first, rest)) => path
            .split_first()
            .is_some_and(|(head, tail)| segment_match(first, head) && segments_match(rest, tail)),
    }
}

fn segment_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) && p[pi] != '*' {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ti));
            pi += 1;
        } else if let Some((sp, st)) = star {
            pi = sp + 1;
            ti = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::glob_match as m;

    #[test]
    fn a_pattern_without_slash_matches_the_name_at_any_depth() {
        assert!(m("Cargo.lock", "Cargo.lock"));
        assert!(m("Cargo.lock", "crates/a/Cargo.lock"));
        assert!(m("*.sql", "db/x/init.sql"));
        assert!(!m("Cargo.lock", "Cargo.lock.bak"));
    }

    #[test]
    fn double_star_spans_whole_segments_including_none() {
        assert!(m("src/**", "src/a.rs"));
        assert!(m("src/**", "src/a/b/c.rs"));
        assert!(m("**/migrations/**", "migrations/1.sql"));
        assert!(m("**/migrations/**", "app/db/migrations/1.sql"));
        assert!(!m("src/**", "lib/a.rs"));
        assert!(m("src/**/mod.rs", "src/mod.rs"));
        assert!(m("src/**/mod.rs", "src/a/b/mod.rs"));
    }

    #[test]
    fn single_star_stays_in_its_segment() {
        assert!(m("src/*.rs", "src/a.rs"));
        assert!(!m("src/*.rs", "src/a/b.rs"));
        assert!(m("src/a?.rs", "src/ab.rs"));
        assert!(!m("src/a?.rs", "src/a.rs"));
        assert!(m("src/*test*", "src/my_test_x"));
    }

    #[test]
    fn a_trailing_slash_means_everything_below() {
        assert!(m("docs/", "docs/a/b.md"));
        assert!(!m("docs/", "doc/a.md"));
        assert!(m("./docs/", "./docs/a.md"));
    }
}
