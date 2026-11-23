//! Pure decisions derived from the initialize handshake.

use crate::schema::InitializeResponse;

/// Optional operations and content advertised by a harness.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each ACP capability is independently advertised"
)]
pub struct Capabilities {
    pub steering: bool,
    pub load: bool,
    pub resume: bool,
    pub fork: bool,
    pub list: bool,
    pub close: bool,
    pub delete: bool,
    pub options: bool,
    pub title: bool,
    pub archive: bool,
    pub status: bool,
    pub additional_directories: bool,
    pub system_prompt: bool,
    pub system_prompt_override: bool,
    pub image: bool,
    pub audio: bool,
    pub embedded_context: bool,
    pub mcp_http: bool,
    pub mcp_acp: bool,
    pub mcp_sse: bool,
    pub providers: bool,
    pub auth_status: bool,
    pub logout: bool,
}

impl From<&InitializeResponse> for Capabilities {
    fn from(initialized: &InitializeResponse) -> Self {
        let agent = &initialized.agent_capabilities;
        let session = &agent.session_capabilities;
        let prompt = &agent.prompt_capabilities;
        let mcp = &agent.mcp_capabilities;
        Self {
            steering: initialized
                .meta
                .as_ref()
                .and_then(|meta| meta.get("steering"))
                .and_then(|steering| steering.get("supported"))
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            load: agent.load_session,
            resume: session.resume.is_some(),
            fork: session.fork.is_some(),
            list: session.list.is_some(),
            close: session.close.is_some(),
            delete: session.delete.is_some(),
            options: session.options.is_some(),
            title: session.title.is_some(),
            archive: session.archive.is_some(),
            status: session.status.is_some(),
            additional_directories: session.additional_directories.is_some(),
            system_prompt: session.system_prompt.is_some(),
            system_prompt_override: session
                .system_prompt
                .as_ref()
                .and_then(|prompt| prompt.mode.as_ref())
                .is_some_and(|modes| modes.override_.is_some()),
            image: prompt.image,
            audio: prompt.audio,
            embedded_context: prompt.embedded_context,
            mcp_http: mcp.http,
            mcp_sse: mcp.sse,
            mcp_acp: mcp.acp,
            providers: agent.providers.is_some(),
            auth_status: agent.auth.status.is_some(),
            logout: agent.auth.logout.is_some(),
        }
    }
}

impl Capabilities {
    /// Whether the same session identity can reopen in a new process.
    #[must_use]
    pub const fn can_restore(&self) -> bool {
        self.resume || self.load
    }
}
