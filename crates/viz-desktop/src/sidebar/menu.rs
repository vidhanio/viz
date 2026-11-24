//! Four-group thread context menu (used from both sidebar cards and the top
//! chat title) with hover-opening nested submenus (Snooze, Auto-settle
//! behavior, Move to project, Copy).

use std::rc::Rc;

use rune::{Align, El, Icon, Side, Tx, col, el, icon, row, text};
use rune_kit::Kit;
use viz_client::protocol::{
    AutoSettle, Position, Project, Shelf, ThreadId, ThreadSummary, TriageAction,
};

use crate::{
    app::Desktop,
    model::{ContextSubmenu, ProjectFilter, SidebarEdit, SnoozePreset},
    theme::Theme,
    ui,
    update::{Msg, SidebarMsg},
};

/// Minimum width of the primary thread context menu (points).
const CONTEXT_MENU_WIDTH: f32 = 208.0;
/// Minimum width of a nested submenu panel (points).
const SUBMENU_WIDTH: f32 = 160.0;

type SelectAction = Rc<dyn Fn(&mut Desktop, &mut Tx)>;

enum CtxEntry {
    Item(Box<CtxItem>),
    Separator,
}

struct CtxItem {
    label: String,
    icon: Option<&'static Icon>,
    danger: bool,
    checked: Option<bool>,
    disabled: bool,
    submenu: Option<(ContextSubmenu, Vec<CtxEntry>, bool)>,
    on_select: Option<SelectAction>,
}

impl CtxItem {
    fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            icon: None,
            danger: false,
            checked: None,
            disabled: false,
            submenu: None,
            on_select: None,
        }
    }

    const fn icon(mut self, glyph: &'static Icon) -> Self {
        self.icon = Some(glyph);
        self
    }

    const fn danger(mut self, on: bool) -> Self {
        self.danger = on;
        self
    }

    const fn checked(mut self, on: bool) -> Self {
        self.checked = Some(on);
        self
    }

    const fn disabled(mut self, on: bool) -> Self {
        self.disabled = on;
        self
    }

    fn submenu(mut self, kind: ContextSubmenu, entries: Vec<CtxEntry>, open: bool) -> Self {
        self.submenu = Some((kind, entries, open));
        self
    }

    fn on_select_tx(mut self, f: impl Fn(&mut Desktop, &mut Tx) + 'static) -> Self {
        self.on_select = Some(Rc::new(f));
        self
    }

    fn entry(self) -> CtxEntry {
        CtxEntry::Item(Box::new(self))
    }
}

