//! Disposable harness processes for learning a harness's options for new
//! threads.

use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

use agent_client_protocol::v1::schema::{SessionNotification, SessionUpdate};
use viz_protocol::{ConfigOption, OptionValue};

use super::options::{Catalog, Desired, model_option, normalize, valid_value, with_legacy_modes};
use crate::{
    Client, ClientEvents, Error, HarnessConfig, SessionSetup, error::Result,
    lifecycle::method_missing,
};

/// Bounds each disposable process.
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// Gets the options for new sessions and returns any catalog change to persist.
///
/// A disposable process fills a missing catalog or model profile. If a model
/// probe fails after discovering the base catalog, the base catalog is
/// returned.
///
/// # Errors
///
/// When `model` is not offered, or probing fails or times out.
pub async fn catalog(
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
            match probe(config, cwd, Some(requested), &mut catalog).await {
                Ok(probed) => changed |= probed,
                Err(error) if changed => {
                    tracing::warn!(model = requested, %error, "ACP model probe failed");
                }
                Err(error) => return Err(error),
            }
        }
        None if model.is_some() => return Err(Error::Unavailable("model".to_owned())),
        Some(_) | None => {}
    }
    let catalog = catalog.ok_or_else(|| Error::Unavailable("ACP options catalog".to_owned()))?;
    let options = catalog.options(config, model);
    Ok((options, changed.then_some(catalog)))
}

/// Checks the harness with a disposable process and returns its catalog
/// with the base options as reported now, keeping known model profiles.
///
/// # Errors
/// When the harness fails to start, open a session, or answer in time.
pub async fn refresh(
    config: &HarnessConfig,
    cwd: &Path,
    stored: Option<&Catalog>,
) -> Result<Catalog> {
    let mut catalog = stored.cloned();
    probe(config, cwd, None, &mut catalog).await?;
    catalog.ok_or_else(|| Error::Unavailable("ACP options catalog".to_owned()))
}

/// Learns `catalog` from a disposable process. Without a `model`, an agent
/// with `session/options` reports the defaults without creating a session.
/// Otherwise, since ACP cannot fetch settings for an inactive model, this asks
/// a prompt-free session after selecting `model`, then deletes it. Returns
/// whether `catalog` changed.
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
            Arc::new(move |notification: SessionNotification, _| {
                if let SessionUpdate::ConfigOptionUpdate(update) = notification.update {
                    *reported.lock().expect("probe lock poisoned") =
                        Some(normalize(update.config_options));
                }
            })
        };
        let agent = Client::start(config, cwd, sink, ClientEvents::default()).await?;
        if model.is_none()
            && let Some(options) = agent.session_options(cwd).await?
        {
            return Ok(merge(
                catalog,
                &with_legacy_modes(
                    normalize(options.config_options.unwrap_or_default()),
                    options.modes.as_ref(),
                ),
            ));
        }
        let response = agent.new_session(&SessionSetup::new(cwd)).await?;
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
        let options = with_legacy_modes(options, response.modes.as_ref());
        let result = select_model(&agent, &session, options, model, catalog).await;
        if agent.capabilities().delete
            && let Err(error) = agent.delete_session(&session).await
            && !method_missing(&error)
        {
            tracing::warn!("ACP {} catalog cleanup: {error:#}", agent.label());
        }
        result
    };
    tokio::time::timeout(PROBE_TIMEOUT, probing)
        .await
        .map_err(|_| Error::ProbeTimeout)?
}

/// Merges the probe session's `options` into `catalog`, then those after
/// selecting `model`, if given.
async fn select_model(
    agent: &Client,
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
        .ok_or_else(|| Error::Unavailable("model".to_owned()))?;
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
