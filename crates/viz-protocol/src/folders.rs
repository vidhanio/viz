//! Server-hosted folder browsing and project provisioning requests.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct BrowseFolders {
    /// A directory or a typed path to complete. Relative paths use the project
    /// or server directory.
    pub path: Option<String>,
    pub project: Option<String>,
    pub show_hidden: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FolderEntry {
    pub path: String,
    pub name: String,
    pub git: bool,
    pub project: bool,
    pub issue: Option<FolderIssue>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum FolderIssue {
    PermissionDenied,
    NotFound,
    NotDirectory,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FolderEntries {
    pub path: String,
    pub parent: Option<String>,
    pub home: Option<String>,
    pub git: bool,
    pub project: bool,
    pub completion: Option<String>,
    pub entries: Vec<FolderEntry>,
    pub issue: Option<FolderIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CreateFolder {
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FolderCreated {
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InitRepository {
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StartClone {
    pub url: String,
    pub path: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct CloneId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReadClone {
    pub id: CloneId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CancelClone {
    pub id: CloneId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ClonePhase {
    Preparing,
    Counting,
    Compressing,
    Receiving,
    Resolving,
    CheckingOut,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum CloneState {
    Running {
        phase: ClonePhase,
        percent: Option<u8>,
    },
    Completed {
        project: String,
    },
    Failed {
        message: String,
    },
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CloneStatus {
    pub id: CloneId,
    pub state: CloneState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitHubRepositories {
    pub host: Option<String>,
    pub query: Option<String>,
    pub page: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GitHubRepository {
    pub name: String,
    pub full_name: String,
    pub default_branch: String,
    pub clone_url: String,
    pub ssh_url: String,
    pub avatar_url: String,
    pub private: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitHubRepositoryList {
    pub repositories: Vec<GitHubRepository>,
}
