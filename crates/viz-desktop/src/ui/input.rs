//! Text inputs, search fields, and labeled form field layouts.

use rune::{El, KeyEvent, TextEdit, Tx, col, icon, row, text};
use rune_kit::TextInput;

use super::icons::{CIRCLE_ALERT, SEARCH};
use crate::theme::{ICON_SMALL, S4, S6, Size, Theme};

type InputHook = Box<dyn FnOnce(TextInput) -> TextInput>;

/// Standard single-line text input configured with viz's [`Size`] height and
/// full parent width.
pub fn text_field(edit: &TextEdit, label: &str, placeholder: &str, size: Size) -> TextInput {
    TextInput::new(edit)
        .label(label)
        .placeholder(placeholder)
        .height(size.height())
        .width(None)
}

/// Visual chrome for [`SearchInput`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SearchStyle {
    /// Standalone bordered search box (`1px` `border_control` hairline) for
    /// sidebars and filter bars.
    #[default]
    Bordered,
    /// Borderless search header inside a combobox, model picker, or command
    /// palette.
    Plain,
}

/// Search field with leading [`SEARCH`] icon, optional clear button when
/// non-empty, active-descendant wiring, and bordered/plain styles.
pub struct SearchInput {
    edit: TextEdit,
    placeholder: String,
    label: Option<String>,
    style: SearchStyle,
    size: Size,
    disabled: bool,
    invalid: bool,
    active: Option<String>,
    trailing: Option<El>,
    hooks: Vec<InputHook>,
}

/// Starts a [`SearchInput`] bound to `edit`.
pub fn search_input(edit: &TextEdit, placeholder: impl Into<String>) -> SearchInput {
    SearchInput {
        edit: edit.clone(),
        placeholder: placeholder.into(),
        label: None,
        style: SearchStyle::Bordered,
        size: Size::Compact,
        disabled: false,
        invalid: false,
        active: None,
        trailing: None,
        hooks: Vec::new(),
    }
}

impl SearchInput {
    /// Sets the accessible label (defaults to `placeholder`).
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Chooses [`SearchStyle::Bordered`] or [`SearchStyle::Plain`].
    pub const fn style(mut self, style: SearchStyle) -> Self {
        self.style = style;
        self
    }

    /// Sets the input height and typography density (defaults to
    /// [`Size::Compact`]).
    pub const fn size(mut self, size: Size) -> Self {
        self.size = size;
        self
    }

    /// Disables editing and dims the search field.
    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Highlights the input border in `t.danger` for invalid filter syntax.
    pub const fn invalid(mut self, invalid: bool) -> Self {
        self.invalid = invalid;
        self
    }

    /// Connects `aria-activedescendant` to the currently highlighted list row.
    pub fn active_descendant(mut self, key: Option<impl Into<String>>) -> Self {
        self.active = key.map(Into::into);
        self
    }

    /// Places a trailing accessory (such as a shortcut cap or result count).
    pub fn trailing(mut self, el: El) -> Self {
        self.trailing = Some(el);
        self
    }

    /// Binds the mutable [`TextEdit`] state.
    pub fn on_edit<S: 'static>(
        mut self,
        f: impl for<'a> Fn(&'a mut S) -> &'a mut TextEdit + 'static,
    ) -> Self {
        self.hooks.push(Box::new(move |input| input.on_edit(f)));
        self
    }

    /// Intercepts keyboard navigation (ArrowUp/Down, Enter, Escape).
    pub fn on_key<S: 'static>(
        mut self,
        f: impl Fn(&mut S, &KeyEvent, &mut Tx) -> bool + 'static,
    ) -> Self {
        self.hooks.push(Box::new(move |input| input.on_key(f)));
        self
    }

    /// Fires when the user presses Enter.
    pub fn on_submit<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.hooks.push(Box::new(move |input| input.on_submit(f)));
        self
    }

    /// Shows the trailing clear button whenever the query is non-empty and
    /// invokes `f` on press.
    pub fn on_clear<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        let non_empty = !self.edit.text.is_empty();
        self.hooks.push(Box::new(move |input| {
            if non_empty {
                input.clear_button().on_action(f)
            } else {
                input
            }
        }));
        self
    }

    /// Builds the search input against `t`.
    pub fn build(self, t: &Theme) -> El {
        let mut input = TextInput::new(&self.edit)
            .placeholder(self.placeholder)
            .height(self.size.height())
            .width(None)
            .leading_icon(&SEARCH)
            .leading_icon_size(self.size.icon_size())
            .text_style(self.size.font(t))
            .disabled(self.disabled)
            .invalid(self.invalid)
            .active_descendant(self.active);
        if let Some(label) = self.label {
            input = input.label(label);
        }
        if self.style == SearchStyle::Plain {
            input = input.plain();
        }
        if let Some(trailing) = self.trailing {
            input = input.trailing(trailing);
        }
        for hook in self.hooks {
            input = hook(input);
        }
        input.build(&t.k)
    }
}

/// Form field layout grouping a label, optional required marker or trailing
/// accessory, control slot, and quiet description or danger validation message.
pub struct Field {
    label: String,
    required: bool,
    description: Option<String>,
    error: Option<String>,
    trailing: Option<El>,
    control: El,
}

/// Starts a labeled [`Field`] around `control`.
pub fn field(label: impl Into<String>, control: El) -> Field {
    Field {
        label: label.into(),
        required: false,
        description: None,
        error: None,
        trailing: None,
        control,
    }
}

impl Field {
    /// Marks the field as required with a subtle `t.danger` indicator.
    pub const fn required(mut self) -> Self {
        self.required = true;
        self
    }

    /// Adds quiet helper text beneath the control.
    pub fn description(mut self, text: impl Into<String>) -> Self {
        self.description = Some(text.into());
        self
    }

    /// Shows a validation error beneath the control, superseding helper text
    /// emphasis.
    pub fn error(mut self, message: impl Into<String>) -> Self {
        self.error = Some(message.into());
        self
    }

    /// Places a trailing element on the label row (such as a badge or action).
    pub fn trailing(mut self, el: El) -> Self {
        self.trailing = Some(el);
        self
    }

    /// Builds the form field group against `t`.
    pub fn build(self, t: &Theme) -> El {
        let mut label_row = row()
            .key("field-label")
            .w_full()
            .gap(S4)
            .center_y()
            .a11y_hidden()
            .child(
                text(&self.label)
                    .font(t.small_medium)
                    .color(t.text)
                    .truncate(),
            );
        if self.required {
            label_row = label_row.child(text("*").font(t.small_medium).color(t.danger));
        }
        if let Some(trailing) = self.trailing {
            label_row = label_row.child(rune::spacer()).child(trailing);
        }

        let summary = self
            .error
            .as_deref()
            .or(self.description.as_deref())
            .unwrap_or_default();
        let mut group = col()
            .role("group")
            .label(&self.label)
            .description(summary)
            .w_full()
            .gap(S6)
            .items_start()
            .child(label_row)
            .child(self.control);

        if let Some(error) = self.error {
            group = group.child(
                row()
                    .key("field-error")
                    .w_full()
                    .gap(S4)
                    .center_y()
                    .a11y_hidden()
                    .child(icon(&CIRCLE_ALERT, ICON_SMALL).color(t.danger))
                    .child(text(error).font(t.small).color(t.danger).wrap()),
            );
        } else if let Some(description) = self.description {
            group = group.child(
                text(description)
                    .key("field-desc")
                    .font(t.small)
                    .color(t.muted)
                    .wrap()
                    .a11y_hidden(),
            );
        }
        group
    }
}
