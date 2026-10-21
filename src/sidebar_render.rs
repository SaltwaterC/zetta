//! Drawing the project sidebar and its title bar toggle.
//!
//! The sidebar is a cached view of its own, composed beside the tab body
//! rather than above it: terminal output dirties the body every frame, and a
//! sibling of that is reused through all of it. Like the title bar chrome it
//! renders purely from `Zetta` state, so `ZettaSubview`'s notify observer is
//! its whole invalidation contract.
//!
//! Every row's actions run through the weak `Zetta` handle rather than as
//! dispatched actions, because they name a group or a worktree by path and
//! none of them is a keymap target.

use super::*;
use crate::sidebar::{SidebarEntry, SidebarSources, SidebarWorktree, sidebar_entries};
use crate::tab_groups::TabGroup;
use crate::worktree_ui::WorktreeFinish;
use gpui::{ClickEvent, Font};
use std::rc::Rc;
use terminal_view::standalone_terminal_font;
use ui::ListItem;

/// What every row is drawn with.
#[derive(Clone, Copy)]
struct SidebarRowContext<'a> {
    colors: &'a ThemeColors,
    handle: &'a WeakEntity<Zetta>,
    /// The terminal's font and size, so a row reads at the size of the
    /// terminal beside it rather than the smaller UI font.
    font: &'a Font,
    font_size: Pixels,
}

/// A row action: runs `f` on `Zetta` when the row's menu entry or button
/// fires.
fn on_zetta(
    handle: &WeakEntity<Zetta>,
    f: impl Fn(&mut Zetta, &mut Window, &mut Context<Zetta>) + 'static,
) -> impl Fn(&mut Window, &mut App) + 'static {
    let handle = handle.clone();
    move |window, cx| {
        handle.update(cx, |this, cx| f(this, window, cx)).ok();
    }
}

fn on_zetta_click(
    handle: &WeakEntity<Zetta>,
    f: impl Fn(&mut Zetta, &mut Window, &mut Context<Zetta>) + 'static,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    let action = on_zetta(handle, f);
    move |_, window, cx| action(window, cx)
}

/// The title bar button that opens and closes the sidebar, left of the
/// application menu.
pub(crate) fn sidebar_toggle_control(open: bool, show_label: bool) -> AnyElement {
    Button::new(
        "toggle-project-sidebar",
        if show_label { "Projects" } else { "" },
    )
    .start_icon(
        Icon::new(if open {
            IconName::ThreadsSidebarLeftOpen
        } else {
            IconName::ThreadsSidebarLeftClosed
        })
        .size(IconSize::Small),
    )
    .style(ButtonStyle::Subtle)
    .size(ButtonSize::Large)
    .toggle_state(open)
    .aria_label("Toggle project sidebar")
    .tooltip(Tooltip::for_action_title(
        if open {
            "Hide the project sidebar"
        } else {
            "Show the project sidebar"
        },
        &ToggleSidebar,
    ))
    .on_click(|_, window, cx| window.dispatch_action(Box::new(ToggleSidebar), cx))
    .into_any_element()
}

/// `ZettaSubview`'s adapter for the sidebar. The root fills the box the
/// window column reserved for it.
pub(crate) fn render_sidebar_boundary(
    zetta: &mut Zetta,
    window: &mut Window,
    cx: &mut Context<Zetta>,
) -> AnyElement {
    let frame = WindowFrameGeometry::new(window, cx);
    zetta.render_sidebar(frame.rounded_bottom_left, frame.corner_radius, cx)
}

