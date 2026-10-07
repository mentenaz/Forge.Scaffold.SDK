//! Generated secrets (database passwords, JWT secrets).
//!
//! Alphanumeric only, so a secret is safe in `.env` files, connection URLs and
//! YAML without quoting. Every secret contains at least one upper-case letter,
//! one lower-case letter and one digit, which also satisfies SQL Server's
//! password complexity rules.

/// Length of every generated secret.
pub const SECRET_LEN: usize = 32;

const ALPHABET: &[u8; 62] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

/// Source of secrets. Swap in [`SequentialSecrets`] for deterministic tests.
pub trait SecretSource {
    fn next_secret(&mut self) -> String;
}

/// Random secrets from the operating system's randomness (via v4 UUIDs).
pub struct RandomSecrets;

impl SecretSource for RandomSecrets {
    fn next_secret(&mut self) -> String {
        loop {
            let mut out = String::with_capacity(SECRET_LEN);
            while out.len() < SECRET_LEN {
                for byte in uuid::Uuid::new_v4().as_bytes() {
                    // Rejection sampling: 248 = 4 * 62, so no modulo bias.
                    if *byte < 248 && out.len() < SECRET_LEN {
                        out.push(ALPHABET[(*byte % 62) as usize] as char);
                    }
                }
            }
            if has_all_classes(&out) {
                return out;
            }
        }
    }
}

/// Deterministic secrets: `Aa1` followed by the counter, padded with `x`.
pub struct SequentialSecrets {
    next: u64,
}

impl SequentialSecrets {
    pub fn new() -> Self {
        Self { next: 1 }
    }
}

impl Default for SequentialSecrets {
    fn default() -> Self {
        Self::new()
    }
}

impl SecretSource for SequentialSecrets {
    fn next_secret(&mut self) -> String {
        let n = self.next;
        self.next += 1;
        format!("{:x<width$}", format!("Aa1{n}"), width = SECRET_LEN)
    }
}

fn has_all_classes(s: &str) -> bool {
    s.chars().any(|c| c.is_ascii_uppercase())
        && s.chars().any(|c| c.is_ascii_lowercase())
        && s.chars().any(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_secrets_are_long_alphanumeric_and_complex() {
        let mut src = RandomSecrets;
        let mut seen = std::collections::HashSet::new();
        for _ in 0..200 {
            let s = src.next_secret();
            assert_eq!(s.len(), SECRET_LEN);
            assert!(s.chars().all(|c| c.is_ascii_alphanumeric()), "{s}");
            assert!(has_all_classes(&s), "{s}");
            assert!(seen.insert(s));
        }
    }

    #[test]
    fn sequential_secrets_are_deterministic_and_complex() {
        let mut src = SequentialSecrets::new();
        let a = src.next_secret();
        let b = src.next_secret();
        assert_ne!(a, b);
        assert_eq!(a.len(), SECRET_LEN);
        assert!(has_all_classes(&a));
        assert!(a.starts_with("Aa11x"));
    }
}
