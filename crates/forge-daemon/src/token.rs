//! The bearer token every client must present.
//!
//! 32 random bytes, hex-encoded, `0600` on disk, generated on first start and
//! never logged.

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

const TOKEN_BYTES: usize = 32;
const OWNER_ONLY: u32 = 0o600;

/// A bearer token. Its `Debug` is redacted so it cannot reach the logs by
/// accident.
#[derive(Clone, PartialEq, Eq)]
pub struct Token(String);

impl Token {
    /// Load the token, generating and persisting one on first run.
    pub fn load_or_create(path: &Path) -> Result<Self, TokenError> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let token = text.trim();
                if token.is_empty() {
                    return Err(TokenError::Empty(path.to_path_buf()));
                }
                tighten_permissions(path)?;
                Ok(Self(token.to_owned()))
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                let token = Self(hex(&random_bytes()?));
                token.write(path)?;
                Ok(token)
            }
            Err(source) => Err(TokenError::Read {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    fn write(&self, path: &Path) -> Result<(), TokenError> {
        let write = || -> io::Result<()> {
            let mut file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(OWNER_ONLY)
                .open(path)?;
            file.write_all(self.0.as_bytes())?;
            file.write_all(b"\n")
        };
        write().map_err(|source| TokenError::Write {
            path: path.to_path_buf(),
            source,
        })?;
        // `mode` only applies when the file is created; an existing file keeps
        // whatever it had.
        tighten_permissions(path)
    }

    /// Compare in constant time, so a wrong token leaks no prefix.
    pub fn matches(&self, candidate: &str) -> bool {
        let expected = self.0.as_bytes();
        let actual = candidate.as_bytes();
        if expected.len() != actual.len() {
            return false;
        }
        expected
            .iter()
            .zip(actual)
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
    }

    /// Only for handing to a client (the app's install flow); never log this.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(<redacted>)")
    }
}

fn tighten_permissions(path: &Path) -> Result<(), TokenError> {
    let apply = || -> io::Result<()> {
        let mut perms = std::fs::metadata(path)?.permissions();
        if perms.mode() & 0o777 != OWNER_ONLY {
            perms.set_mode(OWNER_ONLY);
            std::fs::set_permissions(path, perms)?;
        }
        Ok(())
    };
    apply().map_err(|source| TokenError::Permissions {
        path: path.to_path_buf(),
        source,
    })
}

/// macOS always has `/dev/urandom`; the daemon is macOS-only, so this
/// avoids pulling a random-number crate in for one call.
fn random_bytes() -> Result<[u8; TOKEN_BYTES], TokenError> {
    let mut buffer = [0u8; TOKEN_BYTES];
    File::open("/dev/urandom")
        .and_then(|mut source| source.read_exact(&mut buffer))
        .map_err(TokenError::Entropy)?;
    Ok(buffer)
}

fn hex(bytes: &[u8]) -> String {
    use fmt::Write as _;
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

#[derive(Debug, thiserror::Error)]
pub enum TokenError {
    #[error("cannot read the token at {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("cannot write the token to {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("cannot set 0600 on {path}: {source}")]
    Permissions {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("the token file at {0} is empty; delete it and restart to generate a new one")]
    Empty(PathBuf),
    #[error("cannot read entropy from /dev/urandom: {0}")]
    Entropy(#[source] io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode_of(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn first_run_generates_a_64_char_hex_token_at_0600() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("token");

        let token = Token::load_or_create(&path).unwrap();
        assert_eq!(token.expose().len(), TOKEN_BYTES * 2);
        assert!(token.expose().chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(mode_of(&path), OWNER_ONLY);
    }

    #[test]
    fn later_runs_reuse_the_same_token() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("token");

        let first = Token::load_or_create(&path).unwrap();
        let second = Token::load_or_create(&path).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn two_daemons_do_not_share_a_token() {
        let temp = tempfile::tempdir().unwrap();
        let a = Token::load_or_create(&temp.path().join("a")).unwrap();
        let b = Token::load_or_create(&temp.path().join("b")).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn loose_permissions_are_tightened_on_load() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("token");
        std::fs::write(&path, "deadbeef\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        Token::load_or_create(&path).unwrap();
        assert_eq!(mode_of(&path), OWNER_ONLY);
    }

    #[test]
    fn matching_ignores_the_trailing_newline_on_disk() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("token");
        let token = Token::load_or_create(&path).unwrap();

        assert!(token.matches(token.expose()));
        assert!(!token.matches(&format!("{}\n", token.expose())));
        assert!(!token.matches(""));
        assert!(!token.matches("00"));
    }

    #[test]
    fn an_empty_token_file_is_an_error_not_an_empty_password() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("token");
        std::fs::write(&path, "   \n").unwrap();

        assert!(matches!(
            Token::load_or_create(&path),
            Err(TokenError::Empty(_))
        ));
    }

    #[test]
    fn debug_does_not_leak_the_secret() {
        let token = Token("s3cret".to_owned());
        assert_eq!(format!("{token:?}"), "Token(<redacted>)");
    }
}
