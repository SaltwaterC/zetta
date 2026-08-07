use super::*;

const MINIMUM_PANE_COLUMNS: usize = 2;

const MINIMUM_PANE_ROWS: usize = 1;

const PANE_RESIZE_REPEAT_DELAY: Duration = Duration::from_millis(400);

const PANE_RESIZE_REPEAT_INTERVAL: Duration = Duration::from_millis(75);

const PANE_SPLIT_SEPARATOR_SIZE: Pixels = px(1.);

fn resize_cell_count(current: usize, delta: isize, minimum: usize) -> usize {
    if delta.is_negative() {
        current.saturating_sub(delta.unsigned_abs()).max(minimum)
    } else {
        current.saturating_add(delta as usize).max(minimum)
    }
}

fn pane_resize_cell_delta(
    layout: &PaneLayout,
    pane_id: u64,
    axis: SplitAxis,
    directional_delta: isize,
) -> isize {
    if layout
        .resize_boundary(pane_id, axis)
        .is_some_and(|boundary| !boundary.active_is_first)
    {
        -directional_delta
    } else {
        directional_delta
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum PaneResizeDirection {
    Left,
    Right,
    Up,
    Down,
}

impl PaneResizeDirection {
    fn from_key(key: &str) -> Option<Self> {
        match key {
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            "up" => Some(Self::Up),
            "down" => Some(Self::Down),
            _ => None,
        }
    }

    fn bit(self) -> u8 {
        match self {
            Self::Left => 1 << 0,
            Self::Right => 1 << 1,
            Self::Up => 1 << 2,
            Self::Down => 1 << 3,
        }
    }
}

#[derive(Default)]
pub(crate) struct PaneResizeKeys {
    pressed: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PaneResizeGutter {
    pub(crate) tab_id: u64,
    pub(crate) first_pane: u64,
    pub(crate) second_pane: u64,
    pub(crate) axis: SplitAxis,
}

pub(crate) struct PaneResizeDrag {
    gutter: PaneResizeGutter,
    first_panes: Vec<u64>,
    second_panes: Vec<u64>,
}

/// Identifies a pane being dragged in pane-move mode. The same value serves
/// as both the drag payload (this pane is being dragged) and, when rendered
/// for a different pane, the drop target's identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PaneMoveDrag {
    pub(crate) tab_id: u64,
    pub(crate) pane_id: u64,
}

impl PaneResizeKeys {
    /// Returns whether `direction` was newly pressed.
    fn press(&mut self, direction: PaneResizeDirection) -> bool {
        let bit = direction.bit();
        if self.pressed & bit == 0 {
            self.pressed |= bit;
            true
        } else {
            false
        }
    }

    fn release(&mut self, direction: PaneResizeDirection) {
        self.pressed &= !direction.bit();
    }

    fn clear(&mut self) {
        self.pressed = 0;
    }

    fn is_empty(&self) -> bool {
        self.pressed == 0
    }

    fn len(&self) -> u32 {
        self.pressed.count_ones()
    }

    fn delta(&self) -> (isize, isize) {
        let held = |direction: PaneResizeDirection| self.pressed & direction.bit() != 0;
        (
            (held(PaneResizeDirection::Right) as isize)
                - (held(PaneResizeDirection::Left) as isize),
            (held(PaneResizeDirection::Down) as isize) - (held(PaneResizeDirection::Up) as isize),
        )
    }
}

#[derive(Default)]
struct WindowResize {
    width_delta: f32,
    height_delta: f32,
}

impl WindowResize {
    fn add(&mut self, axis: SplitAxis, delta: f32) {
        match axis {
            SplitAxis::Vertical => self.width_delta += delta,
            SplitAxis::Horizontal => self.height_delta += delta,
        }
    }
}

fn minimum_resized_window_extent(current: f32, requested: f32, minimum: Pixels) -> f32 {
    requested.max(current.min(f32::from(minimum)))
}

fn resize_window(window: &mut Window, resize: WindowResize, cx: &App) -> bool {
    if resize.width_delta == 0. && resize.height_delta == 0. {
        return false;
    }
    let bounds = window.bounds();
    let current_width = f32::from(bounds.size.width);
    let current_height = f32::from(bounds.size.height);
    let mut requested_width = current_width + resize.width_delta;
    let mut requested_height = current_height + resize.height_delta;

    // Clamp programmatic resizes before issuing them so resize-mode keypresses
    // never produce an undersized window.
    requested_width = minimum_resized_window_extent(
        current_width,
        requested_width,
        ZETTA_MINIMUM_WINDOW_SIZE.width,
    );
    requested_height = minimum_resized_window_extent(
        current_height,
        requested_height,
        ZETTA_MINIMUM_WINDOW_SIZE.height,
    );

    if window.is_maximized() {
        let wants_growth =
            resize.width_delta.is_sign_positive() || resize.height_delta.is_sign_positive();
        if resize.width_delta.is_sign_positive() {
            requested_width = current_width;
        }
        if resize.height_delta.is_sign_positive() {
            requested_height = current_height;
        }
        if wants_growth {
            // A maximized window is pinned to the screen bounds, so growth has
            // nowhere to go. Un-maximize so the next resize can actually widen
            // or heighten the window, matching floating-window behavior.
            window.zoom_window();
        }
    } else if window.is_fullscreen() {
        if resize.width_delta.is_sign_positive() {
            requested_width = current_width;
        }
        if resize.height_delta.is_sign_positive() {
            requested_height = current_height;
        }
    }
    if (resize.width_delta.is_sign_positive() || resize.height_delta.is_sign_positive())
        && let Some(display) = window.display(cx)
    {
        let visible = display.visible_bounds();
        if resize.width_delta.is_sign_positive() {
            let maximum = f32::from(visible.right() - bounds.origin.x);
            requested_width = requested_width.min(maximum).max(current_width);
        }
        if resize.height_delta.is_sign_positive() {
            let maximum = f32::from(visible.bottom() - bounds.origin.y);
            requested_height = requested_height.min(maximum).max(current_height);
        }
    }
    if requested_width <= 0.
        || requested_height <= 0.
        || ((requested_width - current_width).abs() < f32::EPSILON
            && (requested_height - current_height).abs() < f32::EPSILON)
    {
        return false;
    }

    window.resize(size(px(requested_width), px(requested_height)));
    true
}

impl Zetta {
    pub(crate) fn toggle_pane_resize_mode(
        &mut self,
        _: &TogglePaneResizeMode,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .tabs
            .get(self.active_tab)
            .is_none_or(|tab| tab.active_pane().is_none())
        {
            return;
        }
        self.pane_resize_mode = !self.pane_resize_mode;
        self.pane_resize_keys.clear();
        self.cancel_pane_resize_repeat();
        self.pane_resize_drag = None;
        if self.pane_resize_mode {
            self.pane_move_mode = false;
        }
        let input_enabled = pane_input_enabled(self.pane_resize_mode || self.pane_move_mode);
        for view in self
            .tabs
            .iter()
            .flat_map(|tab| tab.panes.iter())
            .filter_map(|pane| pane.view.as_ref())
        {
            view.update(cx, |view, cx| view.set_input_enabled(input_enabled, cx));
        }
        cx.notify();
    }

    pub(crate) fn toggle_pane_move_mode(
        &mut self,
        _: &TogglePaneMoveMode,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .tabs
            .get(self.active_tab)
            .is_none_or(|tab| tab.active_pane().is_none())
        {
            return;
        }
        self.pane_move_mode = !self.pane_move_mode;
        if self.pane_move_mode {
            self.pane_resize_mode = false;
            self.pane_resize_keys.clear();
            self.cancel_pane_resize_repeat();
            self.pane_resize_drag = None;
        }
        let input_enabled = pane_input_enabled(self.pane_resize_mode || self.pane_move_mode);
        for view in self
            .tabs
            .iter()
            .flat_map(|tab| tab.panes.iter())
            .filter_map(|pane| pane.view.as_ref())
        {
            view.update(cx, |view, cx| view.set_input_enabled(input_enabled, cx));
        }
        cx.notify();
    }

    pub(crate) fn move_pane_left(
        &mut self,
        _: &MovePaneLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_active_pane(PaneDirection::Left, window, cx);
    }

    pub(crate) fn move_pane_right(
        &mut self,
        _: &MovePaneRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_active_pane(PaneDirection::Right, window, cx);
    }

    pub(crate) fn move_pane_up(
        &mut self,
        _: &MovePaneUp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_active_pane(PaneDirection::Up, window, cx);
    }

    pub(crate) fn move_pane_down(
        &mut self,
        _: &MovePaneDown,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_active_pane(PaneDirection::Down, window, cx);
    }

    fn move_active_pane(
        &mut self,
        direction: PaneDirection,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.pane_move_mode {
            return;
        }
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        if tab.maximized_pane.is_some() {
            return;
        }
        if !tab.layout.move_pane(tab.active_pane, direction) {
            return;
        }
        for terminal in tab.panes.iter().filter_map(|pane| pane.terminal.as_ref()) {
            terminal.update(cx, |terminal, _| terminal.truncate_on_next_resize());
        }
        cx.notify();
    }

    pub(crate) fn move_pane_via_drag(
        &mut self,
        dragged: PaneMoveDrag,
        target: PaneMoveDrag,
        cx: &mut Context<Self>,
    ) {
        if !self.pane_move_mode
            || dragged.tab_id != target.tab_id
            || dragged.pane_id == target.pane_id
        {
            return;
        }
        let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == target.tab_id) else {
            return;
        };
        if tab.maximized_pane.is_some() {
            return;
        }
        if !tab.layout.swap_panes(dragged.pane_id, target.pane_id) {
            return;
        }
        tab.activate_pane(dragged.pane_id);
        for terminal in tab.panes.iter().filter_map(|pane| pane.terminal.as_ref()) {
            terminal.update(cx, |terminal, _| terminal.truncate_on_next_resize());
        }
        cx.notify();
    }

    pub(crate) fn resize_pane_left(
        &mut self,
        _: &ResizePaneLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.resize_active_pane_in_direction(PaneResizeDirection::Left, window, cx);
    }

    pub(crate) fn resize_pane_right(
        &mut self,
        _: &ResizePaneRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.resize_active_pane_in_direction(PaneResizeDirection::Right, window, cx);
    }

    pub(crate) fn resize_pane_up(
        &mut self,
        _: &ResizePaneUp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.resize_active_pane_in_direction(PaneResizeDirection::Up, window, cx);
    }

    pub(crate) fn resize_pane_down(
        &mut self,
        _: &ResizePaneDown,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.resize_active_pane_in_direction(PaneResizeDirection::Down, window, cx);
    }

    pub(crate) fn pane_resize_key_up(
        &mut self,
        event: &KeyUpEvent,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        if self.pane_resize_mode
            && let Some(direction) = PaneResizeDirection::from_key(&event.keystroke.key)
        {
            self.pane_resize_keys.release(direction);
            if self.pane_resize_keys.is_empty() {
                self.cancel_pane_resize_repeat();
            }
        }
    }

    fn resize_active_pane_in_direction(
        &mut self,
        direction: PaneResizeDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.pane_resize_mode {
            return;
        }
        let held_key_count = self.pane_resize_keys.len();
        if !self.pane_resize_keys.press(direction) {
            // Preserve the platform's native repeat for an ordinary one-key
            // resize. Synthetic repeat is only needed once multiple held keys
            // must be combined into a single two-axis operation.
            if held_key_count == 1 {
                let (columns_delta, rows_delta) = match direction {
                    PaneResizeDirection::Left => (-1, 0),
                    PaneResizeDirection::Right => (1, 0),
                    PaneResizeDirection::Up => (0, -1),
                    PaneResizeDirection::Down => (0, 1),
                };
                self.resize_active_pane_by_cells(columns_delta, rows_delta, window, cx);
            }
            return;
        }
        self.resize_active_pane_by_held_keys(window, cx);
        if held_key_count == 1 {
            self.start_pane_resize_repeat(window, cx);
        }
    }

    fn resize_active_pane_by_held_keys(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (columns_delta, rows_delta) = self.pane_resize_keys.delta();
        if columns_delta == 0 && rows_delta == 0 {
            return;
        }
        self.resize_active_pane_by_cells(columns_delta, rows_delta, window, cx);
    }

    fn start_pane_resize_repeat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.pane_resize_repeat_generation = self.pane_resize_repeat_generation.wrapping_add(1);
        let generation = self.pane_resize_repeat_generation;
        let this = cx.entity().downgrade();
        let executor = cx.background_executor().clone();
        window
            .spawn(cx, async move |cx| {
                executor.timer(PANE_RESIZE_REPEAT_DELAY).await;
                loop {
                    let repeating = this
                        .update_in(cx, |this, window, cx| {
                            let repeating = this.pane_resize_mode
                                && this.pane_resize_repeat_generation == generation
                                && !this.pane_resize_keys.is_empty();
                            if repeating {
                                this.resize_active_pane_by_held_keys(window, cx);
                            }
                            repeating
                        })
                        .unwrap_or(false);
                    if !repeating {
                        break;
                    }
                    executor.timer(PANE_RESIZE_REPEAT_INTERVAL).await;
                }
            })
            .detach();
    }

    fn cancel_pane_resize_repeat(&mut self) {
        self.pane_resize_repeat_generation = self.pane_resize_repeat_generation.wrapping_add(1);
    }

    pub(crate) fn resize_pane_gutter_drag(
        &mut self,
        gutter: PaneResizeGutter,
        split_bounds: Bounds<Pixels>,
        pointer_position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if !self.pane_resize_mode {
            return;
        }
        if self
            .pane_resize_drag
            .as_ref()
            .is_none_or(|drag| drag.gutter != gutter)
            && !self.begin_pane_resize_drag(gutter)
        {
            return;
        }

        let Some(tab_index) = self.tabs.iter().position(|tab| tab.id == gutter.tab_id) else {
            return;
        };
        let Some(drag) = self.pane_resize_drag.as_ref() else {
            return;
        };
        let (split_start, split_extent, pointer_coordinate) = match gutter.axis {
            SplitAxis::Vertical => (
                split_bounds.left(),
                split_bounds.size.width,
                pointer_position.x,
            ),
            SplitAxis::Horizontal => (
                split_bounds.top(),
                split_bounds.size.height,
                pointer_position.y,
            ),
        };
        let available_extent = f32::from(split_extent) - f32::from(PANE_SPLIT_SEPARATOR_SIZE);
        if available_extent <= 0. {
            return;
        }
        let Some(first_ratio) = self.tabs[tab_index].layout.split_ratio(
            gutter.first_pane,
            gutter.second_pane,
            gutter.axis,
        ) else {
            return;
        };

        let current_first_extent = available_extent * first_ratio;
        let requested_first_extent = (f32::from(pointer_coordinate - split_start)
            - f32::from(PANE_SPLIT_SEPARATOR_SIZE) / 2.)
            .clamp(0., available_extent);
        let first_capacity =
            self.minimum_pane_capacity(tab_index, &drag.first_panes, gutter.axis, cx);
        let second_capacity =
            self.minimum_pane_capacity(tab_index, &drag.second_panes, gutter.axis, cx);
        let layout_delta =
            (requested_first_extent - current_first_extent).clamp(-first_capacity, second_capacity);
        if layout_delta == 0. {
            return;
        }

        if self.tabs[tab_index].layout.adjust_split_ratio(
            gutter.first_pane,
            gutter.second_pane,
            gutter.axis,
            layout_delta / available_extent,
        ) {
            // A gutter drag changes terminal geometry just like keyboard pane
            // resizing, so defer scrollback reflow until that resize arrives.
            for terminal in self.tabs[tab_index]
                .panes
                .iter()
                .filter_map(|pane| pane.terminal.as_ref())
            {
                terminal.update(cx, |terminal, _| terminal.truncate_on_next_resize());
            }
            cx.notify();
        }
    }

    fn begin_pane_resize_drag(&mut self, gutter: PaneResizeGutter) -> bool {
        let Some(tab) = self.tabs.iter().find(|tab| tab.id == gutter.tab_id) else {
            return false;
        };
        if tab.maximized_pane.is_some() || !tab.minimized_panes.is_empty() {
            return false;
        }
        let Some((first_panes, second_panes)) =
            tab.layout
                .split_panes(gutter.first_pane, gutter.second_pane, gutter.axis)
        else {
            return false;
        };
        self.pane_resize_drag = Some(PaneResizeDrag {
            gutter,
            first_panes,
            second_panes,
        });
        true
    }

    fn resize_active_pane_by_cells(
        &mut self,
        columns_delta: isize,
        rows_delta: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.pane_resize_mode {
            return;
        }
        let (tab_id, pane_id, bounds, columns_delta, rows_delta) = {
            let Some(tab) = self.tabs.get_mut(self.active_tab) else {
                return;
            };
            // The terminal focus is authoritative while a keybinding is being
            // handled. This also protects against focus notifications that
            // have not yet updated Tab::active_pane.
            let pane_id = tab
                .panes
                .iter()
                .find(|pane| {
                    pane.view
                        .as_ref()
                        .is_some_and(|view| view.focus_handle(cx).contains_focused(window, cx))
                })
                .map(|pane| pane.id)
                .unwrap_or(tab.active_pane);
            tab.activate_pane(pane_id);
            let Some(bounds) = tab
                .pane(pane_id)
                .and_then(|pane| pane.terminal.as_ref())
                .map(|terminal| terminal.read(cx).last_content().terminal_bounds)
            else {
                return;
            };
            let layout = tab.visible_layout();
            let columns_delta = layout.as_ref().map_or(columns_delta, |layout| {
                pane_resize_cell_delta(layout, pane_id, SplitAxis::Vertical, columns_delta)
            });
            let rows_delta = layout.as_ref().map_or(rows_delta, |layout| {
                pane_resize_cell_delta(layout, pane_id, SplitAxis::Horizontal, rows_delta)
            });
            (tab.id, pane_id, bounds, columns_delta, rows_delta)
        };
        let columns = resize_cell_count(bounds.num_columns(), columns_delta, MINIMUM_PANE_COLUMNS);
        let rows = resize_cell_count(bounds.num_lines(), rows_delta, MINIMUM_PANE_ROWS);
        self.resize_pane_to(tab_id, pane_id, Some(columns), Some(rows), window, cx);
    }

    pub(crate) fn resize_pane_to(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        columns: Option<usize>,
        rows: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab_index) = self.tabs.iter().position(|tab| tab.id == tab_id) else {
            return;
        };
        let Some(bounds) = self.tabs[tab_index]
            .pane(pane_id)
            .and_then(|pane| pane.terminal.as_ref())
            .map(|terminal| terminal.read(cx).last_content().terminal_bounds)
        else {
            return;
        };

        let mut changed = false;
        let mut window_resize = WindowResize::default();
        if let Some(columns) = columns {
            let (layout_changed, window_delta) = self.resize_pane_axis(
                tab_index,
                pane_id,
                SplitAxis::Vertical,
                columns.max(MINIMUM_PANE_COLUMNS),
                bounds.num_columns(),
                bounds.cell_width(),
                cx,
            );
            changed |= layout_changed;
            window_resize.add(SplitAxis::Vertical, window_delta);
        }
        if let Some(rows) = rows {
            let (layout_changed, window_delta) = self.resize_pane_axis(
                tab_index,
                pane_id,
                SplitAxis::Horizontal,
                rows.max(MINIMUM_PANE_ROWS),
                bounds.num_lines(),
                bounds.line_height(),
                cx,
            );
            changed |= layout_changed;
            window_resize.add(SplitAxis::Horizontal, window_delta);
        }
        changed |= resize_window(window, window_resize, cx);
        if changed {
            // The next terminal size change is driven by pane geometry, so do
            // not synchronously reflow retained scrollback for every keypress.
            if let Some(tab) = self.tabs.get(tab_index) {
                for terminal in tab.panes.iter().filter_map(|pane| pane.terminal.as_ref()) {
                    terminal.update(cx, |terminal, _| terminal.truncate_on_next_resize());
                }
            }
            cx.notify();
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn resize_pane_axis(
        &mut self,
        tab_index: usize,
        pane_id: u64,
        axis: SplitAxis,
        requested_cells: usize,
        current_cells: usize,
        cell_size: Pixels,
        cx: &mut Context<Self>,
    ) -> (bool, f32) {
        if requested_cells == current_cells {
            return (false, 0.);
        }
        let requested_delta =
            (requested_cells as isize - current_cells as isize) as f32 * f32::from(cell_size);
        let (active_region, boundary, can_adjust_layout) = {
            let tab = &self.tabs[tab_index];
            let Some(layout) = tab.visible_layout() else {
                return (false, 0.);
            };
            let Some(region) = layout
                .regions()
                .into_iter()
                .find(|region| region.id == pane_id)
            else {
                return (false, 0.);
            };
            let region_fraction = match axis {
                SplitAxis::Vertical => region.right - region.left,
                SplitAxis::Horizontal => region.bottom - region.top,
            };
            (
                region_fraction,
                layout.resize_boundary(pane_id, axis),
                tab.maximized_pane.is_none() && tab.minimized_panes.is_empty(),
            )
        };
        if active_region <= f32::EPSILON {
            return (false, 0.);
        }

        let current_pixels = current_cells as f32 * f32::from(cell_size);
        let root_pixels = current_pixels / active_region;
        let mut remaining_delta = requested_delta;
        let mut changed = false;

        if can_adjust_layout && let Some(boundary) = boundary {
            let parent_pixels = root_pixels * boundary.parent_fraction;
            if parent_pixels > 0. {
                let layout_delta = if requested_delta.is_sign_positive() {
                    let available =
                        self.minimum_pane_capacity(tab_index, &boundary.sibling_panes, axis, cx);
                    requested_delta.min(available)
                } else {
                    requested_delta
                };
                if layout_delta != 0.
                    && self.tabs[tab_index].layout.adjust_resize_boundary(
                        pane_id,
                        axis,
                        layout_delta / parent_pixels,
                    )
                {
                    remaining_delta -= layout_delta;
                    changed = true;
                }
            }
        }

        let window_delta = if remaining_delta != 0. {
            let target_fraction = (active_region
                + (requested_delta - remaining_delta) / root_pixels)
                .max(f32::EPSILON);
            remaining_delta / target_fraction
        } else {
            0.
        };
        (changed, window_delta)
    }

    fn minimum_pane_capacity(
        &self,
        tab_index: usize,
        sibling_panes: &[u64],
        axis: SplitAxis,
        cx: &App,
    ) -> f32 {
        sibling_panes
            .iter()
            .filter_map(|pane_id| self.tabs[tab_index].pane(*pane_id))
            .filter_map(|pane| pane.terminal.as_ref())
            .map(|terminal| {
                let bounds = terminal.read(cx).last_content().terminal_bounds;
                let (available, minimum) = match axis {
                    SplitAxis::Vertical => (
                        f32::from(bounds.width()),
                        f32::from(bounds.cell_width()) * MINIMUM_PANE_COLUMNS as f32,
                    ),
                    SplitAxis::Horizontal => (
                        f32::from(bounds.height()),
                        f32::from(bounds.line_height()) * MINIMUM_PANE_ROWS as f32,
                    ),
                };
                (available - minimum).max(0.)
            })
            .reduce(f32::min)
            .unwrap_or(0.)
    }
}

#[cfg(test)]
#[path = "tests/pane_resize.rs"]
mod tests;