/// Mounts the full 4-group thread context menu into `layer` when open for
/// `key`.
#[expect(
    clippy::too_many_lines,
    reason = "single exhaustive 4-group thread context menu definition"
)]
pub(super) fn thread_context_menu(
    s: &Desktop,
    thread: &ThreadSummary,
    key: &str,
    shelf: Shelf,
    now: i64,
    t: &Theme,
    layer: &mut Vec<El>,
) {
    let m = &s.model;
    let Some(context) = m
        .sidebar
        .menu
        .as_ref()
        .filter(|context| context.key == key && m.connected())
    else {
        return;
    };
    let id = thread.id;
    let project = thread
        .project
        .as_deref()
        .and_then(|path| m.cache().project(path));
    let branch = thread
        .worktree
        .as_ref()
        .and_then(|wt| wt.branch.as_deref())
        .or_else(|| project.and_then(|p| p.branch.as_deref()));

    let new_label = match (branch, project) {
        (Some(branch), _) => format!("New thread on {branch}"),
        (None, Some(project)) => format!("New thread in {}", project.name),
        (None, None) => "New thread".to_owned(),
    };
    let new_project = thread.project.clone();
    let new_branch = branch.map(str::to_owned);
    let pinned = thread.lifecycle.pin.is_some();
    let settled = shelf == Shelf::Settled;
    let working = shelf == Shelf::Working;
    let snoozed = shelf == Shelf::Snoozed;
    let unread = thread.lifecycle.unread;
    let archived = thread.lifecycle.archived;

    let mut entries = vec![
        CtxItem::new(new_label)
            .icon(&ui::COMPOSE)
            .disabled(m.selection.new_thread.is_some())
            .on_select_tx(move |s: &mut Desktop, tx| {
                s.dispatch(
                    Msg::Sidebar(SidebarMsg::NewThreadIn {
                        project: new_project.clone(),
                        branch: new_branch.clone(),
                    }),
                    tx,
                );
            })
            .entry(),
        CtxItem::new(if pinned { "Unpin thread" } else { "Pin thread" })
            .icon(if pinned { &ui::PIN_OFF } else { &ui::PIN })
            .on_select_tx(move |s: &mut Desktop, tx| {
                let (action, undo_title) = if pinned {
                    (TriageAction::Unpin, Some("Thread unpinned"))
                } else {
                    (
                        TriageAction::Pin {
                            position: Position::Top,
                        },
                        None,
                    )
                };
                s.dispatch(
                    Msg::Sidebar(SidebarMsg::Triage {
                        thread: id,
                        action,
                        undo_title,
                        now,
                    }),
                    tx,
                );
            })
            .entry(),
        CtxItem::new(if settled {
            "Reopen thread"
        } else {
            "Settle thread"
        })
        .icon(if settled {
            &ui::ROTATE_CCW
        } else {
            &ui::CIRCLE_CHECK
        })
        .disabled(working && !settled)
        .on_select_tx(move |s: &mut Desktop, tx| {
            let (action, undo_title) = if settled {
                (TriageAction::Reopen, None)
            } else {
                (TriageAction::Settle, Some("Thread settled"))
            };
            s.dispatch(
                Msg::Sidebar(SidebarMsg::Triage {
                    thread: id,
                    action,
                    undo_title,
                    now,
                }),
                tx,
            );
        })
        .entry(),
        CtxItem::new("Snooze")
            .icon(&ui::CLOCK)
            .submenu(
                ContextSubmenu::Snooze,
                snooze_entries(id, snoozed, now),
                context.submenu == Some(ContextSubmenu::Snooze),
            )
            .entry(),
        CtxEntry::Separator,
        CtxItem::new("Rename thread")
            .icon(&ui::PENCIL)
            .on_select_tx(move |s: &mut Desktop, tx| {
                if !s.widgets.sidebar_pane.is_open() {
                    s.dispatch(Msg::Sidebar(SidebarMsg::TogglePane), tx);
                }
                s.dispatch(Msg::Sidebar(SidebarMsg::StartInlineRename(id)), tx);
            })
            .entry(),
        CtxItem::new("Regenerate title")
            .icon(&ui::REFRESH_CW)
            .on_select_tx(move |s: &mut Desktop, tx| {
                s.dispatch(Msg::Sidebar(SidebarMsg::RetitleThread(id)), tx);
            })
            .entry(),
        CtxItem::new(if unread { "Mark read" } else { "Mark unread" })
            .icon(if unread { &ui::MAIL_OPEN } else { &ui::MAIL })
            .on_select_tx(move |s: &mut Desktop, tx| {
                let action = if unread {
                    TriageAction::MarkRead
                } else {
                    TriageAction::MarkUnread
                };
                s.dispatch(
                    Msg::Sidebar(SidebarMsg::Triage {
                        thread: id,
                        action,
                        undo_title: None,
                        now,
                    }),
                    tx,
                );
            })
            .entry(),
    ];

    let (filter_label, target_filter) = match project {
        Some(project)
            if m.sidebar.project_filter == ProjectFilter::Project(project.path.clone()) =>
        {
            ("Clear project filter".to_owned(), ProjectFilter::All)
        }
        Some(project) => (
            format!("Filter by {}", project.name),
            ProjectFilter::Project(project.path.clone()),
        ),
        None if m.sidebar.project_filter == ProjectFilter::NoProject => {
            ("Clear project filter".to_owned(), ProjectFilter::All)
        }
        None => ("Filter by No project".to_owned(), ProjectFilter::NoProject),
    };
    entries.push(
        CtxItem::new(filter_label)
            .icon(&ui::LIST_FILTER)
            .on_select_tx(move |s: &mut Desktop, tx| {
                tx.instant();
                s.dispatch(
                    Msg::Sidebar(SidebarMsg::SetProjectFilter(target_filter.clone())),
                    tx,
                );
            })
            .entry(),
    );
    entries.push(
        CtxItem::new("Auto-settle behavior")
            .icon(&ui::TIMER)
            .submenu(
                ContextSubmenu::AutoSettle,
                auto_settle_entries(id, thread.lifecycle.auto_settle, now),
                context.submenu == Some(ContextSubmenu::AutoSettle),
            )
            .entry(),
    );
    entries.push(
        CtxItem::new("Fork thread")
            .icon(&ui::GIT_FORK)
            .on_select_tx(move |s: &mut Desktop, tx| {
                s.dispatch(Msg::Sidebar(SidebarMsg::ForkThread(id)), tx);
            })
            .entry(),
    );
    entries.push(CtxEntry::Separator);

    if m.cache().has_projects() || thread.project.is_some() {
        let destinations = std::iter::once(destination(thread, None))
            .chain(
                m.cache()
                    .projects()
                    .map(|project| destination(thread, Some(project))),
            )
            .map(CtxItem::entry)
            .collect();
        entries.push(
            CtxItem::new("Move to project")
                .icon(&ui::FOLDER)
                .disabled(thread.worktree.is_some())
                .submenu(
                    ContextSubmenu::MoveToProject,
                    destinations,
                    context.submenu == Some(ContextSubmenu::MoveToProject),
                )
                .entry(),
        );
    }

    entries.push(
        CtxItem::new("Copy")
            .icon(&ui::COPY)
            .submenu(
                ContextSubmenu::Copy,
                copy_entries(thread, branch),
                context.submenu == Some(ContextSubmenu::Copy),
            )
            .entry(),
    );
    entries.push(CtxEntry::Separator);
    entries.push(
        CtxItem::new(if archived {
            "Unarchive thread"
        } else {
            "Archive thread"
        })
        .icon(&ui::ARCHIVE)
        .on_select_tx(move |s: &mut Desktop, tx| {
            let (action, undo_title) = if archived {
                (TriageAction::Unarchive, None)
            } else {
                (TriageAction::Archive, Some("Thread archived"))
            };
            s.dispatch(
                Msg::Sidebar(SidebarMsg::Triage {
                    thread: id,
                    action,
                    undo_title,
                    now,
                }),
                tx,
            );
        })
        .entry(),
    );
    entries.push(
        CtxItem::new("Delete")
            .icon(&ui::TRASH)
            .danger(true)
            .on_select_tx(move |s: &mut Desktop, tx| {
                s.dispatch(Msg::Sidebar(SidebarMsg::OpenDeleteThread(id)), tx);
            })
            .entry(),
    );

    if thread.worktree.is_some() {
        entries.push(
            CtxItem::new("Delete with worktree")
                .icon(&ui::TRASH)
                .danger(true)
                .on_select_tx(move |s: &mut Desktop, tx| {
                    s.dispatch(
                        Msg::Sidebar(SidebarMsg::OpenEdit {
                            edit: SidebarEdit::DeleteWithWorktree(id),
                            initial_value: String::new(),
                        }),
                        tx,
                    );
                })
                .entry(),
        );
    }

    mount_context_menu(
        &format!("{key}-context"),
        context.point,
        entries,
        &t.overlay,
        layer,
    );
}

