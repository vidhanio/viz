//! The desktop's shared presentation on Rune and rune-kit.

use rune::{Color, El, Icon, TextEdit, Tx, col, el, row, text};
use rune_kit::{
    Badge, BadgeVariant, Button, ButtonVariant, ControlSize, Dialog, IconButton, IconButtonVariant,
    Keybind, Menu, MenuItem, Placement, PopupList, Select, SelectAppearance, Spinner, TextInput,
    TreeItem, TreeRow, TreeStyle,
};
use viz_client::protocol::ConversationStatus;

use crate::theme::Theme;

pub const COMMAND_KEY: &str = if cfg!(target_os = "macos") {
    "⌘"
} else {
    "Ctrl"
};

pub fn button(label: impl Into<String>, variant: ButtonVariant) -> Button {
    Button::new(label)
        .variant(variant)
        .control_size(ControlSize::Regular)
}

pub fn icon_button(glyph: &'static Icon, label: &str, variant: IconButtonVariant) -> IconButton {
    IconButton::new(glyph).variant(variant).label(label)
}

pub fn chip(label: impl Into<String>, t: &Theme) -> El {
    Badge::new(label).variant(BadgeVariant::Subtle).build(&t.k)
}

pub fn kbd(key: &str, t: &Theme) -> El {
    Keybind::new(&[COMMAND_KEY, key]).build(&t.k)
}

pub fn rule(t: &Theme) -> El {
    el().w_full().h(1.0).fill(t.hairline).a11y_hidden()
}

pub fn section(label: &str, t: &Theme) -> El {
    text(label).font(t.k.ty.small_medium).color(t.muted)
}

pub fn empty(title: &str, detail: &str, t: &Theme) -> El {
    col()
        .key("empty")
        .w_full()
        .max_w(360.0)
        .gap(6.0)
        .items_center()
        .child(text(title).font(t.title).color(t.text))
        .child(
            text(detail)
                .font(t.ui)
                .color(t.muted)
                .wrap()
                .align(rune::Align::Center),
        )
}

pub fn dot(color: Color, size: f32) -> El {
    el().size(size, size)
        .radius(size / 2.0)
        .fill(color)
        .a11y_hidden()
}

pub fn spinner(size: f32, t: &Theme) -> El {
    Spinner::new().size(size).color(t.muted).build(&t.k)
}

pub fn status(status: ConversationStatus, t: &Theme) -> Option<El> {
    let (icon, label) = match status {
        ConversationStatus::Idle => return None,
        ConversationStatus::Working => (spinner(13.0, t), "Working"),
        ConversationStatus::Approval => (dot(t.warning, 6.0), "Approval needed"),
        ConversationStatus::Failed => (dot(t.danger, 6.0), "Failed"),
    };
    Some(
        el().size(14.0, 14.0)
            .center()
            .child(icon)
            .role("status")
            .label(label)
            .help(label),
    )
}

pub fn text_field(edit: &TextEdit, label: &str, placeholder: &str) -> TextInput {
    TextInput::new(edit)
        .label(label)
        .placeholder(placeholder)
        .height(29.0)
        .width(None)
}

pub fn code(body: &str, t: &Theme) -> El {
    text(body)
        .font(t.mono)
        .color(t.text)
        .wrap_anywhere()
        .w_full()
        .selectable(true)
        .selection_color(t.k.selection())
}

pub fn tree_row(row: &TreeRow<'_>, item: TreeItem, t: &Theme) -> El {
    let mut style = TreeStyle::from_kit(&t.k)
        .label(t.ui, t.muted, t.text)
        .gap(5.0)
        .hover(t.hover);
    style.row_height = 29.0;
    style.indent = 14.0;
    style.guides = false;
    style.selected = t.selected;
    item.build(&t.k, row, &style)
        .key(row.id)
        .focusable(true)
        .focus_ring(1.0, -1.0, t.accent)
}

/// The combobox trigger is separate from its menu: server ids can differ even
/// when the labels match.
pub fn picker<S: 'static>(
    id: &str,
    name: &str,
    current: Option<&str>,
    open: bool,
    toggle: impl Fn(&mut S) + 'static,
    close: impl Fn(&mut S) + 'static,
    t: &Theme,
) -> Menu {
    let trigger = Select::new(id, &[] as &[&str])
        .appearance(SelectAppearance::Inline)
        .label(name)
        .placeholder(name)
        .current(current)
        .open(open)
        .on_toggle(toggle);
    let trigger = trigger
        .trigger(&t.k)
        .max_w(154.0)
        .focus_ring(1.0, -1.0, t.accent);
    Menu::new(format!("{id}-picker"))
        .standard()
        .placement(Placement::TopStart)
        .min_width(196.0)
        .open(open)
        .on_close(close)
        .trigger(trigger)
}

