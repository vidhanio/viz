//! Dock and harness request previews using the production desktop surfaces.

use serde_json::json;
use viz_client::protocol::{
    FormValue, PermissionKind, PermissionOption, RequestBody, RequestId, RequestState, RunId,
    RunRequest, Size, TerminalId, TerminalScope, TerminalSummary,
};
use viz_terminal::Exit;

use super::{data::THREAD, fixtures};
use crate::{
    app::Desktop,
    model::{
        Status,
        terminal::{Accessory, Place, Split},
    },
};

pub fn dock(app: &mut Desktop) {
    seed_dock(app, false);
}

pub fn disconnected(app: &mut Desktop) {
    seed_dock(app, true);
}

fn seed_dock(app: &mut Desktop, disconnected: bool) {
    fixtures::seed_workspace(&mut app.model);
    let project = app
        .model
        .project_path()
        .expect("preview project")
        .to_owned();
    let dock = &mut app.model.terminal;
    dock.environment = "local".into();
    dock.listed = true;
    dock.panel_was_open = true;
    let ids = [
        "00000000-0000-4000-8000-000000000001",
        "00000000-0000-4000-8000-000000000002",
        "00000000-0000-4000-8000-000000000003",
    ]
    .map(|id| TerminalId(id.parse().expect("preview UUID")));
    let checkout = TerminalScope::Thread { thread: THREAD };
    let exit = disconnected.then_some(Exit::Code(2));
    for (id, scope, title, exit) in [
        (ids[0], checkout.clone(), "Shell", exit),
        (ids[1], checkout, "Shell", None),
        (ids[2], TerminalScope::Project { project }, "Shell", None),
    ] {
        dock.sessions.push(TerminalSummary {
            id,
            scope,
            cwd: "/work/workbench".into(),
            title: title.into(),
            created_at: 0,
            exit,
        });
    }
    dock.place(ids[0], Place::Tab);
    if disconnected {
        app.model.connection.status = Status::Error("Connection interrupted".into());
    } else {
        dock.place(ids[1], Place::Split(ids[0], Split::Right));
        dock.active = Some(ids[0]);
        dock.accessory = Accessory::Find;
        dock.attached.extend(ids[..2].iter().copied());
    }
    app.model.panels.terminal_open = true;
    app.model.panels.terminal_height = 280.0;
    app.sync_widgets_from_model();
    let output = if disconnected {
        "\x1b[31merror\x1b[0m: missing package metadata\r\n"
    } else {
        "\x1b[32m~/work/workbench\x1b[0m $ cargo check\r\n    Checking workbench\r\n    Finished dev profile\r\n"
    };
    let size = Size::new(if disconnected { 96 } else { 48 }, 8);
    crate::terminal::panel::preview(
        app,
        ids[0],
        size,
        output,
        exit,
        if disconnected { "" } else { "Checking" },
    );
    if !disconnected {
        crate::terminal::panel::preview(
            app,
            ids[1],
            size,
            "\x1b[36m~/work/workbench\x1b[0m $ git status --short\r\n M src/main.rs\r\n",
            None,
            "",
        );
    }
}

pub fn approval(app: &mut Desktop) {
    fixtures::seed_activity(&mut app.model);
    add(
        app,
        RequestBody::Permission {
            call: "run-1".into(),
            title: "Check line counts".into(),
            options: [
                ("once", "Allow once", PermissionKind::AllowOnce),
                ("always", "Allow always", PermissionKind::AllowAlways),
                ("reject", "Reject once", PermissionKind::RejectOnce),
            ]
            .map(|(id, label, kind)| PermissionOption {
                id: id.into(),
                label: label.into(),
                kind,
            })
            .into(),
        },
    );
}

pub fn question(app: &mut Desktop) {
    seed_question(app, false);
}

pub fn multi_step(app: &mut Desktop) {
    seed_question(app, true);
}

fn seed_question(app: &mut Desktop, multi: bool) {
    fixtures::seed_workspace(&mut app.model);
    let mut schema = json!({
        "type": "object",
        "required": ["format"],
        "properties": {
            "format": {"type": "string", "title": "Export format", "oneOf": [
                {"const": "csv", "title": "CSV", "description": "Use the existing spreadsheet workflow"},
                {"const": "json", "title": "JSON", "description": "Preserve nested data"}
            ]}
        }
    });
    if multi {
        schema["properties"]["include"] = json!({"type":"array", "title":"Extra columns", "items":{"enum":["Author", "Timestamp", "Source"]}});
        schema["properties"]["name"] = json!({"type":"string", "title":"File name"});
    }
    add(
        app,
        RequestBody::Question {
            message: if multi {
                "How should I configure the **sample exports**?"
            } else {
                "Which format should I use for the **sample exports**?"
            }
            .into(),
            schema,
        },
    );
    if multi {
        let form = app.model.requests.forms.entry(RequestId(1)).or_default();
        form.step = 1;
        form.fields
            .insert("format".into(), FormValue::Text("csv".into()));
        form.fields.insert(
            "include".into(),
            FormValue::Choices(vec!["Timestamp".into()]),
        );
        crate::requests::sync(app);
    }
}

fn add(app: &mut Desktop, body: RequestBody) {
    app.model.requests.environment = app.model.active_env_key().to_owned();
    let state = app
        .model
        .cache_mut()
        .open_thread_mut(THREAD)
        .state
        .as_mut()
        .expect("preview thread");
    state.requests.push(RunRequest {
        id: RequestId(1),
        thread: THREAD,
        run: RunId(1),
        created_at: 0,
        body,
        state: RequestState::Pending,
    });
    app.sync_widgets_from_model();
    crate::requests::sync(app);
}
