//! Semantic version parsing and version requirement matching.

use std::fmt;

/// A parsed semantic version: major.minor.patch.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SemVer {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl SemVer {
    pub fn new(major: u64, minor: u64, patch: u64) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        let parts: Vec<&str> = s.split('.').collect();
        match parts.len() {
            1 => {
                let major = parts[0]
                    .parse::<u64>()
                    .map_err(|_| format!("invalid version: '{}'", s))?;
                Ok(Self::new(major, 0, 0))
            }
            2 => {
                let major = parts[0]
                    .parse::<u64>()
                    .map_err(|_| format!("invalid version: '{}'", s))?;
                let minor = parts[1]
                    .parse::<u64>()
                    .map_err(|_| format!("invalid version: '{}'", s))?;
                Ok(Self::new(major, minor, 0))
            }
            3 => {
                let major = parts[0]
                    .parse::<u64>()
                    .map_err(|_| format!("invalid version: '{}'", s))?;
                let minor = parts[1]
                    .parse::<u64>()
                    .map_err(|_| format!("invalid version: '{}'", s))?;
                let patch = parts[2]
                    .parse::<u64>()
                    .map_err(|_| format!("invalid version: '{}'", s))?;
                Ok(Self::new(major, minor, patch))
            }
            _ => Err(format!("invalid version: '{}'", s)),
        }
    }
}

impl fmt::Display for SemVer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl PartialOrd for SemVer {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SemVer {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.major
            .cmp(&other.major)
            .then(self.minor.cmp(&other.minor))
            .then(self.patch.cmp(&other.patch))
    }
}

/// A single version comparator (e.g. `>=1.2.3`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Comparator {
    /// `>=1.2.3, <2.0.0` (caret)
    Caret(SemVer),
    /// `>=1.2.3, <1.3.0` (tilde)
    Tilde(SemVer),
    /// `=1.2.3` (exact)
    Exact(SemVer),
    /// `>=1.2.3`
    Gte(SemVer),
    /// `>1.2.3`
    Gt(SemVer),
    /// `<=1.2.3`
    Lte(SemVer),
    /// `<1.2.3`
    Lt(SemVer),
    /// `*` (any)
    Wildcard,
}

impl Comparator {
    pub fn matches(&self, ver: &SemVer) -> bool {
        match self {
            Comparator::Caret(base) => {
                if ver < base {
                    return false;
                }
                if base.major > 0 {
                    // ^1.2.3 => >=1.2.3, <2.0.0
                    ver.major == base.major
                } else if base.minor > 0 {
                    // ^0.2.3 => >=0.2.3, <0.3.0
                    ver.major == 0 && ver.minor == base.minor
                } else {
                    // ^0.0.3 => >=0.0.3, <0.0.4
                    ver.major == 0 && ver.minor == 0 && ver.patch == base.patch
                }
            }
            Comparator::Tilde(base) => {
                if ver < base {
                    return false;
                }
                // ~1.2.3 => >=1.2.3, <1.3.0
                ver.major == base.major && ver.minor == base.minor
            }
            Comparator::Exact(base) => ver == base,
            Comparator::Gte(base) => ver >= base,
            Comparator::Gt(base) => ver > base,
            Comparator::Lte(base) => ver <= base,
            Comparator::Lt(base) => ver < base,
            Comparator::Wildcard => true,
        }
    }
}

/// A version requirement, consisting of one or more comparators.
/// All comparators must match for the requirement to match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionReq {
    pub comparators: Vec<Comparator>,
}

impl VersionReq {
    /// Parse a version requirement string.
    ///
    /// Supported formats:
    /// - `"1.2.3"` — caret (default)
    /// - `"^1.2.3"` — caret (explicit)
    /// - `"~1.2.3"` — tilde
    /// - `"=1.2.3"` — exact
    /// - `">=1.0, <2.0"` — range (comma-separated)
    /// - `"*"` — any version
    pub fn parse(s: &str) -> Result<Self, String> {
        let s = s.trim();
        if s == "*" {
            return Ok(Self {
                comparators: vec![Comparator::Wildcard],
            });
        }

        // Split on commas for range requirements
        let parts: Vec<&str> = s.split(',').map(|p| p.trim()).collect();
        let mut comparators = Vec::new();

        for part in parts {
            comparators.push(parse_single_comparator(part)?);
        }

        Ok(Self { comparators })
    }

    /// Check if a version matches this requirement.
    pub fn matches(&self, ver: &SemVer) -> bool {
        self.comparators.iter().all(|c| c.matches(ver))
    }
}

impl fmt::Display for VersionReq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let parts: Vec<String> = self
            .comparators
            .iter()
            .map(|c| match c {
                Comparator::Caret(v) => format!("^{}", v),
                Comparator::Tilde(v) => format!("~{}", v),
                Comparator::Exact(v) => format!("={}", v),
                Comparator::Gte(v) => format!(">={}", v),
                Comparator::Gt(v) => format!(">{}", v),
                Comparator::Lte(v) => format!("<={}", v),
                Comparator::Lt(v) => format!("<{}", v),
                Comparator::Wildcard => "*".to_string(),
            })
            .collect();
        write!(f, "{}", parts.join(", "))
    }
}

