//! Pending permission cards with the offered ACP choices.

use rune::{El, Key, ScrollSpec, Tx, icon, row, text};
use rune_kit::ButtonVariant;
use viz_client::protocol::{PermissionKind, RequestAnswer, RequestBody, RunRequest};

use crate::{
    app::Desktop,
    model::{Item, requests::pending},
    theme::{ICON_SMALL, S4, S8, S12, Size, Theme},
    ui,
    update::{Msg, requests::RequestMsg},
};

pub fn approval_rows<'a>(app: &'a Desktop, t: &'a Theme) -> impl Iterator<Item = El> + 'a {
    pending(&app.model).filter_map(|request| card(app, request, t))
}

fn card(app: &Desktop, request: &RunRequest, t: &Theme) -> Option<El> {
    let RequestBody::Permission {
        call,
        title,
        options,
    } = &request.body
    else {
        return None;
    };
    let id = request.id;
    let sending = app.model.requests.sending.contains(&id);
    let disabled = sending || !app.model.connected();
    let detail = detail(app, call, title);
    let shortcut_options = options
        .iter()
        .take(9)
        .map(|option| option.id.clone())
        .collect::<Vec<_>>();
    Some(
        ui::Card::Surface
            .build(t)
            .key(format!("approval-{id}"))
            .gap(S8)
            .padding(S12)
            .w_full()
            .transition(crate::motion::tucked_deck())
            .child(
                row()
                    .gap(S4)
                    .center_y()
                    .child(icon(&ui::SHIELD, ICON_SMALL).color(t.warning))
                    .child(
                        text(if sending {
                            "Sending answer…"
                        } else {
                            "Waiting for approval"
                        })
                        .font(t.small)
                        .color(t.warning),
                    ),
            )
            .child(text(title).font(t.body).color(t.text))
            .children(detail.map(|detail| {
                ui::Card::Recessed
                    .frame(t)
                    .key(format!("approval-detail-{id}"))
                    .max_h(160.0)
                    .scroll(ScrollSpec {
                        stick_bottom: false,
                    })
                    .scroll_anchor()
                    .child(rune::col().padding(S8).child(ui::code(&detail, t)))
            }))
            .child(row().gap(S4).wrap().children(options.iter().map(|option| {
                let option_id = option.id.clone();
                ui::button(
                    &option.label,
                    if matches!(option.kind, PermissionKind::AllowOnce) {
                        ButtonVariant::Submit
                    } else {
                        ButtonVariant::Ghost
                    },
                    Size::Compact,
                )
                .disabled(disabled)
                .on_press_tx(move |app: &mut Desktop, tx| choose(app, id, option_id.clone(), tx))
                .build(&t.k)
            })))
            .on_key(move |app: &mut Desktop, event, tx| {
                let Key::Text(value) = &event.key else {
                    return false;
                };
                let Some(index) = value
                    .parse::<usize>()
                    .ok()
                    .and_then(|digit| digit.checked_sub(1))
                else {
                    return false;
                };
                if disabled || event.mods.cmd || event.mods.ctrl || event.mods.alt {
                    return false;
                }
                if let Some(option) = shortcut_options.get(index) {
                    choose(app, id, option.clone(), tx);
                    return true;
                }
                false
            }),
    )
}

fn choose(app: &mut Desktop, id: viz_client::protocol::RequestId, option: String, tx: &mut Tx) {
    app.dispatch(
        Msg::Requests(RequestMsg::Answer(id, RequestAnswer::Permission { option })),
        tx,
    );
}

fn detail(app: &Desktop, call: &str, title: &str) -> Option<String> {
    let transcript = app.model.transcript();
    let tool = transcript
        .acp_partial
        .iter()
        .rev()
        .chain(transcript.items.iter().rev())
        .find_map(|item| match item {
            Item::Tool(tool) if tool.call_id == call => Some(tool),
            _ => None,
        });
    tool.map(|tool| {
        if let Some(command) = &tool.input {
            return command.clone();
        }
        if !tool.diffs.is_empty() {
            return tool
                .diffs
                .iter()
                .map(|diff| format!("{}\n{}", diff.path, diff.new_text))
                .collect::<Vec<_>>()
                .join("\n\n");
        }
        tool.pending_input
            .clone()
            .unwrap_or_else(|| tool.summary.clone())
    })
    .filter(|text| !text.is_empty() && text != title)
}
