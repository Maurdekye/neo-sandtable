//! One launcher reader consumes an owner-provisioned subscription token file.
//! No ambient login lookup, auth probe, logging, serialization or public getter.
//! The launcher must apply this only to the intended confined seat process.

use std::{fs, io::Read, path::Path};

const TOKEN_BYTES: u64 = 16 * 1024;
pub const TOKEN_ENV: &str = "CLAUDE_CODE_OAUTH_TOKEN";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum LoginError {
    #[error("owner login file location is unavailable or unsupported")]
    Location,
    #[error("owner login file could not be read")]
    Read,
    #[error("owner login file could not be deleted; login withheld")]
    Delete,
    #[error("owner login file contains an invalid token")]
    Invalid,
}

/// Intentionally no Debug, Display, Clone, serialization or token accessor.
/// The launcher may hold one Arc and apply the same owner login to its seats.
pub struct SubscriptionLogin {
    token: String,
}

impl SubscriptionLogin {
    pub fn consume_file(path: &Path) -> Result<Self, LoginError> {
        consume_outside(
            path,
            Path::new(crate::calibration_isolation::ISOLATION_ROOT),
        )
    }

    /// Add the one intended credential variable. The caller must first clear
    /// ambient authority and apply the explicitly reviewed runtime environment.
    pub fn apply_to(&self, command: &mut tokio::process::Command) {
        // These must never select API billing, a provider alternative or an
        // ambient OAuth refresh path instead of the owner-supplied token.
        for name in [
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR",
        ] {
            command.env_remove(name);
        }
        command.env(TOKEN_ENV, &self.token);
    }
}

fn plain_metadata(path: &Path, directory: bool) -> Result<(), LoginError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| LoginError::Location)?;
    if metadata.file_type().is_symlink() {
        return Err(LoginError::Location);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(LoginError::Location);
        }
    }
    if if directory {
        !metadata.is_dir()
    } else {
        !metadata.is_file()
    } {
        return Err(LoginError::Location);
    }
    Ok(())
}

fn consume_outside(path: &Path, isolation: &Path) -> Result<SubscriptionLogin, LoginError> {
    if !path.is_absolute() || !isolation.is_absolute() {
        return Err(LoginError::Location);
    }
    // Metadata only. Reject all links/reparse ancestors before opening a token.
    plain_metadata(path, false)?;
    for ancestor in path.ancestors().skip(1) {
        plain_metadata(ancestor, true)?;
    }
    plain_metadata(isolation, true)?;
    let root = isolation.canonicalize().map_err(|_| LoginError::Location)?;
    let canonical = path.canonicalize().map_err(|_| LoginError::Location)?;
    if canonical.starts_with(root) {
        return Err(LoginError::Location);
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // Open the reparse point itself, not its target; validate handle metadata
        // too. No sharing while reading prevents a second reader in this window.
        options.custom_flags(0x0020_0000).share_mode(0);
    }
    let file = options.open(path).map_err(|_| LoginError::Read)?;
    let metadata = file.metadata().map_err(|_| LoginError::Read)?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(LoginError::Location);
        }
    }
    if !metadata.is_file() {
        return Err(LoginError::Location);
    }
    let mut bytes = Vec::new();
    let result = file.take(TOKEN_BYTES + 1).read_to_end(&mut bytes);
    // take() owns and drops the only open file before Windows deletion. No
    // credential is returned, even on read failure, unless deletion succeeds.
    fs::remove_file(path).map_err(|_| LoginError::Delete)?;
    result.map_err(|_| LoginError::Read)?;
    if bytes.len() as u64 > TOKEN_BYTES {
        return Err(LoginError::Invalid);
    }
    let token = String::from_utf8(bytes).map_err(|_| LoginError::Invalid)?;
    let token = token.trim_end_matches(['\r', '\n']);
    if token.is_empty()
        || token.len() as u64 > TOKEN_BYTES
        || !token.bytes().all(|byte| byte.is_ascii_graphic())
    {
        return Err(LoginError::Invalid);
    }
    Ok(SubscriptionLogin {
        token: token.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Files {
        root: std::path::PathBuf,
        isolation: std::path::PathBuf,
        token: std::path::PathBuf,
    }
    impl Files {
        fn new(bytes: &[u8]) -> Self {
            let root =
                std::env::temp_dir().join(format!("cna-login-test-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&root).unwrap();
            let isolation = root.join("isolation");
            fs::create_dir(&isolation).unwrap();
            let token = root.join("owner-token");
            fs::write(&token, bytes).unwrap();
            Self {
                root,
                isolation,
                token,
            }
        }
    }
    impl Drop for Files {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn one_reader_deletes_before_environment_handoff_and_cannot_reopen() {
        let files = Files::new(b"SYNTHETIC_LOGIN_ONLY\r\n");
        let login = consume_outside(&files.token, &files.isolation).unwrap();
        assert!(!files.token.exists());
        assert!(matches!(
            consume_outside(&files.token, &files.isolation),
            Err(LoginError::Location)
        ));
        let mut command = tokio::process::Command::new("UNEXECUTED");
        command.env_clear();
        login.apply_to(&mut command);
        let actual: Vec<_> = command
            .as_std()
            .get_envs()
            .filter_map(|(name, value)| value.map(|value| (name, value)))
            .collect();
        assert_eq!(actual.len(), 1);
        assert_eq!(actual[0].0, TOKEN_ENV);
        assert_eq!(actual[0].1, "SYNTHETIC_LOGIN_ONLY");
    }

    #[test]
    fn invalid_and_oversized_secrets_are_deleted_without_returning_contents() {
        for bytes in [
            b"".to_vec(),
            b"SYNTHETIC LOGIN".to_vec(),
            vec![b'a'; TOKEN_BYTES as usize + 1],
            vec![255],
        ] {
            let files = Files::new(&bytes);
            assert!(matches!(
                consume_outside(&files.token, &files.isolation),
                Err(LoginError::Invalid)
            ));
            assert!(!files.token.exists());
        }
        assert!(!format!("{:?}", LoginError::Invalid).contains("SYNTHETIC"));
    }

    #[test]
    fn seat_tree_and_relative_paths_refuse_without_consuming_file() {
        let files = Files::new(b"SYNTHETIC_LOGIN_ONLY");
        let inside = files.isolation.join("token");
        fs::rename(&files.token, &inside).unwrap();
        assert!(matches!(
            consume_outside(&inside, &files.isolation),
            Err(LoginError::Location)
        ));
        assert!(inside.exists());
        assert!(matches!(
            consume_outside(Path::new("relative-token"), &files.isolation),
            Err(LoginError::Location)
        ));
    }
}
