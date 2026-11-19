//! The desktop's components on Rune and rune-kit. Each takes a [`Size`] or a
//! variant and owns its insets; views compose them and add no padding of
//! their own beyond the layout tokens in `theme`.

use rune::{Color, El, Icon, TextEdit, Tx, col, el, icon, row, text};
use rune_kit::{
    Button, ButtonVariant, Dialog, IconButton, IconButtonVariant, Keybind, Loader, Menu, MenuItem,
    Placement, PopupList, Select, SelectAppearance, TextInput,
};
use viz_client::protocol::ConversationStatus;

use crate::theme::{
    CARD_INSET, DIALOG_INSET, DIALOG_WIDTH, DOT, EMPTY_WIDTH, HAIRLINE, ICON, ICON_SMALL,
    RADIUS_CARD, READING_INSET, READING_WIDTH, ROW_INSET, S4, S8, S12, Size, TRAILING, TREE_ROW,
    TREE_ROW_TALL, TREE_STEP, Theme,
};

pub const COMMAND_KEY: &str = if cfg!(target_os = "macos") {
    "⌘"
} else {
    "Ctrl"
};
/// The sidebar toggle's shortcut, as its tooltip shows it.
pub const SIDEBAR_SHORTCUT: &str = if cfg!(target_os = "macos") {
    "⌘B"
} else {
    "Ctrl+B"
};
/// The Agents card's shortcut.
pub const AGENTS_SHORTCUT: &str = if cfg!(target_os = "macos") {
    "⇧⌘B"
} else {
    "Ctrl+Shift+B"
};

pub fn button(label: impl Into<String>, variant: ButtonVariant, size: Size) -> Button {
    Button::new(label).variant(variant).control_size(size.kit())
}

pub fn icon_button(
    glyph: &'static Icon,
    label: &str,
    variant: IconButtonVariant,
    size: Size,
) -> IconButton {
    IconButton::new(glyph)
        .variant(variant)
        .label(label)
        .control_size(size.kit())
}

pub fn kbd(key: &str, t: &Theme) -> El {
    Keybind::new(&[COMMAND_KEY, key]).build(&t.k)
}

/// A horizontal hairline across its parent.
pub fn rule(t: &Theme) -> El {
    el().w_full().h(HAIRLINE).fill(t.hairline).a11y_hidden()
}

/// A short vertical hairline between inline metadata.
pub fn divider(t: &Theme) -> El {
    el().w(HAIRLINE)
        .h(ICON_SMALL)
        .fill(t.hairline)
        .a11y_hidden()
}

/// A section title: small, medium, muted.
pub fn section(label: &str, t: &Theme) -> El {
    text(label).font(t.small_medium).color(t.muted)
}

/// Secondary one-line text that truncates.
pub fn quiet(label: &str, t: &Theme) -> El {
    text(label)
        .font(t.small)
        .color(t.muted)
        .min_w(0.0)
        .shrink()
        .truncate()
        .help_when_truncated(label)
}

/// A quiet read-only value for an inline control that can no longer change.
pub fn locked_label(key: &str, label: &str, detail: &str, t: &Theme) -> El {
    row()
        .key(key)
        .h(Size::Default.height())
        .px(S4)
        .center_y()
        .gap(S4)
        .max_w(PICKER_TRIGGER_MAX)
        .help(detail)
        .child(icon(&LOCK, ICON_SMALL).color(t.muted).a11y_hidden())
        .child(
            text(label)
                .font(t.small)
                .color(t.muted)
                .min_w(0.0)
                .truncate()
                .shrink(),
        )
}

/// The inline width reserved for a read-only value.
pub fn locked_label_width(label: &str, t: &Theme) -> f32 {
    (S4.mul_add(3.0, rune::text::measure(label, &[], t.small, None).0) + ICON_SMALL)
        .min(PICKER_TRIGGER_MAX)
}

/// A glyph centered in an icon-wide box one line tall, so it sits on the
/// first line of the text beside it.
pub fn lead(mark: El, line: f32) -> El {
    el().size(ICON, line).center().child(mark).a11y_hidden()
}

/// The centered reading column's frame; its parent owns the vertical spacing.
pub fn reading(child: El) -> El {
    col()
        .w_full()
        .px(READING_INSET)
        .items_center()
        .child(child.w_full().max_w(READING_WIDTH))
}

