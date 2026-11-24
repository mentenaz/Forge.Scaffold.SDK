//! Node version ranges, as written in `engines.node` of a `package.json`, and the
//! check the host makes before it installs or builds: "this SPFx version needs
//! Node 18, you have Node 22".
//!
//! The engine does not run `node -v` (it runs no commands): the host finds the
//! installed version and asks [`check_node`]. A manifest carries the range of its
//! template in `requires.node`, and a detected solution has the range of its own
//! `package.json` in [`crate::SpfxSolution::node_range`].
//!
//! This is the small part of npm's semver ranges that `engines` fields use:
//! comparators `>= > <= < =`, `^`, `~`, partial versions and `x`/`*` wildcards,
//! several comparators separated by spaces (all must hold), and alternatives
//! separated by `||` (any may hold). A space between an operator and its version
//! (`< 23.0.0`, as in some SPFx templates) is accepted. Pre-release and build
//! parts of a version are ignored. Hyphen ranges (`1.2 - 1.4`) are not supported
//! and are an error, not a silent mismatch.

use crate::error::{Result, ScaffoldError};

type Version = (u64, u64, u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    /// at least this version
    Ge,
    /// below this version
    Lt,
}

#[derive(Debug, Clone, Copy)]
struct Comparator {
    op: Op,
    version: Version,
}

fn bad(msg: impl Into<String>) -> ScaffoldError {
    ScaffoldError::Manifest(msg.into())
}

/// `22.23.0`, `v22.23.0`, `22.23.0-nightly.1` to `(22, 23, 0)`.
fn parse_version(text: &str) -> Option<Version> {
    let core = text.trim().trim_start_matches('v').split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    parts.next().is_none().then_some((major, minor, patch))
}

/// A version that may stop early or use wildcards: `18`, `18.x`, `18.17`, `*`.
#[derive(Debug, Clone, Copy)]
struct Partial {
    major: Option<u64>,
    minor: Option<u64>,
    patch: Option<u64>,
}

fn parse_partial(text: &str) -> Option<Partial> {
    let core = text.trim().trim_start_matches('v').split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let mut next = |first: bool| -> Option<Option<u64>> {
        match parts.next() {
            None => Some(None),
            Some("x" | "X" | "*") => Some(None),
            Some("") => (first && core.is_empty()).then_some(None),
            Some(p) => p.parse().ok().map(Some),
        }
    };
    let major = next(true)?;
    let minor = next(false)?;
    let patch = next(false)?;
    let shaped = (major.is_some() || minor.is_none()) && (minor.is_some() || patch.is_none());
    (parts.next().is_none() && shaped).then_some(Partial { major, minor, patch })
}

fn cmp(op: Op, version: Version) -> Comparator {
    Comparator { op, version }
}

/// Turns one operator and one (possibly partial) version into comparators.
fn expand(op: &str, text: &str) -> std::result::Result<Vec<Comparator>, String> {
    let p = parse_partial(text).ok_or_else(|| format!("'{text}' is not a version"))?;
    let (ma, mi, pa) = (p.major, p.minor, p.patch);
    let low = (ma.unwrap_or(0), mi.unwrap_or(0), pa.unwrap_or(0));
    // the first version that is NOT covered by this partial
    let upper = match (ma, mi, pa) {
        (None, _, _) => None,
        (Some(m), None, _) => Some((m + 1, 0, 0)),
        (Some(m), Some(n), None) => Some((m, n + 1, 0)),
        (Some(m), Some(n), Some(k)) => Some((m, n, k + 1)),
    };
    Ok(match op {
        "" | "=" => {
            let mut out = vec![cmp(Op::Ge, low)];
            out.extend(upper.map(|u| cmp(Op::Lt, u)));
            out
        }
        ">=" => vec![cmp(Op::Ge, low)],
        ">" => match upper {
            Some(u) => vec![cmp(Op::Ge, u)],
            None => vec![cmp(Op::Lt, (0, 0, 0))], // nothing is greater than *
        },
        "<" => vec![cmp(Op::Lt, low)],
        "<=" => match upper {
            Some(u) => vec![cmp(Op::Lt, u)],
            None => vec![],
        },
        "~" => {
            let mut out = vec![cmp(Op::Ge, low)];
            out.push(cmp(
                Op::Lt,
                match (ma, mi) {
                    (None, _) => return Ok(vec![]),
                    (Some(m), None) => (m + 1, 0, 0),
                    (Some(m), Some(n)) => (m, n + 1, 0),
                },
            ));
            out
        }
        "^" => {
            let upper = match (ma, mi, pa) {
                (None, _, _) => return Ok(vec![]),
                (Some(0), Some(0), Some(k)) => (0, 0, k + 1),
                (Some(0), Some(n), _) if n > 0 || mi.is_some() => (0, n + 1, 0),
                (Some(m), _, _) => (m + 1, 0, 0),
            };
            vec![cmp(Op::Ge, low), cmp(Op::Lt, upper)]
        }
        other => return Err(format!("unknown operator '{other}'")),
    })
}

