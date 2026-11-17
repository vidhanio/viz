//! viz desktop app: conversations in a sidebar, the selected one's transcript,
//! and a message field. `cargo run -p viz-desktop` opens the window; `-- --snap
//! out.png` renders it headless.

mod backend;
mod transcript;

use std::time::Duration;

use rune::{
    App, AppSpec, Cx, El, ScrollSpec, TextEdit, col, el, row, text, text_input,
    tok::{self, Theme, radius, space, type_},
};
use tokio::sync::mpsc;
use viz_client::protocol::ConversationId;

use crate::{
    backend::Command,
    transcript::{Line, Role},
};

/// Longest a headless run waits for the first connection attempt before
/// rendering.
const HEADLESS_CONNECT_WAIT: Duration = Duration::from_secs(5);

/// The backend connection, as the status line shows it.
#[derive(Debug)]
pub(crate) enum Status {
    /// Trying to connect.
    Connecting,
    /// Connected to a backend of this version.
    Connected {
        /// Backend version.
        version: String,
    },
    /// Not connected, and why.
    Error(String),
}

/// The window's state. The backend thread fills it in through the app handle.
pub(crate) struct Desktop {
    commands: mpsc::UnboundedSender<Command>,
    status: Status,
    /// The orchestrator's conversation, once connected.
    root: Option<ConversationId>,
    /// Every other conversation.
    conversations: Vec<ConversationId>,
    selected: Option<ConversationId>,
    transcript: Vec<Line>,
    /// The latest failed request, shown under the message field.
    notice: Option<String>,
    draft: TextEdit,
}

impl Desktop {
    fn new(commands: mpsc::UnboundedSender<Command>) -> Self {
        Self {
            commands,
            status: Status::Connecting,
            root: None,
            conversations: Vec::new(),
            selected: None,
            transcript: Vec::new(),
            notice: None,
            draft: TextEdit::new(""),
        }
    }

    /// Shows `conversation`, empty until its view arrives.
    fn show(&mut self, conversation: ConversationId) {
        if self.selected != Some(conversation) {
            self.selected = Some(conversation);
            self.transcript.clear();
        }
    }

    fn send(&mut self, command: Command) {
        if self.commands.send(command).is_err() {
            self.notice = Some("The connection thread stopped".into());
        }
    }

    fn select(&mut self, conversation: ConversationId) {
        self.show(conversation);
        self.send(Command::Select(conversation));
    }

    fn submit(&mut self) {
        let text = self.draft.text.trim().to_owned();
        if text.is_empty() {
            return;
        }
        let Some(conversation) = self.selected else {
            self.notice = Some("Not connected to the backend".into());
            return;
        };
        self.notice = None;
        self.draft.clear();
        self.send(Command::Submit { conversation, text });
    }

    fn title(&self, conversation: ConversationId) -> String {
        if self.root == Some(conversation) {
            "Orchestrator".into()
        } else {
            format!("Conversation {conversation}")
        }
    }
}

/// A state with no connection thread behind it, for `AppSpec::init`; `main`
/// replaces it.
fn detached() -> Desktop {
    Desktop::new(mpsc::unbounded_channel().0)
}

fn view(s: &Desktop, cx: &mut Cx<Desktop>) -> El {
    let t = tok::theme(cx.system_dark());
    row()
        .key("window")
        .w_full()
        .h_full()
        .fill(t.bg)
        .child(sidebar(s, t))
        .child(main_pane(s, t))
}

fn sidebar(s: &Desktop, t: Theme) -> El {
    let conversations = s.root.into_iter().chain(s.conversations.iter().copied());
    let list = col().key("conversations").gap(2.0).children(
        conversations.map(|id| conversation_row(id, s.title(id), s.selected == Some(id), t)),
    );
    col()
        .key("sidebar")
        .w(220.0)
        .h_full()
        .min_h(0.0)
        .padding(space::S3)
        .gap(space::S2)
        .fill(t.card)
        .border_sides([0.0, 1.0, 0.0, 0.0], t.separator)
        .child(
            row()
                .key("new-chat")
                .role("button")
                .label("New chat")
                .h(28.0)
                .px(space::S3)
                .center()
                .radius(radius::MD)
                .fill(t.control_off)
                .hover(t.hover)
                .on_press(|s: &mut Desktop| s.send(Command::Create))
                .child(text("New chat").font(type_::LABEL).color(t.text)),
        )
        .child(
            list.scroll(ScrollSpec {
                stick_bottom: false,
            })
            .grow(1.0)
            .min_h(0.0),
        )
        .child(status_line(&s.status, t))
}