fn mount_context_menu(
    menu_key: &str,
    point: Option<(f32, f32)>,
    entries: Vec<CtxEntry>,
    k: &Kit,
    layer: &mut Vec<El>,
) {
    let anchor_key = if let Some((x, y)) = point {
        let point_key = format!("{menu_key}-point");
        layer.push(el().key(point_key.clone()).absolute(x, y).size(0.0, 0.0));
        point_key
    } else {
        format!("{menu_key}-trigger")
    };

    layer.push(
        el().key(format!("{menu_key}-scrim"))
            .role("none")
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .on_press(|s: &mut Desktop| s.dispatch_local(Msg::Sidebar(SidebarMsg::CloseRowMenu))),
    );

    let mut subs = Vec::new();
    let surface = build_menu_surface(menu_key, entries, CONTEXT_MENU_WIDTH, true, k, &mut subs)
        .modal(true, |s: &mut Desktop| {
            s.dispatch_local(Msg::Sidebar(SidebarMsg::CloseRowMenu));
        });

    layer.push(
        surface
            .anchored(&anchor_key, Side::Bottom, Align::Start, 0.0)
            .instant_changes()
            .motion_intent(rune::MotionIntent::CUT),
    );
    layer.extend(subs);
}

fn build_menu_surface(
    menu_key: &str,
    entries: Vec<CtxEntry>,
    min_width: f32,
    is_root: bool,
    k: &Kit,
    submenus: &mut Vec<El>,
) -> El {
    let c = &k.c;
    let mut list = col().key("rows").items_stretch().padding(2.0);
    for (idx, entry) in entries.into_iter().enumerate() {
        let row_el = match entry {
            CtxEntry::Separator => el()
                .key(format!("sep{idx}"))
                .h(1.0)
                .mt(2.0)
                .mb(2.0)
                .ml(-2.0)
                .mr(-2.0)
                .fill(c.border_muted)
                .a11y_hidden(),
            CtxEntry::Item(item) => build_menu_item(
                *item,
                &format!("{menu_key}-item{idx}"),
                is_root,
                k,
                submenus,
            ),
        };
        list = list.child(row_el);
    }

    let frame = col()
        .key(format!("{menu_key}-menu"))
        .role("menu")
        .focus_group()
        .min_w(min_width)
        .max_w(280.0)
        .items_stretch()
        .radius(6.0)
        .fill(c.layer_01)
        .child(list);

    k.elevate(frame, &k.elevation.floating)
}

