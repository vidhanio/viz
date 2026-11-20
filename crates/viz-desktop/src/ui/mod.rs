//! The desktop's components on Rune and rune-kit. Each takes a [`Size`] or a
//! variant and owns its insets; views compose them and add no padding of
//! their own beyond the layout tokens in `theme`.

use rune::{Color, Edge, El, Icon, TextEdit, Transition, Tx, col, el, icon, row, text};
use rune_kit::{
    Button, ButtonVariant, IconButton, IconButtonVariant, Keybind, Loader, Menu, MenuItem,
    Placement, PopupList, Select, SelectAppearance, TextInput,
};
use viz_client::protocol::ConversationStatus;

use crate::theme::{
    CARD_INSET, CARD_WIDTH, DIALOG_INSET, DIALOG_WIDTH, DOT, EMPTY_WIDTH, HAIRLINE, ICON,
    ICON_SMALL, RADIUS_CARD, READING_INSET, READING_WIDTH, ROW_INSET, S2, S4, S8, S12, Size,
    TRAILING, TREE_ROW, TREE_ROW_TALL, TREE_STEP, Theme,
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
        let frame = col().w_full().radius(RADIUS_CARD);
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

/// The most lines a dialog's or toast's title wraps to before "…".
const TITLE_LINES: usize = 2;
/// The most lines a toast's detail (an error, a list of paths) shows.
const TOAST_DETAIL_LINES: usize = 4;

/// A dialog: a title of up to [`TITLE_LINES`], what it's about (`subject`,
/// one quiet line cut in the middle so a long path or chat title keeps both
/// ends) and a description, beside the close button's own column; then the
/// body, which insets itself like [`dialog_body`].
pub struct Dialog<'a> {
    key: &'a str,
    title: &'a str,
    subject: Option<&'a str>,
    detail: Option<&'a str>,
}

pub const fn dialog<'a>(key: &'a str, title: &'a str, detail: Option<&'a str>) -> Dialog<'a> {
    Dialog {
        key,
        title,
        subject: None,
        detail,
    }
}

impl<'a> Dialog<'a> {
    pub const fn subject(mut self, subject: &'a str) -> Self {
        self.subject = Some(subject);
        self
    }

    pub fn build<S: 'static>(
        self,
        close: impl Fn(&mut S) + Clone + 'static,
        body: El,
        t: &Theme,
        layer: &mut Vec<El>,
    ) {
        let mut head = col().grow(1.0).shrink().min_w(0.0).gap(S4).child(
            text(self.title)
                .font(t.title)
                .color(t.text)
                .wrap_anywhere()
                .max_lines(TITLE_LINES)
                .w_full(),
        );
        if let Some(subject) = self.subject {
            head = head.child(
                text(subject)
                    .font(t.ui)
                    .color(t.muted)
                    .truncate_middle()
                    .w_full(),
            );
        }
        if let Some(detail) = self.detail {
            head = head.child(
                text(detail)
                    .font(t.ui)
                    .color(t.muted)
                    .wrap_anywhere()
                    .w_full(),
            );
        }
        // The button's box is a control tall around a title-line glyph: pull
        // it up and out so the glyph sits on the title's line and the inset.
        let dismiss = icon_button(&CLOSE, "Close", IconButtonVariant::Ghost, Size::Small)
            .on_press(close.clone())
            .build(&t.k)
            .key("close")
            .mt(-S2)
            .mr(-S4);
        let header = row()
            .key("header")
            .w_full()
            .padding(DIALOG_INSET)
            .gap(S8)
            .items_start()
            .child(head)
            .child(dismiss);
        let panel = col()
            .label(self.title)
            .description(self.detail.unwrap_or_default())
            .child(header)
            .child(col().key("body").w_full().child(body));
        modal(self.key, panel, DIALOG_WIDTH, close, t, layer);
    }
}

/// A dialog without a title row, for an overlay whose first control says
/// what it is (the palette's search field).
pub fn sheet<S: 'static>(
    key: &str,
    label: &str,
    width: f32,
    close: impl Fn(&mut S) + 'static,
    body: impl IntoIterator<Item = El>,
    t: &Theme,
    layer: &mut Vec<El>,
) {
    modal(
        key,
        col().label(label).children(body),
        width,
        close,
        t,
        layer,
    );
}

/// rune-kit's `Dialog` scrim, panel, outline and dismissal (Escape, a click
/// outside) around `panel`.
fn modal<S: 'static>(
    key: &str,
    panel: El,
    width: f32,
    close: impl Fn(&mut S) + 'static,
    t: &Theme,
    layer: &mut Vec<El>,
) {
    let k = &t.k;
    let panel = k.shadow_layers(&k.elevation.overlay).fold(
        panel
            .key(format!("{key}-dialog"))
            .role("dialog")
            .w_full()
            .max_w(width)
            .radius(Size::Default.radius())
            .fill(k.c.layer_01)
            .token_motion(k.motion.overlay)
            .modal(true, close),
        El::box_shadow,
    );
    layer.push(
        el().key(format!("{key}-scrim"))
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .fill(k.c.scrim)
            .token_motion(k.motion.overlay)
            .a11y_hidden(),
    );
    layer.push(
        el().key(format!("{key}-layer"))
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .padding(DIALOG_INSET)
            .center()
            .child(panel),
    );
}

/// A toast: `mark`, a title of up to [`TITLE_LINES`] and a detail of up to
/// [`TOAST_DETAIL_LINES`], beside the close button's own column. Long words
/// (paths, URLs) break anywhere rather than overflow.
pub fn toast<S: 'static>(
    key: String,
    title: &str,
    detail: Option<&str>,
    mark: El,
    close: impl Fn(&mut S) + 'static,
    t: &Theme,
) -> El {
    let k = &t.k;
    let mut content = col()
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .gap(S4)
        .a11y_hidden()
        .child(
            text(title)
                .font(t.ui_medium)
                .color(t.text)
                .wrap_anywhere()
                .max_lines(TITLE_LINES)
                .w_full(),
        );
    if let Some(detail) = detail {
        content = content.child(
            text(detail)
                .font(t.ui)
                .color(t.muted)
                .wrap_anywhere()
                .max_lines(TOAST_DETAIL_LINES)
                .w_full(),
        );
    }
    let dismiss = icon_button(&CLOSE, "Close", IconButtonVariant::GhostMuted, Size::Small)
        .on_press(close)
        .build(k)
        .key("close")
        .mt(-S2)
        .mr(-S4);
    let panel = col()
        .key(key)
        .role("status")
        .label(title)
        .description(detail.unwrap_or_default())
        .w(CARD_WIDTH)
        .padding(CARD_INSET)
        .radius(RADIUS_CARD)
        .fill(k.c.layer_01)
        .child(
            row()
                .w_full()
                .gap(S12)
                .items_start()
                .child(mark.a11y_hidden())
                .child(content)
                .child(dismiss),
        );
    k.shadow_layers(&k.elevation.floating)
        .fold(panel, El::box_shadow)
        .transition(Transition::move_edge(Edge::Bottom))
        .token_motion(k.motion.overlay)
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
    /// What the action is about, as [`Dialog::subject`].
    pub subject: Option<&'a str>,
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
        let dialog = dialog("edit", self.title, self.detail);
        match self.subject {
            Some(subject) => dialog.subject(subject),
            None => dialog,
        }
        .build(close, dialog_body(input, actions), t, layer);
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
