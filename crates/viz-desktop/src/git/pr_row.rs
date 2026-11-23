//! Pull request status, checks, reviews, and creation.

use rune::{El, col, row, text};
use rune_kit::{ButtonVariant, Placement};
use viz_client::protocol::{GitHubPull, GitHubPullState};

use crate::{
    app::Desktop,
    effects::git::Request,
    model::git::{CARD_WIDTH, Popup, default_branch},
    theme::{Size, Theme},
    ui,
    update::{GitMsg, Msg, TimelineMsg},
};

pub fn view(s: &Desktop, pr: &GitHubPull, t: &Theme, overlays: &mut Vec<El>) -> El {
    let (state, tone) = match (pr.state, pr.draft) {
        (GitHubPullState::Open, true) => ("Draft", ui::Tone::Neutral),
        (GitHubPullState::Open, false) => ("Open", ui::Tone::Success),
        (GitHubPullState::Merged, _) => ("Merged", ui::Tone::Delegated),
        (GitHubPullState::Closed, _) => ("Closed", ui::Tone::Danger),
    };
    let url = pr.url.clone();
    let title = ui::list_row("git-pr", &pr.title)
        .leading_icon(&ui::GIT_FORK)
        .trailing(ui::badge(state).tone(tone).build(t))
        .on_press_tx(move |s: &mut Desktop, tx| {
            s.dispatch(Msg::Timeline(TimelineMsg::OpenUrl(url.clone())), tx);
        })
        .build(t);
    let checks = pr
        .checks
        .as_deref()
        .map_or_else(|| "Checks".into(), |value| format!("Checks · {value}"));
    let meta = row()
        .px(8.0)
        .center_y()
        .gap(4.0)
        .child(text(format!("#{}", pr.number)).font(t.small).color(t.muted))
        .child(chip(
            &checks,
            "pr-checks",
            Popup::Checks,
            Request::Checks(pr.head_commit.clone()),
            t,
        ))
        .child(chip(
            "Reviews",
            "pr-reviews",
            Popup::Reviews,
            Request::Reviews(pr.number),
            t,
        ));
    popover(s, t, overlays);
    col().child(title).child(meta)
}

fn popover(s: &Desktop, t: &Theme, overlays: &mut Vec<El>) {
    let (key, entries) = match s.model.git.popup {
        Some(Popup::Checks) => (
            "pr-checks",
            s.model.git.checks.as_ref().map(|checks| {
                checks
                    .iter()
                    .map(|check| {
                        result_row(
                            check.id,
                            &check.name,
                            check.conclusion.as_deref().unwrap_or(&check.status),
                            check.url.as_deref(),
                            t,
                        )
                    })
                    .collect()
            }),
        ),
        Some(Popup::Reviews) => (
            "pr-reviews",
            s.model.git.reviews.as_ref().map(|reviews| {
                reviews
                    .iter()
                    .map(|review| {
                        result_row(
                            review.id,
                            review.author.as_deref().unwrap_or("Reviewer"),
                            &review.state,
                            Some(&review.url),
                            t,
                        )
                    })
                    .collect()
            }),
        ),
        _ => return,
    };
    let mut contents = col().padding(ui::MENU_INSET).w_full();
    match entries {
        Some(entries) if !Vec::<El>::is_empty(&entries) => contents = contents.children(entries),
        Some(_) => {
            contents = contents.child(text("No results").font(t.small).color(t.muted).padding(8.0));
        }
        None => {
            contents = contents.child(
                text(s.model.git.error.as_deref().unwrap_or("Loading…"))
                    .font(t.small)
                    .color(t.muted)
                    .padding(8.0),
            );
        }
    }
    overlays.push(
        ui::anchored_popover(
            "pr-results",
            key,
            Placement::BottomEnd,
            CARD_WIDTH,
            rune_kit::popover::fit_scroll(contents)
                .key("pr-results-scroll")
                .scroll_anchor()
                .max_h(224.0),
            t,
        )
        .modal(true, super::close_popup),
    );
}

fn chip(label: &str, key: &str, popup: Popup, request: Request, t: &Theme) -> El {
    ui::button(label, ButtonVariant::Ghost, Size::Small)
        .on_press_tx(move |s: &mut Desktop, tx| {
            super::toggle_popup(s, popup, tx);
            if s.model.git.popup == Some(popup) {
                super::dispatch(s, GitMsg::Request(request.clone()), tx);
            }
        })
        .build(&t.k)
        .key(key)
}

fn result_row(id: u64, title: &str, detail: &str, url: Option<&str>, t: &Theme) -> El {
    let row = ui::list_row(format!("pr-result-{id}"), title).detail(detail);
    match url.map(str::to_owned) {
        Some(url) => row
            .on_press_tx(move |s: &mut Desktop, tx| {
                s.dispatch(Msg::Timeline(TimelineMsg::OpenUrl(url.clone())), tx);
            })
            .build(t),
        None => row.build(t),
    }
}

pub fn dialog(s: &Desktop, t: &Theme, overlays: &mut Vec<El>) {
    let git = &s.model.git;
    let fields = [
        Some(
            ui::field(
                "Title",
                ui::text_field(
                    &s.widgets.git_message,
                    "Title",
                    "Pull request title",
                    Size::Default,
                )
                .on_edit(|s: &mut Desktop| &mut s.widgets.git_message)
                .build(&t.k),
            )
            .required()
            .build(t),
        ),
        Some(
            ui::field(
                "Description",
                ui::text_field(
                    &s.widgets.git_body,
                    "Description",
                    "Summarize the changes",
                    Size::Default,
                )
                .on_edit(|s: &mut Desktop| &mut s.widgets.git_body)
                .build(&t.k),
            )
            .build(t),
        ),
        super::actions::error(s, t),
    ]
    .into_iter()
    .flatten();
    let submit = |s: &mut Desktop, tx: &mut rune::Tx| {
        let title = s.widgets.git_message.text.trim().to_owned();
        let body = s.widgets.git_body.text.clone();
        let base = s
            .model
            .project()
            .and_then(|p| p.settings.base_branch.clone());
        super::dispatch(
            s,
            GitMsg::Request(Request::CreatePull {
                title,
                body,
                draft: false,
                base,
            }),
            tx,
        );
    };
    let footer = ui::dialog_actions([
        ui::button("Cancel", ButtonVariant::Ghost, Size::Default)
            .on_press(super::close_dialog)
            .build(&t.k),
        ui::button(
            if git.busy { "Creating…" } else { "Create PR" },
            ButtonVariant::Submit,
            Size::Default,
        )
        .disabled(git.busy || s.widgets.git_message.text.trim().is_empty())
        .on_press_tx(submit)
        .build(&t.k),
    ]);
    let description = format!(
        "Open a pull request targeting {}.",
        default_branch(&s.model)
    );
    ui::dialog("git-create-pr", "Create pull request", Some(&description)).build(
        super::close_dialog,
        ui::dialog_body(fields, footer),
        t,
        overlays,
    );
}
