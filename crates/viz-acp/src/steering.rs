//! The harness extension for injecting input into an active prompt.

use std::sync::Arc;

use agent_client_protocol::{JsonRpcRequest, JsonRpcResponse};
use serde::{Deserialize, Serialize};

use crate::{
    Capabilities,
    connection::Connection,
    error::Result,
    lifecycle::method_missing,
    schema::{ContentBlock, SessionId},
};

/// A handle that can inject a prompt while the session's driver is borrowed.
#[derive(Clone)]
pub struct Steering {
    pub(crate) connection: Arc<Connection>,
    pub(crate) capabilities: Capabilities,
}

impl std::fmt::Debug for Steering {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Steering")
            .field("supported", &self.capabilities.steering)
            .finish_non_exhaustive()
    }
}

/// Whether the harness consumed an attempted steering prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SteerOutcome {
    Injected,
    /// The harness started a detached turn. Its output arrives as session
    /// updates.
    StartedNewTurn,
    /// The target turn finished first. The caller can send a new prompt.
    PromptRequired,
    /// The caller can request tool-safe cancellation and queue a new prompt.
    Unsupported,
}

impl Steering {
    /// Injects blocks into an active turn, if the harness offers native
    /// steering.
    ///
    /// # Errors
    /// Returns an error when content is unsupported or the request could have
    /// failed after delivery.
    pub async fn submit(&self, session: &str, blocks: &[ContentBlock]) -> Result<SteerOutcome> {
        if !self.capabilities.steering {
            return Ok(SteerOutcome::Unsupported);
        }
        crate::prompt::validate_context(blocks, &self.capabilities)?;
        let request = SteeringRequest {
            session_id: SessionId::new(session.to_owned()),
            prompt: blocks.to_vec(),
            meta: Metadata {
                steering: IdleBehavior {
                    idle_behavior: IdleMode::PromptRequired,
                },
            },
        };
        match self.connection.request(request, None).await {
            Ok(SteeringResponse::Injected) => Ok(SteerOutcome::Injected),
            Ok(SteeringResponse::StartedNewTurn) => Ok(SteerOutcome::StartedNewTurn),
            Ok(SteeringResponse::PromptRequired) => Ok(SteerOutcome::PromptRequired),
            Err(error) if method_missing(&error) => Ok(SteerOutcome::Unsupported),
            Err(error) => Err(error),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcRequest)]
#[request(method = "_session/steering", response = SteeringResponse)]
#[serde(rename_all = "camelCase")]
struct SteeringRequest {
    session_id: SessionId,
    prompt: Vec<ContentBlock>,
    #[serde(rename = "_meta")]
    meta: Metadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Metadata {
    steering: IdleBehavior,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdleBehavior {
    idle_behavior: IdleMode,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcResponse)]
#[serde(tag = "outcome", rename_all = "camelCase")]
enum SteeringResponse {
    Injected,
    StartedNewTurn,
    PromptRequired,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum IdleMode {
    PromptRequired,
}