#[derive(Clone, Copy, Debug)]
pub enum Card {
    /// A raised surface with a hairline: tool output, the composer.
    Surface,
    /// A hairline only: plans, subagents.
    Outline,
    /// The quiet wash behind my messages.
    Message,
}

impl Card {
    /// The surface alone, for compound cards whose children own their insets.
    pub fn frame(self, t: &Theme) -> El {
        self.paint(col(), t)
    }

    /// [`Card::frame`] laid out as a row.
    pub fn row(self, t: &Theme) -> El {
        self.paint(row(), t)
    }

    fn paint(self, frame: El, t: &Theme) -> El {
        let frame = frame.w_full().radius(RADIUS_CARD);
        match self {
            Self::Surface => frame.fill(t.surface).border(HAIRLINE, t.hairline),
            Self::Outline => frame.border(HAIRLINE, t.hairline),
            Self::Message => frame.fill(t.mine),
        }
    }

    /// The surface with the card inset and gap.
    pub fn build(self, t: &Theme) -> El {
        self.frame(t).padding(CARD_INSET).gap(S8)
    }
}

/// An empty state: an optional mark, a title, a line of detail, an optional
/// action.
pub fn empty(mark: Option<El>, title: &str, detail: &str, action: Option<El>, t: &Theme) -> El {
    let mut body = col()
        .key("empty")
        .w_full()
        .max_w(EMPTY_WIDTH)
        .gap(S8)
        .items_center();
    if let Some(mark) = mark {
        body = body.child(mark.mb(S4));
    }
    body = body.child(text(title).font(t.title).color(t.text));
    if !detail.is_empty() {
        body = body.child(
            text(detail)
                .font(t.ui)
                .color(t.muted)
                .wrap()
                .align(rune::Align::Center),
        );
    }
    body.children(action.map(|action| action.mt(S8)))
}

pub fn dot(color: Color) -> El {
    el().size(DOT, DOT)
        .radius(DOT / 2.0)
        .fill(color)
        .a11y_hidden()
}

pub fn spinner(t: &Theme) -> El {
    Loader::new().size(ICON_SMALL).color(t.muted).build(&t.k)
}

/// A subagent's state as a mark and a word: a spinner while it runs, a
/// success dot when done, a danger dot when it failed.
pub fn agent_state(status: ConversationStatus, t: &Theme) -> (El, &'static str) {
    match status {
        ConversationStatus::Working => (spinner(t), "Running"),
        ConversationStatus::Failed => (dot(t.danger), "Failed"),
        ConversationStatus::Idle => (dot(t.success), "Done"),
    }
}

/// A conversation's run state as an icon-sized mark: a spinner while it works,
/// a danger dot after a failure, nothing while idle.
pub fn status(status: ConversationStatus, t: &Theme) -> Option<El> {
    let (mark, label) = match status {
        ConversationStatus::Idle => return None,
        ConversationStatus::Working => (spinner(t), "Working"),
        ConversationStatus::Failed => (dot(t.danger), "Failed"),
    };
    Some(
        el().size(ICON, ICON)
            .center()
            .child(mark)
            .role("status")
            .label(label)
            .help(label),
    )
}

pub fn text_field(edit: &TextEdit, label: &str, placeholder: &str, size: Size) -> TextInput {
    TextInput::new(edit)
        .label(label)
        .placeholder(placeholder)
        .height(size.height())
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

/// A sidebar row. Labels start at `SIDEBAR_INSET` plus one `TREE_STEP` per
/// depth (a leading icon takes one step), only the label shrinks, and the row
/// ends in its [`trailing`] slot.
pub struct TreeLine<'a> {
    pub key: &'a str,
    pub label: &'a str,
    /// A quiet second line; the row grows to `TREE_ROW_TALL`.
    pub detail: Option<&'a str>,
    pub depth: u16,
    /// Unread: the medium weight in full ink.
    pub strong: bool,
    /// Archived: faint.
    pub dim: bool,
    pub selected: bool,
}

