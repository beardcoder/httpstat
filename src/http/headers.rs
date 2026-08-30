//! A small ordered, case-insensitive header collection.
//!
//! HTTP allows a field name to repeat (`Set-Cookie` being the obvious case), so
//! headers are kept as an ordered list rather than a map; lookups compare names
//! ASCII-case-insensitively as the specification requires.

use std::fmt;

/// An ordered list of `(name, value)` pairs preserving wire order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Headers(Vec<(String, String)>);

impl Headers {
    pub fn new() -> Self {
        Headers(Vec::new())
    }

    pub fn push(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.0.push((name.into(), value.into()));
    }

    /// Append to the value of the last header, used for obsolete line folding.
    pub fn append_to_last(&mut self, continuation: &str) -> bool {
        match self.0.last_mut() {
            Some((_, value)) => {
                value.push(' ');
                value.push_str(continuation);
                true
            }
            None => false,
        }
    }

    /// The first value for `name`, or `None` when the header is absent.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// Every value for `name`, in wire order.
    pub fn get_all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.iter()
            .filter(move |(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn contains(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    /// Remove every header named `name`, returning how many were dropped.
    pub fn remove_all(&mut self, name: &str) -> usize {
        let before = self.0.len();
        self.0.retain(|(k, _)| !k.eq_ignore_ascii_case(name));
        before - self.0.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &(String, String)> {
        self.0.iter()
    }

    /// Distinct header names, in first-seen order.
    pub fn names(&self) -> Vec<&str> {
        let mut seen: Vec<&str> = Vec::new();
        for (name, _) in self.iter() {
            if !seen.iter().any(|s| s.eq_ignore_ascii_case(name)) {
                seen.push(name.as_str());
            }
        }
        seen
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<Vec<(String, String)>> for Headers {
    fn from(pairs: Vec<(String, String)>) -> Self {
        Headers(pairs)
    }
}

impl FromIterator<(String, String)> for Headers {
    fn from_iter<I: IntoIterator<Item = (String, String)>>(iter: I) -> Self {
        Headers(iter.into_iter().collect())
    }
}

impl<'a> IntoIterator for &'a Headers {
    type Item = &'a (String, String);
    type IntoIter = std::slice::Iter<'a, (String, String)>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl fmt::Display for Headers {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (name, value) in self.iter() {
            writeln!(f, "{name}: {value}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Headers {
        let mut h = Headers::new();
        h.push("Content-Type", "text/html");
        h.push("Set-Cookie", "a=1");
        h.push("set-cookie", "b=2");
        h
    }

    #[test]
    fn lookup_ignores_case_and_returns_the_first_value() {
        let h = sample();
        assert_eq!(h.get("content-TYPE"), Some("text/html"));
        assert_eq!(h.get("Set-Cookie"), Some("a=1"));
        assert_eq!(h.get("missing"), None);
        assert!(h.contains("SET-COOKIE"));
    }

    #[test]
    fn get_all_returns_every_repeat_in_order() {
        let h = sample();
        assert_eq!(h.get_all("Set-Cookie").collect::<Vec<_>>(), ["a=1", "b=2"]);
    }

    #[test]
    fn remove_all_drops_every_case_variant() {
        let mut h = sample();
        assert_eq!(h.remove_all("SET-cookie"), 2);
        assert_eq!(h.len(), 1);
        assert_eq!(h.remove_all("nope"), 0);
    }

    #[test]
    fn names_are_deduplicated_case_insensitively() {
        assert_eq!(sample().names(), ["Content-Type", "Set-Cookie"]);
    }

    #[test]
    fn append_to_last_folds_a_continuation_line() {
        let mut h = Headers::new();
        assert!(!h.append_to_last("orphan"));
        h.push("X-Long", "part one");
        assert!(h.append_to_last("part two"));
        assert_eq!(h.get("x-long"), Some("part one part two"));
    }
}
