//! Error type shared by all modules.

use std::fmt;
use crate::validate::ValidationIssue;
use std::path::PathBuf;

/// The result type of every fallible call in this crate.
pub type Result<T> = std::result::Result<T, ScaffoldError>;

/// Everything that can go wrong, with enough detail to show to the user. The enum is `#[non_exhaustive]`: match it with a
/// fallback arm. [`kind`](ScaffoldError::kind) gives a short stable name and [`to_json`](ScaffoldError::to_json) the error as JSON.
#[derive(Debug)]
#[non_exhaustive]
pub enum ScaffoldError {
    /// The manifest is malformed or internally inconsistent.
    Manifest(String),
    /// A token name is not valid (must be non-empty ASCII alphanumeric).
    InvalidTokenName(String),
    /// `{__name__}` was found in a template but no value is defined for `name`.
    UnknownToken {
        /// The name that has no value.
        token: String,
        /// Where it was found (a file or a path).
        context: String,
    },
    /// A path is absolute, escapes the root, or contains characters Windows rejects.
    InvalidPath {
        /// The path as it was given.
        path: String,
        /// What is wrong with it.
        reason: String,
    },
    /// The same path was staged twice (compared case-insensitively, like Windows).
    DuplicatePath(String),
    /// One staged path is a file while another needs it to be a folder.
    PathConflict {
        /// The staged path that is a file.
        existing: String,
        /// The path that needs it to be a folder.
        new: String,
    },
    /// The answers do not fit the manifest (missing, wrong type, unknown key).
    Answers(String),
    /// One or more answers failed the name rules. Never contains sensitive values.
    Validation(Vec<ValidationIssue>),
    /// A template file or folder is missing, unreadable or not allowed (e.g. a symlink).
    Template {
        /// The template file or folder.
        path: String,
        /// What is wrong with it.
        reason: String,
    },
    /// A merge could not be applied.
    Merge {
        /// The file the merge was for.
        file: String,
        /// Why it could not be applied.
        reason: String,
    },
    /// A download failed (network, HTTP status). The reason comes from the host's `Fetch`.
    Fetch {
        /// The address that was requested.
        url: String,
        /// What the host's `Fetch` reported.
        reason: String,
    },
    /// The template index is malformed or inconsistent.
    Index(String),
    /// A downloaded archive is unsafe or has the wrong layout.
    Archive(String),
    /// A pack file is malformed, unsafe or fails its own checksums.
    Pack(String),
    /// A download does not match the checksum in the index.
    ChecksumMismatch {
        /// The archive's address.
        url: String,
        /// The SHA-256 the index lists.
        expected: String,
        /// The SHA-256 of what was downloaded.
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
        /// The file or folder the operation was on.
        path: PathBuf,
        /// The underlying IO error.
        source: std::io::Error,
    },
    /// The destination folder already exists.
    FolderExists(PathBuf),
    /// A file the add mode would create is already in the solution.
    ExistsInSolution(String),
    /// An add would write through a symbolic link or a junction inside the solution, which could put a file somewhere
    /// else on the machine.
    LinkInSolution(String),
    /// A template is bigger than the engine is willing to render in memory (file count, one file, or the total).
    TooLarge(String),
    /// A component (a web part, an extension) the add would create is already in the solution:
    /// one of its files exists, or exists with only the letter case different (`Hello` and
    /// `hello` are the same component, and a case-sensitive file system would not say so).
    ComponentExists {
        /// The name as the user gave it.
        name: String,
        /// The file the add would have created.
        path: String,
        /// The file that is already in the solution (the same as `path`, or differing only by case).
        existing: String,
    },
    /// The generator does not fit the existing solution (version, kind).
    Incompatible(String),
    /// A file changed between planning and applying an add.
    ChangedSincePlan(String),
    /// `installed.json`, the list of template folders this crate downloaded, exists but cannot be read: it is damaged, or it
    /// was written by a newer version. **Nothing was changed.** It is refused only where a wrong answer could do harm
    /// (installing, updating, the list of updates), because without it every downloaded folder would look like the user's own
    /// and a write would rewrite the file from an empty list. The template lists carry the same message as a problem, and
    /// `CheckOutcome::problems` too.
    InstalledUnreadable(String),
    /// An undo record cannot be used: not valid, from a newer format, an unsafe path, or damaged.
    UndoRecord(String),
    /// A revert was refused because these files (below the solution) are no longer as the edit left them; nothing was changed.
    RevertConflict(Vec<String>),
    /// An IO error happened while writing. `rolled_back` tells whether the
    /// folder we created was removed again.
    WriteFailed {
        /// The file that could not be written.
        path: PathBuf,
        /// The underlying IO error.
        source: std::io::Error,
        /// Whether everything this call had created or changed was put back.
        rolled_back: bool,
    },
}

