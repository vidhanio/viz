//! Disposable harness processes: listing native sessions, and learning a
//! harness's options for new threads.

use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

use agent_client_protocol::v1::schema::{SessionInfo, SessionNotification, SessionUpdate};
use anyhow::{Context as _, Result, anyhow, bail};

use super::{
    options::{Catalog, Desired, model_option, normalize, valid_value},
    process::Agent,
};
use crate::{
    config::HarnessConfig,
    wire::{ConfigOption, NativeThread, NativeThreads, OptionValue},
};

/// Bounds each disposable process.
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// The harness's native sessions in `cwd` (ACP `session/list`), one page at a
/// time.
///
/// # Errors
///
/// When the harness cannot import sessions (it needs `session/list` and
/// `session/load`), fails, or times out.
pub(crate) async fn list(
    config: &HarnessConfig,
    cwd: &Path,
    cursor: Option<&str>,
) -> Result<NativeThreads> {
    let listing = async {
        let agent = Agent::start(config, cwd, Arc::new(|_: SessionNotification| {})).await?;
        agent.require_import()?;
        let response = agent.list_sessions(cwd, cursor).await?;
        Ok::<_, anyhow::Error>(NativeThreads {
            threads: response.sessions.into_iter().map(native_thread).collect(),
            next_cursor: response.next_cursor,
        })
    };
    tokio::time::timeout(PROBE_TIMEOUT, listing)
        .await
        .map_err(|_| anyhow!("listing ACP sessions timed out"))?
}

fn native_thread(session: SessionInfo) -> NativeThread {
    NativeThread {
        session_id: session.session_id.0.to_string(),
        cwd: session.cwd.display().to_string(),
        title: session.title,
        updated_at: session.updated_at.as_deref().and_then(unix_millis),
    }
}

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

/// Unix milliseconds of an RFC 3339 timestamp such as
/// `2026-11-20T05:58:49.443Z`.
fn unix_millis(timestamp: &str) -> Option<i64> {
    let (date, time) = timestamp.split_once(['T', 't', ' '])?;
    let mut date = date.splitn(3, '-').map(str::parse::<i64>);
    let (year, month, day) = (date.next()?.ok()?, date.next()?.ok()?, date.next()?.ok()?);
    let offset_at = time.find(['Z', 'z', '+', '-']).unwrap_or(time.len());
    let (clock, zone) = time.split_at(offset_at);
    let (clock, fraction) = clock.split_once('.').unwrap_or((clock, ""));
    let mut clock = clock.splitn(3, ':').map(str::parse::<i64>);
    let (hour, minute, second) = (
        clock.next()?.ok()?,
        clock.next()?.ok()?,
        clock.next()?.ok()?,
    );
    let millis: i64 = format!("{fraction:0<3}").get(..3)?.parse().ok()?;
    let offset_minutes = match zone.as_bytes().first() {
        None | Some(b'Z' | b'z') => 0,
        Some(sign) => {
            let (hours, minutes) = zone[1..].split_once(':')?;
            let minutes = hours.parse::<i64>().ok()? * 60 + minutes.parse::<i64>().ok()?;
            if *sign == b'-' { -minutes } else { minutes }
        }
    };
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    // Days since 1970-01-01 in the proleptic Gregorian calendar (Howard
    // Hinnant's `days_from_civil`).
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    let seconds = days * 86_400 + hour * 3600 + minute * 60 + second - offset_minutes * 60;
    Some(seconds * 1000 + millis)
}
