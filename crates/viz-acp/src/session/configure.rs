//! Applies settings to a bound session and reconciles the reported values.

use viz_protocol::{ConfigOption, OptionValue};

use super::{
    Session,
    state::{lock, options_with_mode, set_options},
};
use crate::{
    Error,
    error::Result,
    options::{
        Desired, LEGACY_MODE, Selection, defaults, model_option, reconcile, thinking_option,
        valid_value,
    },
};

impl Session {
    /// Applies the thread's saved settings after [`Session::open`]: `desired`
    /// values, the configured defaults for a new session, and an exact
    /// `selection`, which fails unless the runner keeps it. Returns the
    /// settings to persist.
    ///
    /// # Errors
    /// Returns selection errors or errors while applying options.
    pub async fn restore(
        &self,
        desired: &Desired,
        selection: Option<&Selection>,
    ) -> Result<Desired> {
        let mut base = if self.restored {
            Desired::new()
        } else {
            defaults(&self.config, &self.options())
        };
        base.extend(
            desired
                .iter()
                .map(|(id, value)| (id.clone(), value.clone())),
        );
        let model = selection.and_then(|selection| selection.model.as_deref());
        let thinking = selection.and_then(|selection| selection.thinking.as_deref());
        let mut preferred = base.clone();
        let mut model_id = model_option(&self.options()).map(|option| option.id.clone());
        if let Some(model) = model {
            let id = self.set_exact(model_option, "Model", model).await?;
            preferred.insert(id.clone(), OptionValue::Select(model.to_owned()));
            model_id = Some(id);
        } else if let Some(id) = &model_id
            && let Some(value) = base.get(id)
        {
            self.set_option(id, value).await?;
        }
        if !self.restored {
            let mut with_defaults = defaults(&self.config, &self.options());
            with_defaults.extend(preferred);
            preferred = with_defaults;
        }
        let thinking_id =
            thinking.and_then(|_| thinking_option(&self.options()).map(|option| option.id.clone()));
        for (id, value) in &preferred {
            if Some(id) != model_id.as_ref() && Some(id) != thinking_id.as_ref() {
                self.set_option(id, value).await?;
            }
        }
        if let Some(thinking) = thinking {
            let id = self
                .set_exact(thinking_option, "Thinking level", thinking)
                .await?;
            preferred.insert(id, OptionValue::Select(thinking.to_owned()));
        }
        if let Some(model) = model {
            let retained = model_option(&self.options())
                .is_some_and(|option| option.value == OptionValue::Select(model.to_owned()));
            if !retained {
                return Err(Error::Selection(format!("model {model:?}")));
            }
        }
        Ok(reconcile(&self.options(), &preferred))
    }

    /// Sets option `id`, reapplying the preferred effort after a model change.
    /// `desired` is the thread's saved settings; returns them updated, to
    /// persist.
    ///
    /// # Errors
    /// Returns an error for an invalid option or a failed setting request.
    pub async fn configure(
        &self,
        id: &str,
        value: OptionValue,
        desired: &Desired,
    ) -> Result<Desired> {
        let options = self.options();
        let setting = options
            .iter()
            .find(|option| option.id == id)
            .ok_or_else(|| Error::Unavailable(format!("option {id}")))?;
        if !valid_value(setting, &value) {
            return Err(Error::InvalidOption(id.to_owned()));
        }
        let is_model = setting.category.as_deref() == Some("model");
        let mut desired = desired.clone();
        desired.insert(id.to_owned(), value.clone());
        self.set_option(id, &value).await?;
        if is_model
            && let Some(thinking) = thinking_option(&self.options()).map(|option| option.id.clone())
        {
            let preferred = desired
                .get(&thinking)
                .cloned()
                .or_else(|| self.config.effort.clone().map(OptionValue::Select));
            if let Some(preferred) = preferred {
                self.set_option(&thinking, &preferred).await?;
            }
        }
        Ok(reconcile(&self.options(), &desired))
    }

    /// Sets a mode from the latest mode catalog.
    ///
    /// # Errors
    /// Returns an error for an unavailable mode or a failed ACP request.
    pub async fn set_mode(&self, mode: &str) -> Result<()> {
        let offered = lock(&self.state).modes.as_ref().is_some_and(|modes| {
            modes
                .available_modes
                .iter()
                .any(|offered| offered.id.0.as_ref() == mode)
        });
        if !offered {
            return Err(Error::Unavailable(format!("mode {mode:?}")));
        }
        self.agent.set_mode(&self.id, mode).await?;
        let mut state = lock(&self.state);
        if let Some(modes) = &mut state.modes {
            modes.current_mode_id = crate::schema::SessionModeId::new(mode.to_owned());
        }
        state.dirty = true;
        drop(state);
        self.changes.send_replace(());
        Ok(())
    }

    /// Sets the title and returns the value to persist, using a local title
    /// when needed.
    ///
    /// # Errors
    /// Returns protocol and transport failures after native title delivery.
    pub async fn set_title(&self, title: &str) -> Result<String> {
        let title = self
            .agent
            .set_title(&self.id, title)
            .await?
            .map_or_else(|| title.to_owned(), |response| response.title);
        let mut state = lock(&self.state);
        state.title = Some(title.clone());
        state.dirty = true;
        drop(state);
        self.changes.send_replace(());
        Ok(title)
    }

    fn options(&self) -> Vec<ConfigOption> {
        options_with_mode(&lock(&self.state))
    }

    async fn set_option(&self, id: &str, value: &OptionValue) -> Result<()> {
        if id == LEGACY_MODE {
            if let OptionValue::Select(mode) = value {
                let active = lock(&self.state)
                    .modes
                    .as_ref()
                    .is_some_and(|modes| modes.current_mode_id.0.as_ref() == mode.as_str());
                if !active {
                    self.set_mode(mode).await?;
                }
            }
            return Ok(());
        }
        let unchanged = lock(&self.state)
            .options
            .iter()
            .find(|option| option.id == id)
            .is_none_or(|option| option.value == *value || !valid_value(option, value));
        if unchanged {
            return Ok(());
        }
        let response = self.agent.set_option(&self.id, id, value).await?;
        let mut state = lock(&self.state);
        set_options(&mut state, response.config_options);
        state.dirty = true;
        drop(state);
        self.changes.send_replace(());
        Ok(())
    }

    /// Sets the option `find` locates to exactly `value`, returning its id.
    async fn set_exact(
        &self,
        find: fn(&[ConfigOption]) -> Option<&ConfigOption>,
        label: &str,
        value: &str,
    ) -> Result<String> {
        let wanted = OptionValue::Select(value.to_owned());
        let id = find(&self.options())
            .filter(|option| valid_value(option, &wanted))
            .map(|option| option.id.clone())
            .ok_or_else(|| Error::Unavailable(format!("{label} {value:?}")))?;
        self.set_option(&id, &wanted).await?;
        let options = self.options();
        match find(&options) {
            Some(applied) if applied.value == wanted => Ok(applied.id.clone()),
            _ => Err(Error::Selection(format!(
                "{} {value:?}",
                label.to_lowercase()
            ))),
        }
    }
}