impl Zetta {
    fn render_sidebar(
        &self,
        rounded_bottom_left: bool,
        corner_radius: Pixels,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.window_theme(cx);
        let colors = theme.colors();
        let handle = cx.entity().downgrade();
        let (font, font_size) = standalone_terminal_font(cx);
        let context = SidebarRowContext {
            colors,
            handle: &handle,
            font: &font,
            font_size,
        };
        let counts = self.tab_group_counts();
        let active = self.active_tab_group();
        let roots = self.projects.registry.roots();
        let entries = sidebar_entries(&SidebarSources {
            custom: self.tab_groups.custom_groups(),
            roots,
            listings: &self.sidebar.worktrees,
            collapsed: &self.sidebar.collapsed,
            counts: &counts,
            active: &active,
        });

        let mut rows = Vec::with_capacity(entries.len() + 3);
        let mut projects_header = false;
        let mut unlisted_header = false;
        for (position, entry) in entries.iter().enumerate() {
            // Everything after Main and the named groups is under Projects.
            if !projects_header
                && !matches!(
                    entry,
                    SidebarEntry::Main { .. } | SidebarEntry::Custom { .. }
                )
            {
                projects_header = true;
                push_projects_header(&mut rows, roots.is_empty(), context);
            }
            match entry {
                SidebarEntry::Main { tab_count, active } => {
                    rows.push(main_row(*tab_count, *active, context));
                }
                SidebarEntry::Custom {
                    name,
                    tab_count,
                    active,
                } => rows.push(custom_row(position, name, *tab_count, *active, context)),
                SidebarEntry::Project { .. } => rows.push(project_row(position, entry, context)),
                SidebarEntry::Worktree {
                    project,
                    worktree,
                    tab_count,
                    active,
                } => rows.push(worktree_row(
                    position,
                    WorktreeRow {
                        project,
                        worktree,
                        tab_count: *tab_count,
                        active: *active,
                    },
                    context,
                )),
                SidebarEntry::Unlisted {
                    group,
                    tab_count,
                    active,
                } => {
                    if !unlisted_header {
                        unlisted_header = true;
                        rows.push(section_header("Other", None, context));
                    }
                    rows.push(unlisted_row(position, group, *tab_count, *active, context));
                }
            }
        }
        if !projects_header {
            push_projects_header(&mut rows, roots.is_empty(), context);
        }

        v_flex()
            .id("project-sidebar")
            .size_full()
            .bg(colors.panel_background)
            .border_r_1()
            .border_color(colors.border)
            .when(rounded_bottom_left, |sidebar| {
                sidebar.rounded_bl(corner_radius)
            })
            .child(
                v_flex()
                    .id("project-sidebar-rows")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.sidebar.scroll)
                    .p_1()
                    .gap_px()
                    .children(rows),
            )
            .into_any_element()
    }
}

fn tab_count_label(tab_count: usize) -> Option<AnyElement> {
    (tab_count > 0).then(|| {
        Label::new(tab_count.to_string())
            .size(LabelSize::XSmall)
            .color(Color::Muted)
            .into_any_element()
    })
}

fn row_label(label: impl Into<SharedString>, context: SidebarRowContext<'_>) -> impl IntoElement {
    div()
        .min_w_0()
        .overflow_hidden()
        .text_ellipsis()
        .whitespace_nowrap()
        .font(context.font.clone())
        .text_size(context.font_size)
        .text_color(context.colors.text)
        .child(label.into())
}

fn row_button(
    id: impl Into<ElementId>,
    icon: IconName,
    tooltip: &'static str,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> IconButton {
    IconButton::new(id, icon)
        .shape(IconButtonShape::Square)
        .icon_size(IconSize::Small)
        .aria_label(tooltip)
        .tooltip(Tooltip::text(tooltip))
        .on_click(on_click)
}

/// Muted text in the terminal's font and size, like the rows around it.
fn sidebar_text(text: &'static str, context: SidebarRowContext<'_>) -> gpui::Div {
    div()
        .font(context.font.clone())
        .text_size(context.font_size)
        .text_color(context.colors.text_muted)
        .child(text)
}

fn section_header(
    title: &'static str,
    action: Option<AnyElement>,
    context: SidebarRowContext<'_>,
) -> AnyElement {
    h_flex()
        .w_full()
        .pt_2()
        .pb_0p5()
        .px_1p5()
        .justify_between()
        .child(sidebar_text(title, context))
        .children(action)
        .into_any_element()
}

fn sidebar_hint(text: &'static str, context: SidebarRowContext<'_>) -> AnyElement {
    sidebar_text(text, context)
        .px_1p5()
        .py_1()
        .into_any_element()
}