fn build_menu_item(item: CtxItem, key: &str, is_root: bool, k: &Kit, submenus: &mut Vec<El>) -> El {
    let c = &k.c;
    let disabled = item.disabled;
    let sub_open = item.submenu.as_ref().is_some_and(|(_, _, open)| *open);
    let danger = item.danger && !disabled;
    let ink = if danger { c.danger.fg } else { c.text_base };

    let mut content = row()
        .key("content")
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .gap(8.0)
        .center_y()
        .a11y_hidden();
    if let Some(glyph) = item.icon {
        content = content.child(icon(glyph, 14.0).key("icon").color(if danger {
            c.danger.fg
        } else {
            c.icon_muted
        }));
    }
    content = content.child(
        text(item.label.clone())
            .key("label")
            .font(k.ty.ui_compact)
            .color(ink)
            .min_w(0.0)
            .shrink()
            .truncate(),
    );

    let role = if item.checked.is_some() {
        "menuitemcheckbox"
    } else {
        "menuitem"
    };
    let mut e = row()
        .key(key.to_owned())
        .role(role)
        .label(item.label)
        .h(28.0)
        .pl(12.0)
        .pr(6.0)
        .gap(12.0)
        .center_y()
        .radius(4.0)
        .child(content);

    if let Some(on) = item.checked {
        e = e.checked(on);
        let mark = icon(k.glyph.check_small, 14.0)
            .color(c.icon_base)
            .opacity(if on { 1.0 } else { 0.0 });
        e = e.child(
            el().key("indicator")
                .size(16.0, 16.0)
                .center()
                .child(mark)
                .a11y_hidden(),
        );
    }

    if sub_open {
        e = e.fill(c.hover);
    } else if !disabled {
        e = e.focus_highlight(c.hover);
    }

    if let Some((kind, entries, open)) = item.submenu {
        e = attach_submenu(e, key, kind, entries, open, disabled, k, submenus);
    } else if !disabled {
        if is_root {
            e = e.on_hover_change(|s: &mut Desktop, inside, _| {
                if inside {
                    s.dispatch_local(Msg::Sidebar(SidebarMsg::SetSubmenu(None)));
                }
            });
        }
        if let Some(action) = item.on_select {
            e = e.on_press_tx(move |s: &mut Desktop, tx| action(s, tx));
        }
    }

    let e = if disabled { e.opacity(0.45) } else { e };
    e.focusable(!disabled)
        .focus_selected(item.checked == Some(true))
        .disabled(disabled)
}

#[expect(
    clippy::too_many_arguments,
    reason = "attaches submenu chevron, hover handlers, and anchored popup surface"
)]
fn attach_submenu(
    mut e: El,
    key: &str,
    kind: ContextSubmenu,
    entries: Vec<CtxEntry>,
    open: bool,
    disabled: bool,
    k: &Kit,
    submenus: &mut Vec<El>,
) -> El {
    e = e.expanded(open).child(
        icon(k.glyph.caret_right, 16.0)
            .key("chevron")
            .a11y_hidden()
            .mirror_in_rtl()
            .color(k.c.text_faint),
    );
    if !disabled {
        e = e
            .on_hover_change(move |s: &mut Desktop, inside, _| {
                if inside {
                    s.dispatch_local(Msg::Sidebar(SidebarMsg::SetSubmenu(Some(kind))));
                }
            })
            .on_press(move |s: &mut Desktop| {
                s.dispatch_local(Msg::Sidebar(SidebarMsg::ToggleSubmenu(kind)));
            });
    }
    if open {
        let mut deeper = Vec::new();
        let sub_surface = build_menu_surface(
            &format!("{key}-sub"),
            entries,
            SUBMENU_WIDTH,
            false,
            k,
            &mut deeper,
        );
        submenus.push(
            sub_surface
                .anchored(key, Side::Right, Align::Start, 0.0)
                .instant_changes()
                .motion_intent(rune::MotionIntent::CUT),
        );
        submenus.extend(deeper);
    }
    e
}