impl TreeLine<'_> {
    pub fn build(
        self,
        leading: Option<El>,
        markers: impl IntoIterator<Item = El>,
        slot: El,
        t: &Theme,
    ) -> El {
        let color = if self.dim {
            t.faint
        } else if self.selected || self.strong {
            t.text
        } else {
            t.muted
        };
        let title = text(self.label)
            .font(if self.strong { t.ui_medium } else { t.ui })
            .color(color)
            .truncate()
            .help_when_truncated(self.label);
        let labels = col()
            .min_w(0.0)
            .shrink()
            .grow(1.0)
            .a11y_hidden()
            .child(title)
            .children(self.detail.map(|detail| {
                text(detail)
                    .font(t.small)
                    .color(t.faint)
                    .truncate()
                    .help_when_truncated(detail)
            }));
        let height = if self.detail.is_some() {
            TREE_ROW_TALL
        } else {
            TREE_ROW
        };
        let mut line = row()
            .key(self.key)
            .role("treeitem")
            .label(self.label)
            .level(u32::from(self.depth) + 1)
            .h(height)
            .w_full()
            .pl(f32::from(self.depth).mul_add(TREE_STEP, ROW_INSET))
            .gap(S8)
            .center_y()
            .radius(Size::Default.radius())
            .fill(if self.selected {
                t.selected
            } else {
                Color::TRANSPARENT
            })
            .hover(if self.selected { t.selected } else { t.hover })
            .hover_group_focus()
            .focusable(true)
            .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
            .children(leading.map(|mark| lead(mark, TREE_ROW)))
            .child(labels)
            .children(markers)
            .child(slot);
        if let Some(detail) = self.detail {
            line = line.description(detail);
        }
        line
    }
}

/// A row's fixed trailing slot. `rest` shows until the row is hovered or
/// focused, then `action` takes its place; `reveal` keeps the action up (its
/// menu is open).
pub fn trailing(rest: Option<El>, action: Option<El>, reveal: bool) -> El {
    let layer = |child: El| {
        el().pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
            .center()
            .child(child)
    };
    let mut slot = el().key("trailing").size(TRAILING, TRAILING);
    if let Some(rest) = rest {
        let hovered = if action.is_some() { 0.0 } else { 1.0 };
        let shown = if reveal { 0.0 } else { 1.0 };
        slot = slot.child(layer(rest).group_opacity(shown, hovered));
    }
    if let Some(action) = action {
        slot = slot.child(layer(action).group_opacity(if reveal { 1.0 } else { 0.0 }, 1.0));
    }
    slot
}

/// A sidebar section's title row, with an optional action in the trailing
/// slot.
pub fn section_row(key: &str, label: &str, action: Option<El>, t: &Theme) -> El {
    row()
        .key(key)
        .w_full()
        .h(TREE_ROW)
        .mt(S12)
        .pl(ROW_INSET)
        .gap(S8)
        .center_y()
        .child(section(label, t).grow(1.0))
        .child(trailing(None, action, true))
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
        .h(Size::Default.height())
        .max_w(PICKER_TRIGGER_MAX)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent);
    Menu::new(format!("{id}-picker"))
        .standard()
        .placement(Placement::TopStart)
        .min_width(MENU_WIDTH)
        .open(open)
        .on_close(close)
        .trigger(trigger)
}

/// The widest an inline select's trigger gets; rune-kit truncates its label
/// there.
pub const PICKER_TRIGGER_MAX: f32 = 156.0;
/// A menu's narrowest width.
pub const MENU_WIDTH: f32 = 196.0;
const COMPLETION_HEIGHT: f32 = 240.0;
pub const COMPLETION_WIDTH: f32 = 360.0;

pub fn menu_items(menu: Menu, items: impl IntoIterator<Item = MenuItem>) -> Menu {
    items.into_iter().fold(menu, Menu::item)
}

pub fn popup_list<S: 'static>(key: &str, anchor: &str) -> PopupList<S> {
    PopupList::new(key, anchor)
        .placement(Placement::TopStart)
        .offset(S4)
        .max_height(COMPLETION_HEIGHT)
        .animate_filtering(false)
        .open(true)
}

/// A dialog's frame: title, optional description, and a body inset like
/// rune-kit's header.
pub fn dialog(key: &str, title: &str, detail: Option<&str>) -> Dialog {
    let dialog = Dialog::new(key, title).width(DIALOG_WIDTH).open(true).fit();
    match detail {
        Some(detail) => dialog.description(detail),
        None => dialog,
    }
}

/// A dialog's body: its fields, then its actions, at the dialog inset.
pub fn dialog_body(fields: impl IntoIterator<Item = El>, actions: El) -> El {
    col()
        .w_full()
        .px(DIALOG_INSET)
        .pb(DIALOG_INSET)
        .gap(DIALOG_INSET)
        .children(fields)
        .child(actions)
}