pub fn menu_items(menu: Menu, items: impl IntoIterator<Item = MenuItem>) -> Menu {
    items.into_iter().fold(menu, Menu::item)
}

pub fn popup_list<S: 'static>(key: &str, anchor: &str) -> PopupList<S> {
    PopupList::new(key, anchor)
        .placement(Placement::TopStart)
        .offset(6.0)
        .max_height(238.0)
        .animate_filtering(false)
        .open(true)
}

pub struct DialogForm<'a> {
    pub title: &'a str,
    pub detail: Option<&'a str>,
    pub field: Option<(&'a TextEdit, &'a str)>,
    pub action: &'a str,
    pub variant: ButtonVariant,
}

impl DialogForm<'_> {
    pub fn build<S: 'static>(
        self,
        edit: impl Fn(&mut S) -> &mut TextEdit + 'static,
        close: impl Fn(&mut S) + Clone + 'static,
        submit: impl Fn(&mut S, &mut Tx) + Clone + 'static,
        t: &Theme,
        layer: &mut Vec<El>,
    ) {
        let mut dialog = Dialog::new("edit", self.title)
            .width(392.0)
            .open(true)
            .fit()
            .on_close(close.clone());
        if let Some(detail) = self.detail {
            dialog = dialog.description(detail);
        }
        let mut body = col().w_full().px(16.0).pb(16.0).gap(16.0);
        if let Some((text, placeholder)) = self.field {
            body = body.child(
                text_field(text, placeholder, placeholder)
                    .on_edit(edit)
                    .on_submit(submit.clone())
                    .build(&t.k)
                    .key("edit-value"),
            );
        }
        let disabled = self
            .field
            .is_some_and(|(text, _)| text.text.trim().is_empty());
        dialog
            .child(
                body.child(
                    row()
                        .w_full()
                        .justify_end()
                        .gap(6.0)
                        .child(
                            button("Cancel", ButtonVariant::Ghost)
                                .on_press(close)
                                .build(&t.k),
                        )
                        .child(
                            button(self.action, self.variant)
                                .disabled(disabled)
                                .on_press_tx(submit)
                                .build(&t.k),
                        ),
                ),
            )
            .build(&t.k, layer);
    }
}

pub static MORE: Icon = Icon {
    name: "viz-more",
    view: 16.0,
    body: r#"<path d="M3.5 8h.01M8 8h.01M12.5 8h.01" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>"#,
};
pub static FOLDER: Icon = Icon {
    name: "viz-folder",
    view: 16.0,
    body: r#"<path d="M1.75 4.5h5l1.25 1.5h6.25v6.5H1.75z" fill="none" stroke="currentColor" stroke-width="1.25" stroke-linejoin="round"/>"#,
};
pub static CHAT: Icon = Icon {
    name: "viz-chat",
    view: 16.0,
    body: r#"<path d="M2.25 3.5h11.5v7.25H7l-3.25 2v-2H2.25z" fill="none" stroke="currentColor" stroke-width="1.25" stroke-linejoin="round"/>"#,
};
pub static GIT_BRANCH: Icon = Icon {
    name: "viz-git-branch",
    view: 16.0,
    body: r#"<path d="M4.5 6v4m1.5 1.4h1a4.5 4.5 0 0 0 4.5-4.5V6" fill="none" stroke="currentColor" stroke-width="1.2"/><circle cx="4.5" cy="4.5" r="1.5" fill="none" stroke="currentColor" stroke-width="1.2"/><circle cx="4.5" cy="11.5" r="1.5" fill="none" stroke="currentColor" stroke-width="1.2"/><circle cx="11.5" cy="4.5" r="1.5" fill="none" stroke="currentColor" stroke-width="1.2"/>"#,
};
pub static STOP: Icon = Icon {
    name: "viz-stop",
    view: 16.0,
    body: r#"<rect x="4" y="4" width="8" height="8" rx="1.25" fill="currentColor"/>"#,
};
