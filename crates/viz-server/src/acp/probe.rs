//! Disposable harness processes for learning a harness's options for new
//! threads.

use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

use agent_client_protocol::v1::schema::{SessionNotification, SessionUpdate};
use anyhow::{Context as _, Result, anyhow, bail};

use super::{
    options::{Catalog, Desired, model_option, normalize, valid_value},
    process::Agent,
};
use crate::{
    config::HarnessConfig,
    wire::{ConfigOption, OptionValue},
};

/// Bounds each disposable process.
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// The options a new thread of this harness starts with, from the `stored`
/// catalog, which is probed from a disposable session when missing or when it
/// lacks the requested model's thinking levels. Returns the catalog to
/// store when it changed.
///
/// # Errors
///
/// When `model` is not offered, or probing fails or times out.
pub(crate) async fn catalog(
    config: &HarnessConfig,
    cwd: &Path,
    stored: Option<&Catalog>,
    model: Option<&str>,
) -> Result<(Vec<ConfigOption>, Option<Catalog>)> {
    let mut catalog = stored.cloned();
    let mut changed = false;
    if catalog.is_none() {
        changed |= probe(config, cwd, None, &mut catalog).await?;
    }
    let Some(current) = &catalog else {
        return Ok((Vec::new(), None));
    };
    let requested = model.or(config.model.as_deref());
    let offered = requested.filter(|requested| current.offers(requested));
    match offered {
        Some(requested) if !current.has_profile(requested) => {
            changed |= probe(config, cwd, Some(requested), &mut catalog).await?;
        }
        None if model.is_some() => bail!("Model not available"),
        Some(_) | None => {}
    }
    let catalog = catalog.context("the ACP options catalog is missing after probing")?;
    let options = catalog.options(config, model);
    Ok((options, changed.then_some(catalog)))
}

/// Stable ACP cannot fetch settings for an inactive model, so this asks a
/// disposable, prompt-free session, after selecting `model`. Returns whether
/// `catalog` changed.
async fn probe(
    config: &HarnessConfig,
    cwd: &Path,
    model: Option<&str>,
    catalog: &mut Option<Catalog>,
) -> Result<bool> {
    let probing = async {
        let reported: Arc<Mutex<Option<Vec<ConfigOption>>>> = Arc::default();
        let sink = {
            let reported = Arc::clone(&reported);
            Arc::new(move |notification: SessionNotification| {
                if let SessionUpdate::ConfigOptionUpdate(update) = notification.update {
                    *reported.lock().expect("probe lock poisoned") =
                        Some(normalize(update.config_options));
                }
            })
        };
        let agent = Agent::start(config, cwd, sink).await?;
        let response = agent.new_session(cwd, Vec::new()).await?;
        let session = response.session_id.0.to_string();
        let options = response.config_options.map_or_else(
            || {
                reported
                    .lock()
                    .expect("probe lock poisoned")
                    .take()
                    .unwrap_or_default()
            },
            normalize,
        );
        let result = select_model(&agent, &session, options, model, catalog).await;
        if let Err(error) = agent.discard_session(&session).await {
            tracing::warn!("ACP {} catalog cleanup: {error:#}", agent.label());
        }
        result
    };
    tokio::time::timeout(PROBE_TIMEOUT, probing)
        .await
        .map_err(|_| anyhow!("probing ACP options timed out"))?
}

/// Merges the probe session's `options` into `catalog`, then those after
/// selecting `model`, if given.
async fn select_model(
    agent: &Agent,
    session: &str,
    options: Vec<ConfigOption>,
    model: Option<&str>,
    catalog: &mut Option<Catalog>,
) -> Result<bool> {
    let mut changed = merge(catalog, &options);
    let Some(model) = model else {
        return Ok(changed);
    };
    let wanted = OptionValue::Select(model.to_owned());
    let setting = model_option(&options);
    if setting.is_some_and(|setting| setting.value == wanted) {
        return Ok(changed);
    }
    let id = setting
        .filter(|setting| valid_value(setting, &wanted))
        .map(|setting| setting.id.clone())
        .context("Model not available")?;
    let selected = agent.set_option(session, &id, &wanted).await?;
    changed |= merge(catalog, &normalize(selected.config_options));
    Ok(changed)
}

fn merge(catalog: &mut Option<Catalog>, options: &[ConfigOption]) -> bool {
    let Some(next) = Catalog::merge(catalog.as_ref(), options, &Desired::new()) else {
        return false;
    };
    *catalog = Some(next);
    true
}
