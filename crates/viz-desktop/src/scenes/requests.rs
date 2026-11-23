//! Harness request previews using the production desktop surfaces.

use serde_json::json;
use viz_client::protocol::{
    FormValue, PermissionKind, PermissionOption, RequestBody, RequestId, RequestState, RunId,
    RunRequest,
};

use super::{data::THREAD, fixtures};
use crate::app::Desktop;

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
