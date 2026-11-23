//! Composer-attached editor for an ACP elicitation form.

use rune::{El, Key, KeyEvent, ScrollSpec, TextEdit, Tx, col, icon, row, text};
use rune_kit::{ButtonVariant, IconButtonVariant, Markdown};
use serde_json::Value;
use viz_client::protocol::{FormValue, RequestAnswer, RequestBody, RequestId};

use crate::{
    app::Desktop,
    model::requests::{choices, fields, question, required},
    theme::{ICON_SMALL, S4, S8, S12, Size, Theme},
    ui,
    update::{Msg, requests::RequestMsg},
};

fn dispatch(app: &mut Desktop, message: RequestMsg, tx: &mut Tx) {
    app.dispatch(Msg::Requests(message), tx);
}

pub fn sync(app: &mut Desktop) {
    let current = question(&app.model).map(|request| {
        let step = app
            .model
            .requests
            .forms
            .get(&request.id)
            .map_or(0, |form| form.step);
        (request.id, step)
    });
    if app.widgets.request_step == current {
        return;
    }
    app.widgets.request_step = current;
    let value = current.and_then(|(id, step)| {
        let RequestBody::Question { schema, .. } = &question(&app.model)?.body else {
            return None;
        };
        let (name, _) = *fields(schema).get(step)?;
        let value = app.model.requests.forms.get(&id)?.fields.get(name)?;
        match value {
            FormValue::Text(text) => Some(text.clone()),
            FormValue::Number(number) => Some(number.to_string()),
            _ => None,
        }
    });
    app.widgets.request_edit = TextEdit::new(value.as_deref().unwrap_or_default());
}

fn select(app: &mut Desktop, id: RequestId, field: &Value, value: &str, tx: &mut Tx) {
    let kind = field.get("type").and_then(Value::as_str);
    if kind == Some("array") {
        let mut values = question(&app.model)
            .and_then(|request| {
                let RequestBody::Question { schema, .. } = &request.body else {
                    return None;
                };
                let form = app.model.requests.forms.get(&id)?;
                let (name, _) = *fields(schema).get(form.step)?;
                match form.fields.get(name)? {
                    FormValue::Choices(values) => Some(values.clone()),
                    _ => None,
                }
            })
            .unwrap_or_default();
        if let Some(index) = values.iter().position(|entry| entry == value) {
            values.remove(index);
        } else {
            values.push(value.to_owned());
        }
        dispatch(
            app,
            RequestMsg::Choose(id, FormValue::Choices(values), false),
            tx,
        );
    } else {
        let selection = if kind == Some("boolean") {
            FormValue::Boolean(value == "true")
        } else {
            FormValue::Text(value.to_owned())
        };
        dispatch(app, RequestMsg::Choose(id, selection, true), tx);
    }
}

fn options(field: &Value) -> Vec<(&str, &str, Option<&str>)> {
    if field.get("type").and_then(Value::as_str) == Some("boolean") {
        vec![("true", "Yes", None), ("false", "No", None)]
    } else {
        choices(field)
    }
}

fn numeric(app: &mut Desktop, event: &KeyEvent, tx: &mut Tx, id: RequestId, field: &Value) -> bool {
    let Key::Text(text) = &event.key else {
        return false;
    };
    if event.mods.cmd || event.mods.alt || event.mods.ctrl || text.len() != 1 {
        return false;
    }
    if let Some((value, _, _)) = text
        .parse::<usize>()
        .ok()
        .and_then(|digit| digit.checked_sub(1))
        .and_then(|index| options(field).get(index).copied())
    {
        select(app, id, field, value, tx);
        return true;
    }
    false
}

pub fn on_draft_key(app: &mut Desktop, event: &KeyEvent, tx: &mut Tx) -> bool {
    let Some(request) = question(&app.model) else {
        return false;
    };
    let RequestBody::Question { schema, .. } = &request.body else {
        return false;
    };
    let form = app.model.requests.forms.get(&request.id);
    if form.is_some_and(|form| form.collapsed)
        || app.model.requests.sending.contains(&request.id)
        || !app.model.connected()
    {
        return false;
    }
    let id = request.id;
    let Some((_, field)) = fields(schema)
        .get(form.map_or(0, |form| form.step))
        .copied()
    else {
        return false;
    };
    if app.widgets.draft.text.is_empty() && matches!(event.key, Key::Text(_)) {
        return numeric(app, event, tx, id, &field.clone());
    }
    false
}

fn prose(source: &str, t: &Theme) -> El {
    Markdown::new(source.to_owned())
        .style(t.markdown())
        .selectable(true)
        .build(&t.k)
}

fn action(
    label: &str,
    variant: ButtonVariant,
    disabled: bool,
    t: &Theme,
    handler: impl Fn(&mut Desktop, &mut Tx) + 'static,
) -> El {
    ui::button(label, variant, Size::Compact)
        .disabled(disabled)
        .on_press_tx(handler)
        .build(&t.k)
}