/// The Projects header, and under it a hint while there are none.
fn push_projects_header(
    rows: &mut Vec<AnyElement>,
    no_projects: bool,
    context: SidebarRowContext<'_>,
) {
    rows.push(projects_header(context));
    if no_projects {
        rows.push(sidebar_hint(
            "Add a project to give its tabs and worktrees a group of their own.",
            context,
        ));
    }
}

fn projects_header(context: SidebarRowContext<'_>) -> AnyElement {
    section_header(
        "Projects",
        Some(
            row_button(
                "sidebar-add-project",
                IconName::Plus,
                "Add a project",
                |_, window, cx| window.dispatch_action(Box::new(OpenProjects), cx),
            )
            .into_any_element(),
        ),
        context,
    )
}

/// A right-click menu around `row`, built fresh each time it opens.
fn with_row_menu(
    id: impl Into<ElementId>,
    row: ListItem,
    handle: &WeakEntity<Zetta>,
    build: impl Fn(ui::ContextMenu) -> ui::ContextMenu + 'static,
) -> AnyElement {
    let build = Rc::new(build);
    let dismiss_handle = handle.clone();
    ui::right_click_menu::<ui::ContextMenu>(id)
        .menu(move |window, cx| {
            let build = build.clone();
            let menu = ui::ContextMenu::build(window, cx, move |menu, _, _| build(menu));
            restore_focus_on_dismiss(&menu, dismiss_handle.clone(), window, cx);
            menu
        })
        .trigger(move |_, _, _| row)
        .into_any_element()
}

fn main_row(tab_count: usize, active: bool, context: SidebarRowContext<'_>) -> AnyElement {
    let handle = context.handle;
    let row = ListItem::new("sidebar-main")
        .toggle_state(active)
        .start_slot(Icon::new(IconName::Terminal).size(IconSize::Small))
        .child(row_label("Main", context))
        // Always shown, like the Projects header's button, rather than
        // swapped in on hover: it is the only pointer route to a new group.
        .end_slot(
            h_flex()
                .gap_1()
                .children(tab_count_label(tab_count))
                .child(row_button(
                    "sidebar-new-tab-group",
                    IconName::Plus,
                    "New tab group",
                    |_, window, cx| window.dispatch_action(Box::new(NewTabGroup), cx),
                )),
        )
        .tooltip(Tooltip::text("The tabs that belong to no project"))
        .on_click(on_zetta_click(handle, |this, window, cx| {
            this.activate_tab_group(TabGroup::Main, window, cx);
        }));
    let menu_handle = handle.clone();
    with_row_menu("sidebar-main-menu", row, handle, move |menu| {
        menu.entry(
            "New Tab",
            None,
            on_zetta(&menu_handle, |this, window, cx| {
                this.open_new_tab_in_group(TabGroup::Main, window, cx);
            }),
        )
    })
}

fn custom_row(
    position: usize,
    name: &str,
    tab_count: usize,
    active: bool,
    context: SidebarRowContext<'_>,
) -> AnyElement {
    let handle = context.handle;
    let group = TabGroup::Custom(name.to_owned());
    let row = ListItem::new(("sidebar-custom", position))
        .toggle_state(active)
        .start_slot(Icon::new(IconName::Blocks).size(IconSize::Small))
        .child(row_label(name.to_owned(), context))
        .end_slot::<AnyElement>(tab_count_label(tab_count))
        .end_slot_on_hover({
            let name = name.to_owned();
            row_button(
                ("sidebar-custom-remove", position),
                IconName::Close,
                "Remove group (its tabs move to Main)",
                on_zetta_click(handle, move |this, _, cx| {
                    this.remove_tab_group(&name, cx);
                }),
            )
        })
        .on_click({
            let group = group.clone();
            on_zetta_click(handle, move |this, window, cx| {
                this.activate_tab_group(group.clone(), window, cx);
            })
        });
    let menu_handle = handle.clone();
    let name = name.to_owned();
    with_row_menu(
        ("sidebar-custom-menu", position),
        row,
        handle,
        move |menu| {
            let group = group.clone();
            let rename = name.clone();
            let remove = name.clone();
            menu.entry(
                "New Tab",
                None,
                on_zetta(&menu_handle, move |this, window, cx| {
                    this.open_new_tab_in_group(group.clone(), window, cx);
                }),
            )
            .entry(
                "Rename…",
                None,
                on_zetta(&menu_handle, move |this, window, cx| {
                    this.prompt_rename_tab_group(rename.clone(), window, cx);
                }),
            )
            .separator()
            .entry(
                "Remove Group",
                None,
                on_zetta(&menu_handle, move |this, _, cx| {
                    this.remove_tab_group(&remove, cx);
                }),
            )
        },
    )
}