fn parse_set(text: &str) -> std::result::Result<Vec<Comparator>, String> {
    // Glue a lone operator to the version after it: `< 23.0.0` becomes `<23.0.0`.
    let mut words: Vec<String> = Vec::new();
    let mut pending = String::new();
    for word in text.split_whitespace() {
        if word == "-" {
            return Err("hyphen ranges are not supported".into());
        }
        if word.chars().all(|c| matches!(c, '>' | '<' | '=' | '^' | '~')) {
            pending.push_str(word);
        } else {
            words.push(format!("{pending}{word}"));
            pending.clear();
        }
    }
    if !pending.is_empty() {
        return Err(format!("operator '{pending}' has no version"));
    }
    let mut out = Vec::new();
    for word in words {
        let split = word.find(|c: char| !matches!(c, '>' | '<' | '=' | '^' | '~')).unwrap_or(word.len());
        out.extend(expand(&word[..split], &word[split..])?);
    }
    Ok(out)
}

fn parse_range(range: &str) -> std::result::Result<Vec<Vec<Comparator>>, String> {
    range.split("||").map(parse_set).collect()
}

/// Checks that `range` can be read. An empty range, `*` and `x` allow everything.
pub(crate) fn validate_range(range: &str) -> Result<()> {
    parse_range(range)
        .map(|_| ())
        .map_err(|e| bad(format!("'{range}' is not a Node version range: {e}")))
}

/// The lowest version `range` allows, as `(major, minor, patch)`: the smallest lower bound of its alternatives (`^4.2.0` is
/// 4.2.0, `>=4 <6 || ^7` is 4.0.0, `*` is 0.0.0). `None` when the range cannot be read.
pub(crate) fn lowest(range: &str) -> Option<(u64, u64, u64)> {
    let sets = parse_range(range).ok()?;
    sets.iter()
        .map(|set| set.iter().filter(|c| c.op == Op::Ge).map(|c| c.version).max().unwrap_or((0, 0, 0)))
        .min()
}

/// `1.2.3`, `v1.2.3` (a pre-release or build part is ignored) as `(major, minor, patch)`.
pub(crate) fn exact_version(text: &str) -> Option<(u64, u64, u64)> {
    parse_version(text)
}

/// Does `version` (`22.23.0`, `v22.23.0`) satisfy `range`?
pub(crate) fn allows(range: &str, version: &str) -> Result<bool> {
    let v = parse_version(version).ok_or_else(|| bad(format!("'{version}' is not a version like 22.23.0")))?;
    let sets = parse_range(range).map_err(|e| bad(format!("'{range}' is not a Node version range: {e}")))?;
    Ok(sets.iter().any(|set| {
        set.iter().all(|c| match c.op {
            Op::Ge => v >= c.version,
            Op::Lt => v < c.version,
        })
    }))
}

/// The answer to "can this run on the Node that is installed?".
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum NodeCheck {
    /// The template or solution does not say which Node it needs.
    NoRequirement,
    /// The installed Node is inside the range.
    Fine,
    /// The installed Node is outside the range the template or solution needs.
    Mismatch {
        /// The range the template or solution needs.
        required: String,
        /// The Node that is installed.
        installed: String,
    },
}