fn parse_single_comparator(s: &str) -> Result<Comparator, String> {
    let s = s.trim();

    if s == "*" {
        return Ok(Comparator::Wildcard);
    }

    if let Some(rest) = s.strip_prefix("^") {
        let ver = SemVer::parse(rest.trim())?;
        return Ok(Comparator::Caret(ver));
    }
    if let Some(rest) = s.strip_prefix("~") {
        let ver = SemVer::parse(rest.trim())?;
        return Ok(Comparator::Tilde(ver));
    }
    if let Some(rest) = s.strip_prefix(">=") {
        let ver = SemVer::parse(rest.trim())?;
        return Ok(Comparator::Gte(ver));
    }
    if let Some(rest) = s.strip_prefix(">") {
        let ver = SemVer::parse(rest.trim())?;
        return Ok(Comparator::Gt(ver));
    }
    if let Some(rest) = s.strip_prefix("<=") {
        let ver = SemVer::parse(rest.trim())?;
        return Ok(Comparator::Lte(ver));
    }
    if let Some(rest) = s.strip_prefix("<") {
        let ver = SemVer::parse(rest.trim())?;
        return Ok(Comparator::Lt(ver));
    }
    if let Some(rest) = s.strip_prefix("=") {
        let ver = SemVer::parse(rest.trim())?;
        return Ok(Comparator::Exact(ver));
    }

    // Bare version string: treat as caret
    let ver = SemVer::parse(s)?;
    Ok(Comparator::Caret(ver))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_semver_parse() {
        assert_eq!(SemVer::parse("1.2.3").unwrap(), SemVer::new(1, 2, 3));
        assert_eq!(SemVer::parse("0.1.0").unwrap(), SemVer::new(0, 1, 0));
        assert_eq!(SemVer::parse("1.0").unwrap(), SemVer::new(1, 0, 0));
        assert_eq!(SemVer::parse("2").unwrap(), SemVer::new(2, 0, 0));
    }

    #[test]
    fn test_semver_ordering() {
        assert!(SemVer::new(1, 0, 0) < SemVer::new(2, 0, 0));
        assert!(SemVer::new(1, 1, 0) < SemVer::new(1, 2, 0));
        assert!(SemVer::new(1, 2, 3) < SemVer::new(1, 2, 4));
        assert!(SemVer::new(0, 9, 9) < SemVer::new(1, 0, 0));
    }

    #[test]
    fn test_caret_default() {
        let req = VersionReq::parse("1.2.3").unwrap();
        assert!(req.matches(&SemVer::new(1, 2, 3)));
        assert!(req.matches(&SemVer::new(1, 9, 0)));
        assert!(!req.matches(&SemVer::new(2, 0, 0)));
        assert!(!req.matches(&SemVer::new(1, 2, 2)));
    }

    #[test]
    fn test_caret_explicit() {
        let req = VersionReq::parse("^1.2").unwrap();
        assert!(req.matches(&SemVer::new(1, 2, 0)));
        assert!(req.matches(&SemVer::new(1, 9, 9)));
        assert!(!req.matches(&SemVer::new(2, 0, 0)));
    }

    #[test]
    fn test_caret_pre_1_0() {
        // ^0.1.0 => >=0.1.0, <0.2.0
        let req = VersionReq::parse("^0.1.0").unwrap();
        assert!(req.matches(&SemVer::new(0, 1, 0)));
        assert!(req.matches(&SemVer::new(0, 1, 9)));
        assert!(!req.matches(&SemVer::new(0, 2, 0)));

        // ^0.0.3 => >=0.0.3, <0.0.4
        let req = VersionReq::parse("^0.0.3").unwrap();
        assert!(req.matches(&SemVer::new(0, 0, 3)));
        assert!(!req.matches(&SemVer::new(0, 0, 4)));
    }

    #[test]
    fn test_tilde() {
        let req = VersionReq::parse("~1.2.3").unwrap();
        assert!(req.matches(&SemVer::new(1, 2, 3)));
        assert!(req.matches(&SemVer::new(1, 2, 9)));
        assert!(!req.matches(&SemVer::new(1, 3, 0)));
    }

    #[test]
    fn test_exact() {
        let req = VersionReq::parse("=1.2.3").unwrap();
        assert!(req.matches(&SemVer::new(1, 2, 3)));
        assert!(!req.matches(&SemVer::new(1, 2, 4)));
    }

    #[test]
    fn test_range() {
        let req = VersionReq::parse(">=1.0, <2.0").unwrap();
        assert!(req.matches(&SemVer::new(1, 0, 0)));
        assert!(req.matches(&SemVer::new(1, 9, 9)));
        assert!(!req.matches(&SemVer::new(0, 9, 9)));
        assert!(!req.matches(&SemVer::new(2, 0, 0)));
    }

    #[test]
    fn test_wildcard() {
        let req = VersionReq::parse("*").unwrap();
        assert!(req.matches(&SemVer::new(0, 0, 0)));
        assert!(req.matches(&SemVer::new(99, 99, 99)));
    }

    #[test]
    fn test_display() {
        assert_eq!(SemVer::new(1, 2, 3).to_string(), "1.2.3");
        assert_eq!(VersionReq::parse("^1.2.3").unwrap().to_string(), "^1.2.3");
        assert_eq!(VersionReq::parse("*").unwrap().to_string(), "*");
    }
}