impl ScaffoldError {
    /// A short, stable name for the kind of error (`validation`, `invalid_path`, `too_large`, ...), for a host that
    /// answers to the kind and shows the message.
    ///
    /// # Example
    ///
    /// ```
    /// use forge_scaffold::ScaffoldError;
    ///
    /// let error = ScaffoldError::NotFound("template spfx-webpart 9.9.9 is not installed".into());
    /// assert_eq!(error.kind(), "not_found");
    /// assert_eq!(error.to_json()["kind"], "not_found");
    /// println!("{error}"); // the message to show the user
    /// ```
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Manifest(_) => "manifest",
            Self::InvalidTokenName(_) => "invalid_token_name",
            Self::UnknownToken { .. } => "unknown_token",
            Self::InvalidPath { .. } => "invalid_path",
            Self::DuplicatePath(_) => "duplicate_path",
            Self::PathConflict { .. } => "path_conflict",
            Self::Answers(_) => "answers",
            Self::Validation(_) => "validation",
            Self::Template { .. } => "template",
            Self::Merge { .. } => "merge",
            Self::Fetch { .. } => "fetch",
            Self::Index(_) => "index",
            Self::Archive(_) => "archive",
            Self::Pack(_) => "pack",
            Self::ChecksumMismatch { .. } => "checksum_mismatch",
            Self::LocalTemplate(_) => "local_template",
            Self::Busy(_) => "busy",
            Self::NotFound(_) => "not_found",
            Self::Io { .. } => "io",
            Self::FolderExists(_) => "folder_exists",
            Self::ExistsInSolution(_) => "exists_in_solution",
            Self::LinkInSolution(_) => "link_in_solution",
            Self::TooLarge(_) => "too_large",
            Self::ComponentExists { .. } => "component_exists",
            Self::Incompatible(_) => "incompatible",
            Self::ChangedSincePlan(_) => "changed_since_plan",
            Self::InstalledUnreadable(_) => "installed_unreadable",
            Self::UndoRecord(_) => "undo_record",
            Self::RevertConflict(_) => "revert_conflict",
            Self::WriteFailed { .. } => "write_failed",
        }
    }

    /// The error as JSON: its kind, its message, and for a validation error one entry per field.
    pub fn to_json(&self) -> serde_json::Value {
        let mut value = serde_json::json!({ "kind": self.kind(), "message": self.to_string() });
        if let Self::Validation(issues) = self {
            value["fields"] = serde_json::json!(issues.iter().map(|i| serde_json::json!({ "field": i.field, "message": i.message })).collect::<Vec<_>>());
        }
        value
    }
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
            Self::Pack(msg) => write!(f, "Invalid template pack: {msg}"),
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
            Self::ExistsInSolution(path) => {
                write!(f, "'{path}' already exists in the solution, so nothing was added")
            }
            Self::TooLarge(what) => write!(f, "{what}"),
            Self::LinkInSolution(path) => write!(
                f,
                "'{path}' is reached through a symbolic link or a junction inside the solution, so nothing was written; \
                 a file could end up somewhere else on this machine"
            ),
            Self::ComponentExists { name, path, existing } if path == existing && path.contains('#') => write!(
                f,
                "'{name}' is already in this solution, registered as the {} in {}, so nothing was added",
                path.split_once('#').map_or("", |(_, what)| what),
                path.split_once('#').map_or("", |(file, _)| file)
            ),
            Self::ComponentExists { name, path, existing } if path == existing => write!(
                f,
                "'{name}' is already in this solution ('{existing}' exists), so nothing was added"
            ),
            Self::ComponentExists { name, path, existing } => write!(
                f,
                "'{name}' is the same as a component already in this solution: '{existing}' exists, and '{path}' \
                 would differ from it only by letter case, so nothing was added"
            ),
            Self::Incompatible(msg) => write!(f, "{msg}"),
            Self::ChangedSincePlan(path) => {
                write!(f, "'{path}' changed after the plan was made, so nothing was written; plan again")
            }
            Self::UndoRecord(msg) => write!(f, "{msg}"),
            Self::InstalledUnreadable(msg) => write!(f, "{msg}"),
            Self::RevertConflict(files) => write!(
                f,
                "these files changed after the edit, so nothing was reverted: {}",
                files.join(", ")
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
