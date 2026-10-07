//! Error type shared by all modules.

use std::fmt;
use crate::validate::ValidationIssue;
use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, ScaffoldError>;

#[derive(Debug)]
pub enum ScaffoldError {
    /// The manifest is malformed or internally inconsistent.
    Manifest(String),
    /// A token name is not valid (must be non-empty ASCII alphanumeric).
    InvalidTokenName(String),
    /// `{__name__}` was found in a template but no value is defined for `name`.
    UnknownToken { token: String, context: String },
    /// A path is absolute, escapes the root, or contains characters Windows rejects.
    InvalidPath { path: String, reason: String },
    /// The same path was staged twice (compared case-insensitively, like Windows).
    DuplicatePath(String),
    /// One staged path is a file while another needs it to be a folder.
    PathConflict { existing: String, new: String },
    /// The answers do not fit the manifest (missing, wrong type, unknown key).
    Answers(String),
    /// One or more answers failed the name rules. Never contains sensitive values.
    Validation(Vec<ValidationIssue>),
    /// A template file or folder is missing, unreadable or not allowed (e.g. a symlink).
    Template { path: String, reason: String },
    /// A merge could not be applied.
    Merge { file: String, reason: String },
    /// A download failed (network, HTTP status). The reason comes from the host's `Fetch`.
    Fetch { url: String, reason: String },
    /// The template index is malformed or inconsistent.
    Index(String),
    /// A downloaded archive is unsafe or has the wrong layout.
    Archive(String),
    /// A download does not match the checksum in the index.
    ChecksumMismatch {
        url: String,
        expected: String,
        actual: String,
    },
    /// A folder the user made themselves is in the way; it is never overwritten.
    LocalTemplate(String),
    /// Another update is already running.
    Busy(String),
    /// The requested template version is not installed (and not in the index).
    NotFound(String),
    /// A file operation in the templates folder failed.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The destination folder already exists.
    FolderExists(PathBuf),
    /// An IO error happened while writing. `rolled_back` tells whether the
    /// folder we created was removed again.
    WriteFailed {
        path: PathBuf,
        source: std::io::Error,
        rolled_back: bool,
    },
}

impl fmt::Display for ScaffoldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Manifest(msg) => write!(f, "Invalid manifest: {msg}"),
            Self::InvalidTokenName(name) => write!(
                f,
                "Invalid token name '{name}': use ASCII letters and digits only"
            ),
            Self::UnknownToken { token, context } => {
                write!(f, "Unknown token '{{__{token}__}}' in {context}")
            }
            Self::InvalidPath { path, reason } => {
                write!(f, "Invalid path '{path}': {reason}")
            }
            Self::DuplicatePath(p) => write!(f, "Path staged twice: '{p}'"),
            Self::PathConflict { existing, new } => write!(
                f,
                "Path conflict: '{new}' cannot be created because '{existing}' is a file or folder in the way"
            ),
            Self::Answers(msg) => write!(f, "Invalid answers: {msg}"),
            Self::Validation(issues) => {
                let list: Vec<String> = issues
                    .iter()
                    .map(|i| format!("{}: {}", i.field, i.message))
                    .collect();
                write!(f, "Invalid input: {}", list.join("; "))
            }
            Self::Template { path, reason } => write!(f, "Template problem at '{path}': {reason}"),
            Self::Merge { file, reason } => write!(f, "Cannot merge into '{file}': {reason}"),
            Self::Fetch { url, reason } => write!(f, "Download failed for {url}: {reason}"),
            Self::Index(msg) => write!(f, "Invalid template index: {msg}"),
            Self::Archive(msg) => write!(f, "Unsafe or invalid template archive: {msg}"),
            Self::ChecksumMismatch {
                url,
                expected,
                actual,
            } => write!(
                f,
                "Checksum mismatch for {url}: expected {expected}, got {actual}"
            ),
            Self::LocalTemplate(msg) => write!(f, "{msg}"),
            Self::Busy(msg) => write!(f, "{msg}"),
            Self::NotFound(msg) => write!(f, "{msg}"),
            Self::Io { path, source } => write!(f, "'{}': {source}", path.display()),
            Self::FolderExists(p) => write!(
                f,
                "The current folder already exists, please choose a different name: {}",
                p.display()
            ),
            Self::WriteFailed {
                path,
                source,
                rolled_back,
            } => write!(
                f,
                "Failed to write '{}': {source} ({})",
                path.display(),
                if *rolled_back {
                    "partial output removed"
                } else {
                    "could not remove partial output"
                }
            ),
        }
    }
}

impl std::error::Error for ScaffoldError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::WriteFailed { source, .. } | Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}
