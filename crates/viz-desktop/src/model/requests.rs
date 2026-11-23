//! Local request submission and elicitation form state.

use std::collections::{BTreeMap, BTreeSet};

use viz_client::protocol::{FormValue, RequestBody, RequestId, RequestState, RunRequest};

use super::Model;

#[derive(Debug, Clone, Default)]
pub struct Requests {
    pub environment: String,
    pub sending: BTreeSet<RequestId>,
    pub resolved: BTreeSet<RequestId>,
    pub forms: BTreeMap<RequestId, Form>,
}

#[derive(Debug, Clone, Default)]
pub struct Form {
    pub step: usize,
    pub collapsed: bool,
    pub fields: BTreeMap<String, FormValue>,
    pub error: Option<String>,
}

pub fn pending(model: &Model) -> impl Iterator<Item = &RunRequest> {
    model
        .thread_state()
        .into_iter()
        .flat_map(|state| &state.requests)
        .filter(|request| {
            request.state == RequestState::Pending && !model.requests.resolved.contains(&request.id)
        })
}

pub fn question(model: &Model) -> Option<&RunRequest> {
    pending(model).find(|request| matches!(request.body, RequestBody::Question { .. }))
}