fn conversation_row(id: ConversationId, title: String, selected: bool, t: Theme) -> El {
    let row = row()
        .key(format!("conversation-{id}"))
        .role("button")
        .label(title.clone())
        .h(28.0)
        .px(space::S2)
        .center_y()
        .radius(radius::SM)
        .hover(t.hover)
        .on_press(move |s: &mut Desktop| s.select(id))
        .child(text(title).font(type_::BODY).color(t.text).truncate());
    if selected {
        row.fill(t.control_off)
    } else {
        row
    }
}

fn status_line(status: &Status, t: Theme) -> El {
    let (label, color, detail) = match status {
        Status::Connecting => ("Connecting…".to_owned(), t.text_secondary, None),
        Status::Connected { version } => {
            (format!("Connected · v{version}"), t.text_secondary, None)
        }
        Status::Error(message) => (
            "Disconnected".to_owned(),
            rune::oklch(0.65, 0.2, 25.0),
            Some(message),
        ),
    };
    let status = col()
        .key("status")
        .w_full()
        .gap(space::S1)
        .child(text(label).key("state").font(type_::LABEL).color(color));
    match detail {
        Some(detail) => status.child(
            text(detail.clone())
                .key("detail")
                .font(type_::LABEL)
                .color(t.text_secondary)
                .wrap_anywhere()
                .w_full(),
        ),
        None => status,
    }
}

fn main_pane(s: &Desktop, t: Theme) -> El {
    let title = s
        .selected
        .map_or_else(|| "Not connected".to_owned(), |id| s.title(id));
    let lines = col()
        .key("transcript")
        .w_full()
        .gap(space::S3)
        .padding(space::S6)
        .children(s.transcript.iter().map(|line| transcript_line(line, t)));
    let field = el()
        .key("draft-field")
        .w_full()
        .min_h(36.0)
        .px(space::S3)
        .py(space::S2)
        .radius(radius::MD)
        .fill(t.card)
        .border(1.0, t.card_border)
        .center_y()
        .child(
            text_input(&s.draft, type_::BODY)
                .key("draft")
                .label("Message")
                .w_full()
                .color(t.text)
                .placeholder("Message", t.text_secondary)
                .on_edit(|s: &mut Desktop| &mut s.draft)
                .on_submit(|s: &mut Desktop, _| s.submit()),
        );
    let mut composer = col()
        .key("composer")
        .w_full()
        .px(space::S6)
        .pb(space::S6)
        .gap(space::S2)
        .child(field);
    if let Some(notice) = &s.notice {
        composer = composer.child(
            text(notice.clone())
                .key("notice")
                .font(type_::LABEL)
                .color(t.text_secondary)
                .wrap(),
        );
    }
    col()
        .key("main")
        .grow(1.0)
        .h_full()
        .min_w(0.0)
        .min_h(0.0)
        .child(
            row()
                .key("header")
                .w_full()
                .h(44.0)
                .px(space::S6)
                .center_y()
                .border_sides([0.0, 0.0, 1.0, 0.0], t.separator)
                .child(text(title).key("title").font(type_::TITLE).color(t.text)),
        )
        .child(
            lines
                .scroll(ScrollSpec { stick_bottom: true })
                .grow(1.0)
                .min_h(0.0),
        )
        .child(composer)
}

fn transcript_line(line: &Line, t: Theme) -> El {
    let body = text(line.text.clone()).font(type_::BODY).w_full();
    match line.role {
        Role::User => col()
            .key(line.key.clone())
            .w_full()
            .gap(space::S1)
            .child(text("You").font(type_::LABEL).color(t.text_secondary))
            .child(body.color(t.text).wrap()),
        Role::Assistant => body.key(line.key.clone()).color(t.text).wrap(),
        Role::Tool => text(format!("→ {}", line.text))
            .key(line.key.clone())
            .font(type_::LABEL)
            .color(t.text_secondary)
            .w_full()
            .truncate(),
    }
}

fn main() {
    let (commands, receiver) = mpsc::unbounded_channel();
    let app = App::new(AppSpec {
        min_size: Some((640.0, 420.0)),
        ..AppSpec::new("viz", (960.0, 640.0), detached, view)
    })
    .init(move || Desktop::new(commands));
    let (first, connected) = std::sync::mpsc::channel();
    backend::spawn(app.handle(), receiver, first);
    if rune::is_headless() {
        // A snapshot shows the first attempt's outcome, not a race with it.
        _ = connected.recv_timeout(HEADLESS_CONNECT_WAIT);
    }
    app.run();
}