/// Compares the installed Node version (what `node -v` prints) with a requirement.
///
/// # Example
///
/// ```
/// use forge_scaffold::{check_node, NodeCheck};
///
/// // what `node -v` prints, against the range a template or solution asks for
/// assert_eq!(check_node(Some(">=22.14.0 <23"), "v22.16.0")?, NodeCheck::Fine);
/// assert_eq!(check_node(None, "v18.20.8")?, NodeCheck::NoRequirement);
/// match check_node(Some("^20.19.0 || >=22.12.0"), "v18.20.8")? {
///     NodeCheck::Mismatch { required, installed } => println!("needs {required}, have {installed}"),
///     // the enum is #[non_exhaustive]: a host always has a fallback arm
///     _ => {}
/// }
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn check_node(required: Option<&str>, installed: &str) -> Result<NodeCheck> {
    let Some(range) = required.map(str::trim).filter(|r| !r.is_empty()) else {
        return Ok(NodeCheck::NoRequirement);
    };
    Ok(if allows(range, installed)? {
        NodeCheck::Fine
    } else {
        NodeCheck::Mismatch {
            required: range.to_string(),
            installed: installed.trim().trim_start_matches('v').to_string(),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPFX_124: &str = ">=22.14.0 <23.0.0 || >=24.12.0 <25.0.0 || >=26.0.0 <27.0.0";

    #[test]
    fn the_ranges_of_the_real_spfx_templates() {
        // 1.24.1: three Node lines
        for ok in ["22.14.0", "22.23.0", "v22.99.9", "24.12.0", "24.22.1", "26.8.1"] {
            assert!(allows(SPFX_124, ok).unwrap(), "{ok}");
        }
        for no in ["22.13.9", "23.0.0", "24.11.0", "25.1.0", "27.0.0", "18.20.8"] {
            assert!(!allows(SPFX_124, no).unwrap(), "{no}");
        }
        // 1.20.0: Node 18 only
        assert!(allows(">=18.17.1 <19.0.0", "18.20.8").unwrap());
        assert!(!allows(">=18.17.1 <19.0.0", "18.17.0").unwrap());
        assert!(!allows(">=18.17.1 <19.0.0", "22.23.0").unwrap());
        // 1.21 to 1.23: a space between the operator and the version
        assert!(allows(">=22.14.0 < 23.0.0", "22.23.0").unwrap());
        assert!(!allows(">=22.14.0 < 23.0.0", "23.0.0").unwrap());
    }

    #[test]
    fn caret_tilde_and_wildcards() {
        assert!(allows("^18.17.1", "18.99.0").unwrap() && !allows("^18.17.1", "19.0.0").unwrap());
        assert!(allows("^0.2.3", "0.2.9").unwrap() && !allows("^0.2.3", "0.3.0").unwrap());
        assert!(allows("^0.0.3", "0.0.3").unwrap() && !allows("^0.0.3", "0.0.4").unwrap());
        assert!(allows("~22.14.0", "22.14.9").unwrap() && !allows("~22.14.0", "22.15.0").unwrap());
        assert!(allows("22", "22.5.1").unwrap() && !allows("22", "23.0.0").unwrap());
        assert!(allows("22.x", "22.5.1").unwrap() && !allows("22.x", "21.9.9").unwrap());
        assert!(allows("22.14", "22.14.7").unwrap() && !allows("22.14", "22.15.0").unwrap());
        assert!(allows("22.14.0", "22.14.0").unwrap() && !allows("22.14.0", "22.14.1").unwrap());
        for any in ["", "  ", "*", "x"] {
            assert!(allows(any, "1.0.0").unwrap(), "{any:?}");
        }
        assert!(allows(">22", "23.0.0").unwrap() && !allows(">22", "22.9.9").unwrap());
        assert!(allows("<=22", "22.9.9").unwrap() && !allows("<=22", "23.0.0").unwrap());
    }

    #[test]
    fn prerelease_parts_are_ignored() {
        assert!(allows(">=22.14.0 <23.0.0", "22.20.0-nightly.1").unwrap());
        assert!(allows(">=22.14.0-0 <23.0.0", "22.14.0").unwrap());
    }

    #[test]
    fn what_cannot_be_read_is_an_error_not_a_mismatch() {
        for bad in ["1.2 - 1.4", ">=", "latest", ">=22.x.1", "22..1", "@22"] {
            assert!(validate_range(bad).is_err(), "{bad}");
            assert!(allows(bad, "22.0.0").is_err(), "{bad}");
        }
        assert!(allows(">=22", "twenty-two").is_err());
    }

    #[test]
    fn the_check_the_host_makes() {
        assert_eq!(check_node(None, "v22.23.0").unwrap(), NodeCheck::NoRequirement);
        assert_eq!(check_node(Some("  "), "v22.23.0").unwrap(), NodeCheck::NoRequirement);
        assert_eq!(check_node(Some(SPFX_124), "v22.23.0\n").unwrap(), NodeCheck::Fine);
        assert_eq!(
            check_node(Some(">=18.17.1 <19.0.0"), "v22.23.0").unwrap(),
            NodeCheck::Mismatch { required: ">=18.17.1 <19.0.0".into(), installed: "22.23.0".into() }
        );
        assert!(check_node(Some("nonsense"), "22.0.0").is_err());
    }
}