fn snooze_entries(thread: ThreadId, snoozed: bool, now: i64) -> Vec<CtxEntry> {
    let mut entries: Vec<CtxEntry> = SnoozePreset::ALL
        .into_iter()
        .map(|preset| {
            CtxItem::new(preset.label())
                .on_select_tx(move |s: &mut Desktop, tx| {
                    s.dispatch(
                        Msg::Sidebar(SidebarMsg::SnoozePreset {
                            thread,
                            preset,
                            now,
                        }),
                        tx,
                    );
                })
                .entry()
        })
        .collect();
    if snoozed {
        entries.push(CtxEntry::Separator);
        entries.push(
            CtxItem::new("Unsnooze")
                .on_select_tx(move |s: &mut Desktop, tx| {
                    s.dispatch(
                        Msg::Sidebar(SidebarMsg::Triage {
                            thread,
                            action: TriageAction::Wake,
                            undo_title: None,
                            now,
                        }),
                        tx,
                    );
                })
                .entry(),
        );
    }
    entries
}

fn auto_settle_entries(thread: ThreadId, current: AutoSettle, now: i64) -> Vec<CtxEntry> {
    [
        ("Default", AutoSettle::Follow),
        ("Never auto-settle", AutoSettle::Never),
    ]
    .into_iter()
    .map(|(label, mode)| {
        CtxItem::new(label)
            .checked(current == mode)
            .on_select_tx(move |s: &mut Desktop, tx| {
                s.dispatch(
                    Msg::Sidebar(SidebarMsg::Triage {
                        thread,
                        action: TriageAction::AutoSettle { mode },
                        undo_title: None,
                        now,
                    }),
                    tx,
                );
            })
            .entry()
    })
    .collect()
}

fn copy_entries(thread: &ThreadSummary, branch: Option<&str>) -> Vec<CtxEntry> {
    let thread_id = thread.id.0.to_string();
    let mut entries = vec![
        CtxItem::new("Copy thread ID")
            .on_select_tx(move |s: &mut Desktop, tx| {
                s.dispatch(
                    Msg::Sidebar(SidebarMsg::CopyText {
                        label: "Thread ID",
                        value: thread_id.clone(),
                    }),
                    tx,
                );
            })
            .entry(),
    ];
    if let Some(branch) = branch {
        let value = branch.to_owned();
        entries.push(
            CtxItem::new("Copy branch name")
                .on_select_tx(move |s: &mut Desktop, tx| {
                    s.dispatch(
                        Msg::Sidebar(SidebarMsg::CopyText {
                            label: "Branch name",
                            value: value.clone(),
                        }),
                        tx,
                    );
                })
                .entry(),
        );
    }
    if let Some(worktree) = &thread.worktree {
        let value = worktree.path.clone();
        entries.push(
            CtxItem::new("Copy worktree path")
                .on_select_tx(move |s: &mut Desktop, tx| {
                    s.dispatch(
                        Msg::Sidebar(SidebarMsg::CopyText {
                            label: "Worktree path",
                            value: value.clone(),
                        }),
                        tx,
                    );
                })
                .entry(),
        );
    }
    entries
}

fn destination(thread: &ThreadSummary, project: Option<&Project>) -> CtxItem {
    let id = thread.id;
    let current = thread.project.as_deref() == project.map(|project| project.path.as_str());
    let project_id = project.map(|project| project.path.clone());
    CtxItem::new(project.map_or("No project", |project| project.name.as_str()))
        .checked(current)
        .disabled(current)
        .on_select_tx(move |s: &mut Desktop, tx| {
            s.dispatch(
                Msg::Sidebar(SidebarMsg::MoveThreadToProject {
                    thread: id,
                    project: project_id.clone(),
                }),
                tx,
            );
        })
}
