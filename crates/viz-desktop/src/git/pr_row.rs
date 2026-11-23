//! Pull request status, checks, reviews, and creation.

use rune::{El, col, row, text};
use rune_kit::{ButtonVariant, Menu, MenuItem, Placement};
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
    let (state, variant) = match (pr.state, pr.draft) {
        (GitHubPullState::Open, true) => ("Draft", rune_kit::BadgeVariant::Subtle),
        (GitHubPullState::Open, false) => ("Open", rune_kit::BadgeVariant::Accent),
        (GitHubPullState::Merged | GitHubPullState::Closed, _) => (
            if pr.state == GitHubPullState::Merged {
                "Merged"
            } else {
                "Closed"
            },
            rune_kit::BadgeVariant::Neutral,
        ),
    };
    let url = pr.url.clone();
    let title = ui::list_row("git-pr", &pr.title)
        .leading_icon(&ui::GIT_FORK)
        .trailing(rune_kit::Badge::new(state).variant(variant).build(&t.k))
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
            s,
            &checks,
            "pr-checks",
            Popup::Checks,
            Request::Checks(pr.head_commit.clone()),
            t,
            overlays,
        ))
        .child(chip(
            s,
            "Reviews",
            "pr-reviews",
            Popup::Reviews,
            Request::Reviews(pr.number),
            t,
            overlays,
        ));
    col().child(title).child(meta)
}

/// The open popup's results as menu items, or a disabled status item while
/// loading, failed, or empty.
fn results(s: &Desktop, popup: Popup) -> Vec<MenuItem> {
    let git = &s.model.git;
    let items: Option<Vec<MenuItem>> = match popup {
        Popup::Checks => git.checks.as_ref().map(|checks| {
            checks
                .iter()
                .map(|check| {
                    result(
                        &check.name,
                        check.conclusion.as_deref().unwrap_or(&check.status),
                        check.url.as_deref(),
                    )
                })
                .collect()
        }),
        Popup::Reviews => git.reviews.as_ref().map(|reviews| {
            reviews
                .iter()
                .map(|review| {
                    result(
                        review.author.as_deref().unwrap_or("Reviewer"),
                        &review.state,
                        Some(&review.url),
                    )
                })
                .collect()
        }),
        _ => Some(Vec::new()),
    };
    match items {
        Some(items) if !items.is_empty() => items,
        Some(_) => vec![MenuItem::new("No results").disabled(true)],
        None => vec![MenuItem::new(git.error.as_deref().unwrap_or("Loading…")).disabled(true)],
    }
}

fn chip(
    s: &Desktop,
    label: &str,
    key: &str,
    popup: Popup,
    request: Request,
    t: &Theme,
    overlays: &mut Vec<El>,
) -> El {
    let trigger = rune_kit::Button::new(label)
        .variant(ButtonVariant::Ghost)
        .control_size(Size::Small.kit())
        .on_press_tx(move |s: &mut Desktop, tx| {
            super::toggle_popup(s, popup, tx);
            if s.model.git.popup == Some(popup) {
                super::dispatch(s, GitMsg::Request(request.clone()), tx);
            }
        })
        .build(&t.k);
    let open = s.model.git.popup == Some(popup);
    let items = if open { results(s, popup) } else { Vec::new() };
    ui::menu_items(Menu::new(key), items)
        .trigger(trigger)
        .open(open)
        .placement(Placement::BottomEnd)
        .min_width(CARD_WIDTH)
        .on_close(super::close_popup)
        .build(&t.overlay, overlays)
}

fn result(title: &str, status: &str, url: Option<&str>) -> MenuItem {
    let item = MenuItem::new(title).badge(status);
    match url.map(str::to_owned) {
        Some(url) => item.on_select_tx(move |s: &mut Desktop, tx| {
            s.dispatch(Msg::Timeline(TimelineMsg::OpenUrl(url.clone())), tx);
        }),
        None => item,
    }
}

pub fn dialog(s: &Desktop, t: &Theme, overlays: &mut Vec<El>) {
    let git = &s.model.git;
    let fields = [
        Some(
            rune_kit::Field::new()
                .label("Title")
                .control(
                    rune_kit::TextInput::new(&s.widgets.git_message)
                        .label("Title")
                        .placeholder("Pull request title")
                        .width(None)
                        .on_edit(|s: &mut Desktop| &mut s.widgets.git_message)
                        .build(&t.k),
                )
                .build(&t.k),
        ),
        Some(
            rune_kit::Field::new()
                .label("Description")
                .control(
                    rune_kit::TextInput::new(&s.widgets.git_body)
                        .label("Description")
                        .placeholder("Summarize the changes")
                        .width(None)
                        .on_edit(|s: &mut Desktop| &mut s.widgets.git_body)
                        .build(&t.k),
                )
                .build(&t.k),
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
        rune_kit::Button::new("Cancel")
            .variant(ButtonVariant::Ghost)
            .control_size(Size::Default.kit())
            .on_press(super::close_dialog)
            .build(&t.k),
        rune_kit::Button::new(if git.busy { "Creating…" } else { "Create PR" })
            .variant(ButtonVariant::Submit)
            .control_size(Size::Default.kit())
            .disabled(git.busy || s.widgets.git_message.text.trim().is_empty())
            .on_press_tx(submit)
            .build(&t.k),
    ]);
    let description = format!(
        "Open a pull request targeting {}.",
        default_branch(&s.model)
    );
    rune_kit::Dialog::new("git-create-pr", "Create pull request")
        .open(true)
        .fit()
        .on_close(super::close_dialog)
        .description(&description)
        .child(ui::dialog_body(fields, footer))
        .build(&t.overlay, overlays);
}
