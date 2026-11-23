//! Text inputs, search fields, and labeled form field layouts.

use rune::{El, KeyEvent, TextEdit, Tx, col, icon, row, text, text_input};
use rune_kit::{IconButtonVariant, TextInput};

use super::{
    icon_button::icon_button,
    icons::{CIRCLE_ALERT, CLOSE, SEARCH},
};
use crate::theme::{HAIRLINE, ICON_SMALL, S4, S6, S8, Size, Theme};

type ElHook = Box<dyn FnOnce(El) -> El>;

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

/// Flat base16 search field with leading [`SEARCH`] icon, optional clear button
/// when non-empty, exact `input_key` for [`super::ListRow::focus_ring_for`],
/// active-descendant wiring, and bordered/plain styles.
pub struct SearchInput {
    edit: TextEdit,
    placeholder: String,
    label: Option<String>,
    input_key: Option<String>,
    style: SearchStyle,
    size: Size,
    disabled: bool,
    invalid: bool,
    active: Option<String>,
    trailing: Option<El>,
    clear: Option<ElHook>,
    hooks: Vec<ElHook>,
}

/// Starts a [`SearchInput`] bound to `edit`.
pub fn search_input(edit: &TextEdit, placeholder: impl Into<String>) -> SearchInput {
    SearchInput {
        edit: edit.clone(),
        placeholder: placeholder.into(),
        label: None,
        input_key: None,
        style: SearchStyle::Bordered,
        size: Size::Compact,
        disabled: false,
        invalid: false,
        active: None,
        trailing: None,
        clear: None,
        hooks: Vec::new(),
    }
}

impl SearchInput {
    /// Sets the accessible label (defaults to `placeholder`).
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Sets the element key on the inner focusable text input so sibling rows
    /// can target it with [`super::ListRow::focus_ring_for`].
    pub fn input_key(mut self, key: impl Into<String>) -> Self {
        self.input_key = Some(key.into());
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
    #[expect(dead_code, reason = "validation state hook for Phase 1 filter inputs")]
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

    /// Runs after the editable text changes.
    pub fn on_change<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.hooks
            .push(Box::new(move |input| input.on_input_change(f)));
        self
    }

    /// Intercepts keyboard navigation (ArrowUp/Down, Enter, Escape).
    pub fn on_key<S: 'static>(
        mut self,
        f: impl Fn(&mut S, &KeyEvent, &mut Tx) -> bool + 'static,
    ) -> Self {
        self.hooks
            .push(Box::new(move |input| input.on_input_key(f)));
        self
    }

    /// Fires when the user presses Enter.
    #[expect(dead_code, reason = "submit hook for Phase 1 search inputs")]
    pub fn on_submit<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.hooks.push(Box::new(move |input| input.on_submit(f)));
        self
    }

    /// Shows a trailing clear button whenever the query is non-empty and
    /// invokes `f` on press.
    pub fn on_clear<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.clear = Some(Box::new(move |btn| btn.on_press_tx(f)));
        self
    }

    /// Builds the flat search input against `t`.
    pub fn build(self, t: &Theme) -> El {
        let non_empty = !self.edit.text.is_empty();
        let a11y_label = self
            .label
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(&self.placeholder);
        let ink = if self.invalid && !self.disabled {
            t.danger
        } else {
            t.text
        };
        let hint = if self.invalid && !self.disabled {
            t.danger
        } else {
            t.faint
        };
        let mut editor = text_input(&self.edit, self.size.font(t))
            .single_line()
            .key(self.input_key.as_deref().unwrap_or("search-input"))
            .label(a11y_label)
            .placeholder(self.placeholder, hint)
            .color(ink)
            .caret(ink, t.k.selection())
            .disabled(self.disabled)
            .no_focus_ring()
            .grow(1.0)
            .shrink()
            .min_w(0.0);
        if let Some(active) = self.active {
            editor = editor.active_descendant(active);
        }
        if !self.disabled {
            for hook in self.hooks {
                editor = hook(editor);
            }
        }

        let mut shell = row()
            .w_full()
            .h(self.size.height())
            .px(S8)
            .gap(S6)
            .center_y()
            .child(
                icon(&SEARCH, self.size.icon_size())
                    .color(t.icon_muted)
                    .a11y_hidden(),
            )
            .child(editor);

        if non_empty
            && !self.disabled
            && let Some(clear) = self.clear
        {
            let btn = icon_button(
                &CLOSE,
                "Clear search",
                IconButtonVariant::GhostMuted,
                Size::Tiny,
            )
            .build(&t.k)
            .key("clear");
            shell = shell.child(clear(btn));
        }
        if let Some(trailing) = self.trailing {
            shell = shell.child(trailing);
        }

        if self.style == SearchStyle::Bordered {
            let border = if self.invalid && !self.disabled {
                t.danger
            } else {
                t.border_control
            };
            let focus = if self.invalid { t.danger } else { t.accent };
            shell = shell
                .radius(self.size.radius())
                .fill(t.canvas)
                .border(HAIRLINE, border);
            if !self.disabled {
                shell = shell
                    .hover(t.hover_wash)
                    .focus_within_ring(HAIRLINE, -HAIRLINE, focus);
            }
        }
        if self.disabled {
            shell = shell.layer_opacity(0.50);
        }
        shell
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
                    .items_start()
                    .a11y_hidden()
                    .child(icon(&CIRCLE_ALERT, ICON_SMALL).color(t.danger).mt(2.0))
                    .child(
                        text(error)
                            .font(t.small)
                            .color(t.danger)
                            .grow(1.0)
                            .shrink()
                            .min_w(0.0)
                            .wrap(),
                    ),
            );
        } else if let Some(description) = self.description {
            group = group.child(
                text(description)
                    .key("field-desc")
                    .w_full()
                    .font(t.small)
                    .color(t.muted)
                    .wrap()
                    .a11y_hidden(),
            );
        }
        group
    }
}