fn project_row(
    position: usize,
    entry: &SidebarEntry,
    context: SidebarRowContext<'_>,
) -> AnyElement {
    let SidebarEntry::Project {
        root,
        label,
        tab_count,
        active,
        expanded,
        has_worktrees,
        can_create_worktree,
        loading,
        unavailable,
    } = entry
    else {
        unreachable!("project_row is only given project entries");
    };
    let handle = context.handle;
    let group = TabGroup::Project(root.clone());
    let can_create_worktree = cfg!(feature = "worktree") && *can_create_worktree;
    let tooltip: SharedString = match unavailable {
        Some(reason) => format!("{}\n{reason}", root.display()).into(),
        None => root.display().to_string().into(),
    };
    let new_worktree = can_create_worktree.then(|| {
        let root = root.clone();
        row_button(
            ("sidebar-project-new-worktree", position),
            IconName::GitBranchPlus,
            "New worktree",
            on_zetta_click(handle, move |this, window, cx| {
                this.prompt_new_worktree(root.clone(), window, cx);
            }),
        )
    });
    let row = ListItem::new(("sidebar-project", position))
        .toggle_state(*active)
        .when(*has_worktrees, |row| {
            let root = root.clone();
            row.toggle(*expanded)
                .on_toggle(on_zetta_click(handle, move |this, _, cx| {
                    this.toggle_sidebar_project(&root, cx);
                }))
        })
        .start_slot(
            Icon::new(if *active {
                IconName::FolderOpen
            } else {
                IconName::Folder
            })
            .size(IconSize::Small)
            .when(*loading, |icon| icon.color(Color::Muted)),
        )
        .child(row_label(label.clone(), context))
        .end_slot::<AnyElement>(tab_count_label(*tab_count))
        .when_some(new_worktree, |row, button| row.end_slot_on_hover(button))
        .tooltip(Tooltip::text(tooltip))
        .on_click({
            let group = group.clone();
            on_zetta_click(handle, move |this, window, cx| {
                this.activate_tab_group(group.clone(), window, cx);
            })
        });
    let menu_handle = handle.clone();
    let (root, expanded, has_worktrees) = (root.clone(), *expanded, *has_worktrees);
    with_row_menu(
        ("sidebar-project-menu", position),
        row,
        handle,
        move |menu| {
            let group = group.clone();
            let new_worktree_root = root.clone();
            let refresh_root = root.clone();
            let toggle_root = root.clone();
            menu.entry(
                "New Tab",
                None,
                on_zetta(&menu_handle, move |this, window, cx| {
                    this.open_new_tab_in_group(group.clone(), window, cx);
                }),
            )
            .when(can_create_worktree, |menu| {
                menu.entry(
                    "New Worktree…",
                    None,
                    on_zetta(&menu_handle, move |this, window, cx| {
                        this.prompt_new_worktree(new_worktree_root.clone(), window, cx);
                    }),
                )
            })
            .when(has_worktrees, |menu| {
                menu.entry(
                    if expanded {
                        "Hide Worktrees"
                    } else {
                        "Show Worktrees"
                    },
                    None,
                    on_zetta(&menu_handle, move |this, _, cx| {
                        this.toggle_sidebar_project(&toggle_root, cx);
                    }),
                )
            })
            .when(cfg!(feature = "worktree"), |menu| {
                menu.entry(
                    "Refresh Worktrees",
                    None,
                    on_zetta(&menu_handle, move |this, _, cx| {
                        this.refresh_project_worktrees(refresh_root.clone(), cx);
                    }),
                )
            })
            .separator()
            .action("Manage Projects…", Box::new(OpenProjects))
        },
    )
}

/// What a worktree row draws.
struct WorktreeRow<'a> {
    project: &'a Path,
    worktree: &'a SidebarWorktree,
    tab_count: usize,
    active: bool,
}

