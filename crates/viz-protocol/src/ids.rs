//! Identifiers. Numeric IDs are allocated from one upwards, so zero is
//! rejected wherever one is decoded.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, de::Error as _};

macro_rules! ids {
    ($($(#[$meta:meta])* $name:ident;)*) => {$(
        $(#[$meta])*
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
        )]
        #[serde(transparent)]
        pub struct $name(
            #[serde(deserialize_with = "positive")]
            #[schemars(range(min = 1))]
            pub u64,
        );

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    )*};
}

ids! {
    /// A thread id.
    ThreadId;
    /// A run id.
    RunId;
    /// A transcript entry id.
    EntryId;
    /// A harness request's id.
    RequestId;
}

fn positive<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    match u64::deserialize(deserializer)? {
        0 => Err(D::Error::custom("an id starts at 1")),
        id => Ok(id),
    }
}

/// A delegated task's id, allocated by the environment that owns the parent
/// thread.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct TaskId(pub String);

impl TaskId {
    /// The id as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// An application-specific fingerprint of a machine.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct HostId(pub String);

impl HostId {
    /// Reads the platform machine identifier and hashes it for the wire.
    /// A missing identifier cannot establish that two processes share a host.
    #[must_use]
    pub fn local() -> Option<Self> {
        use std::fmt::Write as _;

        use sha2::{Digest as _, Sha256};

        let id = local_machine_id()?;
        let id = id.trim();
        if id.is_empty() {
            return None;
        }
        let mut digest = Sha256::new();
        digest.update(b"viz host identity\0");
        digest.update(std::env::consts::OS);
        digest.update(id);
        let mut encoded = String::with_capacity(64);
        for byte in digest.finalize() {
            write!(encoded, "{byte:02x}").ok()?;
        }
        Some(Self(encoded))
    }
}

#[cfg(target_os = "linux")]
fn local_machine_id() -> Option<String> {
    std::fs::read_to_string("/etc/machine-id")
        .or_else(|_| std::fs::read_to_string("/var/lib/dbus/machine-id"))
        .ok()
}

#[cfg(target_os = "macos")]
fn local_machine_id() -> Option<String> {
    let output = std::process::Command::new("/usr/sbin/ioreg")
        .args(["-rd1", "-c", "IOPlatformExpertDevice"])
        .output()
        .ok()?;
    let properties = std::str::from_utf8(&output.stdout).ok()?;
    let (_, value) = properties.split_once("\"IOPlatformUUID\" = \"")?;
    Some(value.split_once('"')?.0.to_owned())
}

#[cfg(target_os = "windows")]
fn local_machine_id() -> Option<String> {
    let output = std::process::Command::new("reg.exe")
        .args([
            "query",
            r"HKLM\SOFTWARE\Microsoft\Cryptography",
            "/v",
            "MachineGuid",
            "/reg:64",
        ])
        .output()
        .ok()?;
    let properties = String::from_utf8_lossy(&output.stdout);
    let (_, value) = properties.split_once("REG_SZ")?;
    Some(value.trim().to_owned())
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn local_machine_id() -> Option<String> {
    None
}