fn editor(
    app: &Desktop,
    id: RequestId,
    field: &Value,
    name: &str,
    mandatory: bool,
    disabled: bool,
    t: &Theme,
) -> El {
    let options = options(field);
    let label = field.get("title").and_then(Value::as_str).unwrap_or(name);
    if options.is_empty() {
        let input = ui::text_field(
            &app.widgets.request_edit,
            label,
            "Enter an answer",
            Size::Small,
        )
        .disabled(disabled)
        .on_edit(|app: &mut Desktop| &mut app.widgets.request_edit)
        .on_submit(move |app: &mut Desktop, tx| {
            let value = app.widgets.request_edit.text.clone();
            dispatch(app, RequestMsg::Next(id, value), tx);
        })
        .build(&t.k);
        let entry = ui::field(label, input);
        return if mandatory {
            entry.required().build(t)
        } else {
            entry.build(t)
        };
    }
    let selected = app
        .model
        .requests
        .forms
        .get(&id)
        .and_then(|form| form.fields.get(name));
    let keyboard = field.clone();
    col()
        .gap(S4)
        .child(text(label).font(t.small).color(t.text))
        .children(
            options
                .into_iter()
                .enumerate()
                .map(|(index, (value, label, description))| {
                    let active = match selected {
                        Some(FormValue::Choices(values)) => {
                            values.iter().any(|entry| entry == value)
                        }
                        Some(FormValue::Text(text)) => text == value,
                        Some(FormValue::Boolean(flag)) => value == flag.to_string(),
                        _ => false,
                    };
                    let field = field.clone();
                    let value = value.to_owned();
                    row()
                        .key(format!("question-choice-{index}"))
                        .center_y()
                        .gap(S8)
                        .child(
                            ui::button(label, ButtonVariant::Ghost, Size::Small)
                                .active(active)
                                .disabled(disabled)
                                .on_press_tx(move |app: &mut Desktop, tx| {
                                    select(app, id, &field, &value, tx);
                                })
                                .build(&t.k),
                        )
                        .children(
                            description
                                .map(|hint| text(hint).font(t.small).color(t.muted).grow(1.0)),
                        )
                        .children((index < 9).then(|| ui::kbd_cap(&(index + 1).to_string(), t)))
                }),
        )
        .on_key(move |app: &mut Desktop, event, tx| {
            !disabled && numeric(app, event, tx, id, &keyboard)
        })
}

pub fn drawer(app: &Desktop, t: &Theme) -> Option<El> {
    let request = question(&app.model)?;
    let RequestBody::Question { message, schema } = &request.body else {
        return None;
    };
    let id = request.id;
    let form = app.model.requests.forms.get(&id);
    let step = form.map_or(0, |form| form.step);
    let collapsed = form.is_some_and(|form| form.collapsed);
    let sending = app.model.requests.sending.contains(&id);
    let disabled = sending || !app.model.connected();
    let fields = fields(schema);
    let last = step + 1 >= fields.len();
    let header = row()
        .key("question-header")
        .gap(S8)
        .px(S12)
        .center_y()
        .child(icon(&ui::INFO, ICON_SMALL).color(t.accent))
        .child(
            text(if sending {
                "Sending answer…"
            } else {
                "Question from agent"
            })
            .font(t.small)
            .color(t.text),
        )
        .children((fields.len() > 1).then(|| {
            text(format!("{}/{}", step + 1, fields.len()))
                .font(t.small)
                .color(t.muted)
        }))
        .child(rune::el().grow(1.0))
        .child(
            ui::icon_button(
                if collapsed {
                    &ui::CHEVRON_OPEN
                } else {
                    &ui::CHEVRON_UP
                },
                if collapsed {
                    "Expand question"
                } else {
                    "Collapse question"
                },
                IconButtonVariant::Ghost,
                Size::Compact,
            )
            .on_press_tx(move |app: &mut Desktop, tx| dispatch(app, RequestMsg::Collapse(id), tx))
            .build(&t.k),
        );
    let mut content = col().gap(S8).w_full().padding(S12).child(prose(message, t));
    if let Some((name, field)) = fields.get(step).copied() {
        content = content
            .children(
                field
                    .get("description")
                    .and_then(Value::as_str)
                    .map(|description| prose(description, t)),
            )
            .child(editor(
                app,
                id,
                field,
                name,
                required(schema, name),
                disabled,
                t,
            ));
    }
    let error = form.and_then(|form| form.error.as_deref());
    content = content.children(error.map(|message| text(message).font(t.small).color(t.danger)));
    let controls = controls(id, step, last, disabled, t);
    Some(
        ui::tucked_drawer(
            format!("question-{id}"),
            10,
            col().child(header).children((!collapsed).then(|| {
                col()
                    .child(ui::rule(t))
                    .child(
                        col()
                            .key(format!("question-body-{step}"))
                            .max_h(280.0)
                            .scroll(ScrollSpec {
                                stick_bottom: false,
                            })
                            .scroll_anchor()
                            .child(content),
                    )
                    .child(controls)
            })),
            t,
        )
        .transition(crate::motion::tucked_deck()),
    )
}

fn controls(id: RequestId, step: usize, last: bool, disabled: bool, t: &Theme) -> El {
    row()
        .key("question-actions")
        .gap(S4)
        .px(S12)
        .pb(S8)
        .child(action(
            "Decline",
            ButtonVariant::Ghost,
            disabled,
            t,
            move |app, tx| {
                dispatch(app, RequestMsg::Answer(id, RequestAnswer::Decline), tx);
            },
        ))
        .child(rune::el().grow(1.0))
        .children((step > 0).then(|| {
            action("Back", ButtonVariant::Ghost, disabled, t, move |app, tx| {
                dispatch(app, RequestMsg::Back(id), tx);
            })
        }))
        .child(action(
            if last { "Submit" } else { "Next" },
            ButtonVariant::Submit,
            disabled,
            t,
            move |app, tx| {
                let text = app.widgets.request_edit.text.clone();
                dispatch(app, RequestMsg::Next(id, text), tx);
            },
        ))
}