/// A dialog's buttons, right-aligned, the primary last.
pub fn dialog_actions(buttons: impl IntoIterator<Item = El>) -> El {
    row().w_full().justify_end().gap(S8).children(buttons)
}

/// A labelled field in a dialog.
pub fn field(label: &str, input: El, t: &Theme) -> El {
    col()
        .w_full()
        .gap(S4)
        .child(text(label).font(t.small_medium).color(t.muted))
        .child(input)
}

/// A one-field (or no-field) confirmation dialog.
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
        let input = self.field.map(|(value, placeholder)| {
            text_field(value, placeholder, placeholder, Size::Default)
                .on_edit(edit)
                .on_submit(submit.clone())
                .build(&t.k)
                .key("edit-value")
        });
        let disabled = self
            .field
            .is_some_and(|(value, _)| value.text.trim().is_empty());
        let actions = dialog_actions([
            button("Cancel", ButtonVariant::Ghost, Size::Default)
                .on_press(close.clone())
                .build(&t.k),
            button(self.action, self.variant, Size::Default)
                .disabled(disabled)
                .on_press_tx(submit)
                .build(&t.k),
        ]);
        dialog("edit", self.title, self.detail)
            .on_close(close)
            .child(dialog_body(input, actions))
            .build(&t.k, layer);
    }
}

/// Declares Lucide icons as Rune icons: Lucide's own SVG on its 24-point
/// grid, one stroke width throughout, drawn at the `ICON*` token sizes.
macro_rules! lucide {
    ($($(#[$doc:meta])* $name:ident = $icon:ident;)*) => {$(
        $(#[$doc])*
        pub static $name: Icon = Icon {
            name: concat!("lucide-", stringify!($icon)),
            view: 24.0,
            body: lucide_static_svg::Icon::$icon.svg_str(),
        };
    )*};
}

lucide! {
    /// The orchestrator's mark.
    STAR = Sparkle;
    /// New chat.
    COMPOSE = SquarePen;
    PLUS = Plus;
    /// Send.
    ARROW_UP = ArrowUp;
    /// A subagent, and the Agents card.
    BOT = Bot;
    /// Back to a subagent's parent.
    CORNER_LEFT_UP = CornerLeftUp;
    CHEVRON = ChevronRight;
    CHEVRON_OPEN = ChevronDown;
    CHEVRON_UP = ChevronUp;
    MORE = Ellipsis;
    FOLDER = Folder;
    CHAT = MessageSquare;
    GIT_BRANCH = GitBranch;
    STOP = Square;
    INSPECTOR = PanelRight;
    SIDEBAR = PanelLeft;
    PIN = Pin;
    ARCHIVE = Archive;
    TRASH = Trash;
    PAPERCLIP = Paperclip;
    CHECK = Check;
    CLOSE = X;
    CIRCLE_X = CircleX;
    COPY = Copy;
    SEARCH = Search;
    LOCK = Lock;
    EYE = Eye;
    EYE_OFF = EyeOff;
    MINUS = Minus;
}

/// rune-kit's own glyphs, as Lucide.
pub fn kit_glyphs(glyph: &mut rune_kit::Glyphs) {
    glyph.check = &CHECK;
    glyph.check_small = &CHECK;
    glyph.chevron_down = &CHEVRON_OPEN;
    glyph.chevron_up = &CHEVRON_UP;
    glyph.caret_right = &CHEVRON;
    glyph.triangle_down = &CHEVRON_OPEN;
    glyph.triangle_right = &CHEVRON;
    glyph.close = &CLOSE;
    glyph.clear = &CIRCLE_X;
    glyph.copy = &COPY;
    glyph.search = &SEARCH;
    glyph.lock = &LOCK;
    glyph.eye = &EYE;
    glyph.eye_off = &EYE_OFF;
    glyph.circle_x = &CIRCLE_X;
    glyph.dash = &MINUS;
}

/// For `rune::set_icon_swap`: the icons rune-kit draws outside its glyph set
/// (the shell's sidebar toggle), as Lucide.
pub fn swap_kit_icon(icon: &Icon) -> Option<&'static Icon> {
    (icon.name == rune_kit::shell::SIDEBAR.name).then_some(&SIDEBAR)
}