fn worktree_row(
    position: usize,
    row: WorktreeRow<'_>,
    context: SidebarRowContext<'_>,
) -> AnyElement {
    let WorktreeRow {
        project,
        worktree,
        tab_count,
        active,
    } = row;
    let handle = context.handle;
    let group = TabGroup::Worktree {
        project: project.to_path_buf(),
        root: worktree.path.clone(),
    };
    let tooltip: SharedString = match (&worktree.branch, &worktree.source_branch) {
        (Some(branch), Some(source)) => {
            format!("{}\n{branch}, from {source}", worktree.path.display())
        }
        (Some(branch), None) => format!("{}\n{branch}", worktree.path.display()),
        (None, _) => worktree.path.display().to_string(),
    }
    .into();
    let item = ListItem::new(("sidebar-worktree", position))
        .indent_level(1)
        .toggle_state(active)
        .start_slot(Icon::new(IconName::GitWorktree).size(IconSize::Small))
        .child(row_label(worktree.label.clone(), context))
        .end_slot::<AnyElement>(tab_count_label(tab_count))
        .tooltip(Tooltip::text(tooltip))
        .on_click({
            let group = group.clone();
            on_zetta_click(handle, move |this, window, cx| {
                this.activate_tab_group(group.clone(), window, cx);
            })
        });
    let menu_handle = handle.clone();
    let project = project.to_path_buf();
    let worktree = worktree.clone();
    with_row_menu(
        ("sidebar-worktree-menu", position),
        item,
        handle,
        move |menu| {
            let group = group.clone();
            let source = worktree
                .source_branch
                .clone()
                .unwrap_or_else(|| "its source".to_owned());
            let finish = |finish: WorktreeFinish| {
                let (project, worktree) = (project.clone(), worktree.clone());
                on_zetta(&menu_handle, move |this, window, cx| {
                    this.prompt_finish_worktree(
                        project.clone(),
                        worktree.clone(),
                        finish,
                        window,
                        cx,
                    );
                })
            };
            let sync = {
                let (project, worktree) = (project.clone(), worktree.clone());
                on_zetta(&menu_handle, move |this, _, cx| {
                    this.sync_sidebar_worktree(project.clone(), worktree.clone(), cx);
                })
            };
            menu.entry(
                "New Tab",
                None,
                on_zetta(&menu_handle, move |this, window, cx| {
                    this.open_new_tab_in_group(group.clone(), window, cx);
                }),
            )
            .when(worktree.is_managed(), |menu| {
                menu.separator()
                    .entry(format!("Sync with {source}"), None, sync)
                    .entry(
                        format!("Integrate into {source}…"),
                        None,
                        finish(WorktreeFinish::Integrate),
                    )
                    .entry("Discard Worktree…", None, finish(WorktreeFinish::Discard))
            })
        },
    )
}

fn unlisted_row(
    position: usize,
    group: &TabGroup,
    tab_count: usize,
    active: bool,
    context: SidebarRowContext<'_>,
) -> AnyElement {
    let tooltip: SharedString = match group {
        TabGroup::Main => "Main".into(),
        TabGroup::Custom(name) => name.clone().into(),
        TabGroup::Project(root) => {
            format!("{}\nNo longer a registered project", root.display()).into()
        }
        TabGroup::Worktree { root, .. } => {
            format!("{}\nIts project is no longer registered", root.display()).into()
        }
    };
    let group = group.clone();
    ListItem::new(("sidebar-unlisted", position))
        .toggle_state(active)
        .start_slot(
            Icon::new(match group {
                TabGroup::Worktree { .. } => IconName::GitWorktree,
                TabGroup::Custom(_) => IconName::Blocks,
                TabGroup::Main | TabGroup::Project(_) => IconName::Folder,
            })
            .size(IconSize::Small)
            .color(Color::Muted),
        )
        .child(row_label(group.label(), context))
        .end_slot::<AnyElement>(tab_count_label(tab_count))
        .tooltip(Tooltip::text(tooltip))
        .on_click(on_zetta_click(context.handle, move |this, window, cx| {
            this.activate_tab_group(group.clone(), window, cx);
        }))
        .into_any_element()
}
