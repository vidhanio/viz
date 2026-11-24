//! Where viz keeps its config, state, and default local Unix socket.

use std::{
    env,
    path::{Path, PathBuf},
};

use crate::{Error, Result};

/// Maximum encoded byte length of a Unix domain socket path excluding the NUL
/// terminator (`103` on BSD/macOS, `107` on Linux).
pub const MAX_UNIX_SOCKET_PATH: usize = if cfg!(any(
    target_os = "macos",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly",
)) {
    103
} else {
    107
};

/// Filesystem locations viz uses, following the XDG base directory spec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// `$XDG_CONFIG_HOME/viz`.
    pub config_dir: PathBuf,
    /// `$STATE_DIRECTORY` (systemd `StateDirectory=`), default
    /// `$XDG_STATE_HOME/viz`: databases and logs.
    pub state_dir: PathBuf,
}

impl Paths {
    /// Resolves paths from the process environment. Empty variables count as
    /// unset.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NoHome`] if a needed XDG variable and `$HOME` are both
    /// unset.
    pub fn from_env() -> Result<Self> {
        let config_dir = config_dir()?;
        let state_dir = match var("STATE_DIRECTORY") {
            Some(dir) => dir,
            None => xdg("XDG_STATE_HOME", ".local/state")?.join("viz"),
        };
        Ok(Self {
            config_dir,
            state_dir,
        })
    }
}

/// Resolves the default local Unix domain socket path shared by server,
/// desktop, and CLI:
///
/// 1. `$XDG_RUNTIME_DIR/viz/rpc.sock` when `$XDG_RUNTIME_DIR` is absolute.
/// 2. `$TMPDIR/viz/rpc.sock` on macOS when `$TMPDIR` is absolute.
/// 3. `<state_dir>/rpc.sock` as a private per-user fallback.
///
/// # Errors
///
/// Returns [`Error::NoHome`] if the fallback state directory cannot be located,
/// or [`Error::InvalidSocket`] if the resolved path exceeds platform socket
/// path limits.
pub fn default_socket() -> Result<PathBuf> {
    let path = if let Some(runtime) = var("XDG_RUNTIME_DIR").filter(|dir| dir.is_absolute()) {
        runtime.join("viz").join("rpc.sock")
    } else if cfg!(target_os = "macos")
        && let Some(tmp) = var("TMPDIR").filter(|dir| dir.is_absolute())
    {
        tmp.join("viz").join("rpc.sock")
    } else {
        Paths::from_env()?.state_dir.join("rpc.sock")
    };
    validate_socket_path(&path)?;
    Ok(path)
}

/// Verifies that `path` is an absolute Unix socket path with a parent
/// directory and within [`MAX_UNIX_SOCKET_PATH`].
///
/// # Errors
///
/// Returns [`Error::InvalidSocket`] if `path` cannot be bound safely.
pub fn validate_socket_path(path: &Path) -> Result<()> {
    if !path.is_absolute() {
        return Err(Error::InvalidSocket {
            path: path.to_path_buf(),
            reason: "socket path must be absolute",
        });
    }
    if path.parent().is_none() || path.file_name().is_none() {
        return Err(Error::InvalidSocket {
            path: path.to_path_buf(),
            reason: "socket path must name a file inside a directory",
        });
    }
    if path.as_os_str().as_encoded_bytes().len() > MAX_UNIX_SOCKET_PATH {
        return Err(Error::InvalidSocket {
            path: path.to_path_buf(),
            reason: "socket path exceeds platform sockaddr_un length limit",
        });
    }
    Ok(())
}

pub fn config_dir() -> Result<PathBuf> {
    Ok(xdg("XDG_CONFIG_HOME", ".config")?.join("viz"))
}

pub fn var(key: &str) -> Option<PathBuf> {
    env::var_os(key)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn xdg(key: &'static str, fallback: &str) -> Result<PathBuf> {
    var(key)
        .or_else(|| var("HOME").map(|home| home.join(fallback)))
        .ok_or(Error::NoHome { var: key })
}
