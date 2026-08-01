use std::cmp::max;
use std::rc::Rc;
use std::time::Duration;

use niri_config::utils::MergeWith as _;
use niri_config::{
    CenterFocusedColumn, CornerRadius, OutputName, PresetSize, Workspace as WorkspaceConfig,
};
use niri_ipc::{ColumnDisplay, PositionChange, SizeChange, WindowLayout};
use smithay::backend::renderer::element::utils::{
    Relocate, RelocateRenderElement, RescaleRenderElement,
};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::desktop::{layer_map_for_output, Window};
use smithay::output::Output;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Rectangle, Scale, Serial, Size, Transform};
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::SurfaceCachedState;

use super::floating::{FloatingSpace, FloatingSpaceRenderElement};
use super::grid_overview::{GridDirection, GridEntryInfo, GridItem, GridOverview};
use super::scrolling::{
    Column, ColumnWidth, ScrollDirection, ScrollingSpace, ScrollingSpaceRenderElement,
};
use super::shadow::Shadow;
use super::tab_indicator::TabIndicator;
use super::tile::{Tile, TileRenderElement, TileRenderSnapshot};
use super::{
    ActivateWindow, HitType, InsertPosition, InteractiveResizeData, LayoutElement, Options,
    RemovedTile, SizeFrac,
};
use crate::animation::Clock;
use crate::niri_render_elements;
use crate::render_helpers::renderer::NiriRenderer;
use crate::render_helpers::shadow::ShadowRenderElement;
use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};
use crate::render_helpers::xray::{Xray, XrayPos};
use crate::render_helpers::RenderCtx;
use crate::utils::id::IdCounter;
use crate::utils::transaction::{Transaction, TransactionBlocker};
use crate::utils::{
    ensure_min_max_size, ensure_min_max_size_maybe_zero, output_size, send_scale_transform,
    ResizeEdge,
};
use crate::window::ResolvedWindowRules;

#[derive(Debug)]
pub struct Workspace<W: LayoutElement> {
    /// The scrollable-tiling layout.
    scrolling: ScrollingSpace<W>,

    /// The floating layout.
    floating: FloatingSpace<W>,

    /// Whether the floating layout is active instead of the scrolling layout.
    floating_is_active: FloatingActive,

    /// The original output of this workspace.
    ///
    /// Most of the time this will be the workspace's current output, however, after an output
    /// disconnection, it may remain pointing to the disconnected output.
    pub(super) original_output: OutputId,

    /// Current output of this workspace.
    output: Option<Output>,

    /// Latest known output scale for this workspace.
    ///
    /// This should be set from the current workspace output, or, if all outputs have been
    /// disconnected, preserved until a new output is connected.
    scale: smithay::output::Scale,

    /// Latest known output transform for this workspace.
    ///
    /// This should be set from the current workspace output, or, if all outputs have been
    /// disconnected, preserved until a new output is connected.
    transform: Transform,

    /// Latest known view size for this workspace.
    ///
    /// This should be computed from the current workspace output size, or, if all outputs have
    /// been disconnected, preserved until a new output is connected.
    view_size: Size<f64, Logical>,

    /// Latest known working area for this workspace.
    ///
    /// Not rounded to physical pixels.
    ///
    /// This is similar to view size, but takes into account things like layer shell exclusive
    /// zones.
    working_area: Rectangle<f64, Logical>,

    /// This workspace's shadow in the overview.
    shadow: Shadow,

    /// This workspace's background.
    background_buffer: SolidColorBuffer,

    /// Clock for driving animations.
    pub(super) clock: Clock,

    /// Configurable properties of the layout as received from the parent monitor.
    pub(super) base_options: Rc<Options>,

    /// Configurable properties of the layout with logical sizes adjusted for the current `scale`.
    pub(super) options: Rc<Options>,

    /// Optional name of this workspace.
    pub(super) name: Option<String>,

    /// Layout config overrides for this workspace.
    layout_config: Option<niri_config::LayoutPart>,

    /// Grid overview state for this workspace.
    pub(super) grid_overview: Option<GridOverview<W>>,

    /// Unique ID of this workspace.
    id: WorkspaceId,
}

pub(super) struct GridWindowVisual<W: LayoutElement> {
    window_id: W::Id,
    item: GridItem<W>,
    column_tile_count: Option<usize>,
    pos: Point<f64, Logical>,
    scale: f64,
}

#[derive(Debug, Clone)]
pub struct OutputId(String);

impl OutputId {
    pub fn matches(&self, output: &Output) -> bool {
        let output_name = output.user_data().get::<OutputName>().unwrap();
        output_name.matches(&self.0)
    }
}

static WORKSPACE_ID_COUNTER: IdCounter = IdCounter::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WorkspaceId(u64);

impl WorkspaceId {
    fn next() -> WorkspaceId {
        WorkspaceId(WORKSPACE_ID_COUNTER.next())
    }

    pub fn get(self) -> u64 {
        self.0
    }

    pub fn specific(id: u64) -> Self {
        Self(id)
    }
}

niri_render_elements! {
    WorkspaceRenderElement<R> => {
        Scrolling = ScrollingSpaceRenderElement<R>,
        Floating = FloatingSpaceRenderElement<R>,
        GridTile = RelocateRenderElement<RescaleRenderElement<ScrollingSpaceRenderElement<R>>>,
    }
}

#[derive(Debug)]
pub(super) struct InteractiveResize<W: LayoutElement> {
    pub window: W::Id,
    pub original_window_size: Size<f64, Logical>,
    pub data: InteractiveResizeData,
}

/// Resolved width or height in logical pixels.
#[derive(Debug, Clone, Copy)]
pub enum ResolvedSize {
    /// Size of the tile including borders.
    Tile(f64),
    /// Size of the window excluding borders.
    Window(f64),
}

/// Whether the floating space is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FloatingActive {
    /// The scrolling space is active.
    No,
    /// The scrolling space is active, but the floating space should render on top, even if the
    /// active scrolling window is fullscreen.
    ///
    /// This is necessary for focus-follows-mouse that activates but doesn't raise the window to
    /// avoid being annoying.
    NoButRaised,
    /// The floating space is active.
    Yes,
}

/// Where to put a newly added window.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceAddWindowTarget<'a, W: LayoutElement> {
    /// No particular preference.
    #[default]
    Auto,
    /// As a new column at this index.
    NewColumnAt(usize),
    /// Next to this existing window.
    NextTo(&'a W::Id),
}

impl OutputId {
    pub fn new(output: &Output) -> Self {
        let output_name = output.user_data().get::<OutputName>().unwrap();
        Self(output_name.format_make_model_serial_or_connector())
    }
}

impl FloatingActive {
    fn get(self) -> bool {
        self == Self::Yes
    }
}

impl<W: LayoutElement> Workspace<W> {
    pub fn new(output: Output, clock: Clock, options: Rc<Options>) -> Self {
        Self::new_with_config(output, None, clock, options)
    }

    pub fn new_with_config(
        output: Output,
        mut config: Option<WorkspaceConfig>,
        clock: Clock,
        base_options: Rc<Options>,
    ) -> Self {
        let original_output = config
            .as_ref()
            .and_then(|c| c.open_on_output.clone())
            .map(OutputId)
            .unwrap_or(OutputId::new(&output));

        let layout_config = config.as_mut().and_then(|c| c.layout.take().map(|x| x.0));

        let scale = output.current_scale();
        let options = Rc::new(
            Options::clone(&base_options)
                .with_merged_layout(layout_config.as_ref())
                .adjusted_for_scale(scale.fractional_scale()),
        );

        let view_size = output_size(&output);
        let working_area = compute_working_area(&output);

        let scrolling = ScrollingSpace::new(
            view_size,
            working_area,
            scale.fractional_scale(),
            clock.clone(),
            options.clone(),
        );

        let floating = FloatingSpace::new(
            view_size,
            working_area,
            scale.fractional_scale(),
            clock.clone(),
            options.clone(),
        );

        let shadow_config =
            compute_workspace_shadow_config(options.overview.workspace_shadow, view_size);

        Self {
            scrolling,
            floating,
            floating_is_active: FloatingActive::No,
            original_output,
            scale,
            transform: output.current_transform(),
            view_size,
            working_area,
            shadow: Shadow::new(shadow_config),
            background_buffer: SolidColorBuffer::new(view_size, options.layout.background_color),
            output: Some(output),
            clock,
            base_options,
            options,
            name: config.map(|c| c.name.0),
            layout_config,
            grid_overview: None,
            id: WorkspaceId::next(),
        }
    }

    pub fn new_with_config_no_outputs(
        mut config: Option<WorkspaceConfig>,
        clock: Clock,
        base_options: Rc<Options>,
    ) -> Self {
        let original_output = OutputId(
            config
                .as_ref()
                .and_then(|c| c.open_on_output.clone())
                .unwrap_or_default(),
        );

        let layout_config = config.as_mut().and_then(|c| c.layout.take().map(|x| x.0));

        let scale = smithay::output::Scale::Integer(1);
        let options = Rc::new(
            Options::clone(&base_options)
                .with_merged_layout(layout_config.as_ref())
                .adjusted_for_scale(scale.fractional_scale()),
        );

        let view_size = Size::from((1280., 720.));
        let working_area = Rectangle::from_size(Size::from((1280., 720.)));

        let scrolling = ScrollingSpace::new(
            view_size,
            working_area,
            scale.fractional_scale(),
            clock.clone(),
            options.clone(),
        );

        let floating = FloatingSpace::new(
            view_size,
            working_area,
            scale.fractional_scale(),
            clock.clone(),
            options.clone(),
        );

        let shadow_config =
            compute_workspace_shadow_config(options.overview.workspace_shadow, view_size);

        Self {
            scrolling,
            floating,
            floating_is_active: FloatingActive::No,
            output: None,
            scale,
            transform: Transform::Normal,
            original_output,
            view_size,
            working_area,
            shadow: Shadow::new(shadow_config),
            background_buffer: SolidColorBuffer::new(view_size, options.layout.background_color),
            clock,
            base_options,
            options,
            name: config.map(|c| c.name.0),
            layout_config,
            grid_overview: None,
            id: WorkspaceId::next(),
        }
    }

    pub fn new_no_outputs(clock: Clock, options: Rc<Options>) -> Self {
        Self::new_with_config_no_outputs(None, clock, options)
    }

    pub fn id(&self) -> WorkspaceId {
        self.id
    }

    pub fn name(&self) -> Option<&String> {
        self.name.as_ref()
    }

    pub fn unname(&mut self) {
        self.name = None;
    }

    pub fn has_windows_or_name(&self) -> bool {
        self.has_windows() || self.name.is_some()
    }

    pub fn scale(&self) -> smithay::output::Scale {
        self.scale
    }

    pub fn advance_animations(&mut self) {
        self.scrolling.advance_animations();
        self.floating.advance_animations();
        self.advance_grid_overview_animations();
    }

    pub fn are_animations_ongoing(&self) -> bool {
        self.scrolling.are_animations_ongoing()
            || self.floating.are_animations_ongoing()
            || self.are_grid_overview_animations_ongoing()
    }

    pub fn are_transitions_ongoing(&self) -> bool {
        self.scrolling.are_transitions_ongoing() || self.floating.are_transitions_ongoing()
    }

    pub fn is_grid_overview_open(&self) -> bool {
        self.grid_overview.as_ref().map_or(false, |g| g.open)
    }

    pub fn grid_overview_progress(&self) -> Option<f64> {
        self.grid_overview
            .as_ref()
            .and_then(|g| g.progress.as_ref().map(|p| p.value()))
    }

    pub fn is_grid_overview_animation(&self) -> bool {
        self.grid_overview
            .as_ref()
            .map_or(false, |g| g.is_animation())
    }

    pub fn toggle_grid_overview(&mut self) {
        let was_open = self.is_grid_overview_open();
        if !was_open {
            let items = self.grid_overview_items();

            if items.is_empty() {
                return;
            }

            let mut go = GridOverview::new(self.clock.clone(), self.options.clone());
            go.saved_active_window_id = self.active_window().map(|w| w.id().clone());
            go.saved_view_offset = self.scrolling.view_pos();

            for (item, _) in &items {
                let normal = self
                    .grid_item_normal_render_pos(item, false)
                    .unwrap_or_else(|| Point::from((0., 0.)));
                go.entry_positions.push((item.clone(), normal));
            }

            go.compute_layout(&items, self.working_area, false);

            // Initialize column_tile_focus for Column items.
            for (item, _) in &go.layout.entries {
                if let GridItem::Column { col_idx, .. } = item {
                    let active_tile = self.scrolling.column_active_tile_idx(*col_idx);
                    go.column_tile_focus.push((*col_idx, active_tile));
                }
            }

            let focus_id = self.active_window().map(|w| w.id().clone());
            if let Some(ref fid) = focus_id {
                if let Some((row, col)) = go.find_grid_index(fid) {
                    go.focus = (row, col);
                }
            }

            go.toggle();
            self.grid_overview = Some(go);
        } else if self.grid_overview.is_some() {
            self.snapshot_grid_close_start_visuals();
            let Some(ref mut go) = self.grid_overview else {
                return;
            };
            go.toggle();
        }
    }

    pub fn close_grid_overview(&mut self) -> bool {
        if !self.is_grid_overview_open() {
            return false;
        }
        self.toggle_grid_overview();
        true
    }

    pub fn open_grid_overview(&mut self) -> bool {
        if self.is_grid_overview_open() {
            return false;
        }
        self.toggle_grid_overview();
        true
    }

    pub fn grid_navigate(&mut self, dir: GridDirection) -> bool {
        let previous = self.grid_focused_window_id();
        let tile_changed = self
            .grid_overview
            .as_mut()
            .and_then(|go| go.navigate(dir, |col_idx| self.scrolling.column_tile_count(col_idx)));

        if let Some((col_idx, new_tile_idx)) = tile_changed {
            // Update grid_overview state only; do not modify the real scrolling layout.
            if let Some(go) = self.grid_overview.as_mut() {
                if let Some(col) = self.scrolling.columns().nth(col_idx) {
                    if let Some((tile, _)) = col.tiles().nth(new_tile_idx) {
                        let window_id = tile.window().id().clone();
                        go.set_column_tile_focus(col_idx, new_tile_idx, window_id);
                    }
                }
            }
        }

        self.grid_focused_window_id() != previous
    }

    pub fn set_grid_grabbed_window(&mut self, id: Option<W::Id>) {
        if let Some(go) = &mut self.grid_overview {
            go.set_grabbed_window(id);
        }
    }

    pub fn grid_focused_window_id(&self) -> Option<W::Id> {
        let go = self.grid_overview.as_ref()?;

        // While a grab is ongoing, the grabbed window itself is the focused one.
        if let Some(id) = &go.grabbed_window {
            return Some(id.clone());
        }

        let item = go.focused_item()?;
        if let GridItem::Column {
            col_idx, window_id, ..
        } = item
        {
            let tile_idx = go.get_column_tile_focus(*col_idx);
            return self
                .scrolling
                .columns()
                .nth(*col_idx)
                .and_then(|col| col.tiles().nth(tile_idx))
                .map(|(tile, _)| tile.window().id().clone())
                .or_else(|| Some(window_id.clone()));
        }

        Some(item.window_id().clone())
    }

    pub(super) fn grid_expel_from_column_preferred_focus(&self) -> Option<W::Id> {
        let id = self.grid_focused_window_id()?;
        let item = self.grid_item_for_window(&id)?;
        let col_idx = match item {
            GridItem::Column { col_idx, .. } | GridItem::Tab { col_idx, .. } => col_idx,
            GridItem::Floating { .. } => return Some(id),
        };
        let col = self.scrolling.columns().nth(col_idx)?;
        let tile_idx = col.position(&id)?;
        let tile_count = col.tiles().count();

        if tile_count > 1 && tile_idx + 1 == tile_count {
            return col
                .tiles()
                .nth(tile_idx - 1)
                .map(|(tile, _)| tile.window().id().clone());
        }

        Some(id)
    }

    pub fn activate_grid_focused_window(&mut self) -> Option<W::Id> {
        let id = self.grid_focused_window_id()?;
        self.activate_window_from_grid(&id).then_some(id)
    }

    pub(super) fn activate_grid_focused_window_before_close(&mut self) -> bool {
        let Some(id) = self.grid_focused_window_id() else {
            return false;
        };

        if !self.set_grid_focus_for_window(&id) {
            return false;
        }
        if !self.activate_window_from_grid(&id) {
            return false;
        }

        self.set_grid_focus_for_window(&id);
        true
    }

    pub(super) fn grid_window_visual_snapshots(&self) -> Vec<GridWindowVisual<W>> {
        if !self.is_grid_overview_open() {
            return Vec::new();
        }

        self.windows()
            .filter_map(|window| {
                let window_id = window.id().clone();
                let item = self.grid_item_for_window(&window_id)?;
                let column_tile_count = self.grid_item_column_tile_count(&item);
                let (pos, scale) = self.grid_window_visual_transform(&window_id)?;
                Some(GridWindowVisual {
                    window_id,
                    item,
                    column_tile_count,
                    pos,
                    scale,
                })
            })
            .collect()
    }

    pub(super) fn grid_window_visual_snapshots_for_closing_window(
        &self,
        window: &W::Id,
    ) -> Vec<GridWindowVisual<W>> {
        if !self.is_grid_overview_open() {
            return Vec::new();
        }

        let Some(item) = self.grid_item_for_window(window) else {
            return Vec::new();
        };
        let Some(tile_count) = self.grid_item_column_tile_count(&item) else {
            return Vec::new();
        };
        if tile_count <= 1 {
            return Vec::new();
        }

        self.grid_window_visual_snapshots()
    }

    pub(super) fn grid_window_close_preserves_move_animations(&self, window: &W::Id) -> bool {
        if !self.is_grid_overview_open() {
            return false;
        }

        let Some(item) = self.grid_item_for_window(window) else {
            return false;
        };

        self.grid_item_column_tile_count(&item)
            .is_some_and(|count| count > 1)
    }

    fn grid_item_column_tile_count(&self, item: &GridItem<W>) -> Option<usize> {
        match item {
            GridItem::Column { col_idx, .. } | GridItem::Tab { col_idx, .. } => {
                Some(self.scrolling.column_tile_count(*col_idx))
            }
            GridItem::Floating { .. } => None,
        }
    }

    fn set_grid_window_transition_starts(&mut self, snapshots: Vec<GridWindowVisual<W>>) {
        let starts: Vec<_> = snapshots
            .into_iter()
            .filter_map(|snapshot| {
                let new_item = self.grid_item_for_window(&snapshot.window_id)?;
                let new_column_tile_count = self.grid_item_column_tile_count(&new_item);
                let topology_changed = !new_item.matches_animation_key(&snapshot.item);
                let target_changed = new_column_tile_count != snapshot.column_tile_count;
                (topology_changed || target_changed).then_some((
                    snapshot.window_id,
                    snapshot.pos,
                    snapshot.scale,
                ))
            })
            .collect();

        if let Some(go) = &mut self.grid_overview {
            go.set_window_transition_starts(starts);
        }
    }

    pub(super) fn refresh_grid_overview_after_action(
        &mut self,
        preferred_focus: Option<&W::Id>,
        stop_move_animations: bool,
        window_visual_snapshots: Vec<GridWindowVisual<W>>,
    ) {
        if !self.is_grid_overview_open() {
            return;
        }

        if stop_move_animations {
            self.scrolling.stop_move_animations();
        }
        self.recompute_grid_overview_layout(true);

        let focus_set =
            preferred_focus.is_some_and(|id| self.set_grid_focus_for_window_without_animation(id));
        if !focus_set {
            self.sync_grid_focus_to_active_window();
        }

        self.set_grid_window_transition_starts(window_visual_snapshots);
    }

    pub fn refresh_grid_entry_positions(&mut self) {
        let positions: Vec<_> = {
            let go = match &self.grid_overview {
                Some(g) => g,
                None => return,
            };
            go.layout
                .entries
                .iter()
                .map(|(item, _)| {
                    let pos = self
                        .grid_item_normal_render_pos(item, true)
                        .unwrap_or_else(|| Point::from((0., 0.)));
                    (item.clone(), pos)
                })
                .collect()
        };
        if let Some(ref mut go) = self.grid_overview {
            go.entry_positions = positions;
        }
    }

    fn snapshot_grid_close_start_visuals(&mut self) {
        let visuals: Vec<_> = {
            let go = match &self.grid_overview {
                Some(g) if g.open => g,
                _ => return,
            };

            go.layout
                .entries
                .iter()
                .map(|(item, info)| {
                    let fallback = self
                        .grid_item_normal_render_pos(item, false)
                        .unwrap_or(info.target_pos);
                    let (pos, scale) = go.entry_visual_transform(item, info, fallback);
                    (item.clone(), pos, scale)
                })
                .collect()
        };

        if let Some(ref mut go) = self.grid_overview {
            go.snapshot_close_start_visuals(visuals);
        }
    }

    pub(super) fn on_window_closed_in_grid(
        &mut self,
        window_visual_snapshots: Vec<GridWindowVisual<W>>,
        stop_move_animations: bool,
    ) {
        let previous_focus = self.grid_focused_window_id();
        if stop_move_animations && self.is_grid_overview_open() {
            self.scrolling.stop_move_animations();
        }
        self.recompute_grid_overview_layout(true);

        if let Some(id) = previous_focus {
            if self.set_grid_focus_for_window(&id) {
                self.set_grid_window_transition_starts(window_visual_snapshots);
                return;
            }
        }

        self.sync_grid_focus_to_active_window();
        self.set_grid_window_transition_starts(window_visual_snapshots);
    }

    pub fn on_window_added_in_grid(&mut self, id: &W::Id) {
        self.on_window_added_in_grid_impl(id, true);
    }

    pub fn on_window_added_in_grid_preserving_move_animations(&mut self, id: &W::Id) {
        self.on_window_added_in_grid_impl(id, false);
    }

    fn on_window_added_in_grid_impl(&mut self, id: &W::Id, stop_move_animations: bool) {
        if let Some(go) = &mut self.grid_overview {
            if go.open {
                go.record_added_window(id.clone());
                if stop_move_animations {
                    self.scrolling.stop_move_animations();
                }
            }
        }

        self.recompute_grid_overview_layout(true);
        self.sync_grid_focus_to_active_window();
    }

    pub fn grid_window_was_added_while_open(&self, id: &W::Id) -> bool {
        self.grid_overview
            .as_ref()
            .is_some_and(|go| go.open && go.window_was_added_while_open(id))
    }

    fn recompute_grid_overview_layout(&mut self, restart_rearrange: bool) {
        let items = self.grid_overview_items();
        let working_area = self.working_area;

        let mut go = match self.grid_overview.take() {
            Some(go) => go,
            None => return,
        };

        if !go.open {
            self.grid_overview = Some(go);
            return;
        }
        go.compute_layout(&items, working_area, restart_rearrange);

        // Sync column_tile_focus with new layout.
        let valid_col_indices: Vec<_> = go
            .layout
            .entries
            .iter()
            .filter_map(|(item, _)| {
                if let GridItem::Column { col_idx, .. } = item {
                    Some(*col_idx)
                } else {
                    None
                }
            })
            .collect();
        go.column_tile_focus
            .retain(|(col_idx, _)| valid_col_indices.contains(col_idx));
        for col_idx in valid_col_indices {
            let tile_count = self.scrolling.column_tile_count(col_idx);
            if let Some(entry) = go.column_tile_focus.iter_mut().find(|(c, _)| *c == col_idx) {
                if entry.1 >= tile_count {
                    entry.1 = self.scrolling.column_active_tile_idx(col_idx);
                }
            } else {
                let active_tile = self.scrolling.column_active_tile_idx(col_idx);
                go.column_tile_focus.push((col_idx, active_tile));
            }
        }

        self.grid_overview = Some(go);
    }
    pub fn fix_floating_state_for_active(&mut self) {
        if let Some(id) = self.grid_focused_window_id() {
            if self.floating.has_window(&id) {
                self.floating_is_active = FloatingActive::Yes;
            } else {
                self.floating_is_active = FloatingActive::No;
            }
        }
    }

    pub fn advance_grid_overview_animations(&mut self) {
        let mut go = match self.grid_overview.take() {
            Some(go) => go,
            None => return,
        };

        go.advance_animations();
        if go.progress.is_none() && !go.open {
            return;
        }

        self.grid_overview = Some(go);
    }

    pub fn are_grid_overview_animations_ongoing(&self) -> bool {
        self.grid_overview
            .as_ref()
            .map_or(false, |g| g.are_animations_ongoing())
    }

    fn grid_overview_items(&self) -> Vec<(GridItem<W>, Size<f64, Logical>)> {
        let mut items = self.scrolling.grid_overview_items();
        items.extend(
            self.floating
                .tiles()
                .filter(|tile| !Self::tile_ignores_grid_overview(tile))
                .map(|tile| {
                    (
                        GridItem::Floating {
                            window_id: tile.window().id().clone(),
                        },
                        tile.tile_size(),
                    )
                }),
        );
        items
    }

    fn grid_item_for_window(&self, id: &W::Id) -> Option<GridItem<W>> {
        if let Some(tile) = self.floating.tiles().find(|tile| tile.window().id() == id) {
            if Self::tile_ignores_grid_overview(tile) {
                return None;
            }

            return Some(GridItem::Floating {
                window_id: id.clone(),
            });
        }

        self.scrolling.grid_item_for_window(id)
    }

    pub fn window_is_in_grid_overview(&self, id: &W::Id) -> bool {
        self.is_grid_overview_open() && self.grid_item_for_window(id).is_some()
    }

    pub fn active_ignored_floating_window_in_grid(&self) -> Option<&W> {
        if !self.is_grid_overview_open() {
            return None;
        }

        let active = self.active_window()?;
        (self.floating.has_window(active.id()) && self.grid_item_for_window(active.id()).is_none())
            .then_some(active)
    }

    fn tile_ignores_grid_overview(tile: &Tile<W>) -> bool {
        tile.window().rules().ignore_grid_overview == Some(true)
    }

    fn grid_item_normal_render_pos(
        &self,
        item: &GridItem<W>,
        use_target_view_pos: bool,
    ) -> Option<Point<f64, Logical>> {
        match item {
            GridItem::Column { .. } | GridItem::Tab { .. } => {
                let preview = if use_target_view_pos {
                    self.scrolling.grid_preview_at_target(item)
                } else {
                    self.scrolling.grid_preview(item)
                };
                preview.map(|preview| preview.normal_pos)
            }
            GridItem::Floating { window_id } => self
                .floating
                .tiles_with_render_positions()
                .find_map(|(tile, pos)| (tile.window().id() == window_id).then_some(pos)),
        }
    }

    fn grid_item_visible_when_closing(&self, item: &GridItem<W>) -> bool {
        match item {
            GridItem::Column { .. } | GridItem::Tab { .. } => {
                self.scrolling.grid_item_visible_when_closing(item)
            }
            GridItem::Floating { .. } => true,
        }
    }

    fn grid_item_renders_on_top_when_grid_closing(&self, item: &GridItem<W>) -> bool {
        match item {
            GridItem::Column { col_idx, .. } => self
                .scrolling
                .columns()
                .nth(*col_idx)
                .is_some_and(|col| col.pending_sizing_mode().is_fullscreen()),
            GridItem::Tab {
                col_idx, window_id, ..
            } => {
                let Some(col) = self.scrolling.columns().nth(*col_idx) else {
                    return false;
                };
                if !col.pending_sizing_mode().is_fullscreen() {
                    return false;
                }

                let active_idx = self.scrolling.column_active_tile_idx(*col_idx);
                col.tiles()
                    .nth(active_idx)
                    .is_some_and(|(tile, _)| tile.window().id() == window_id)
            }
            GridItem::Floating { .. } => false,
        }
    }

    #[cfg(test)]
    pub fn grid_item_renders_on_top_when_grid_closing_for_tests(&self, item: &GridItem<W>) -> bool {
        self.grid_item_renders_on_top_when_grid_closing(item)
    }

    fn grid_item_visual_transform(
        &self,
        go: &GridOverview<W>,
        item: &GridItem<W>,
        info: &GridEntryInfo,
    ) -> (Point<f64, Logical>, f64) {
        let fallback_pos = self
            .grid_item_normal_render_pos(item, !go.open)
            .unwrap_or(info.target_pos);
        go.entry_visual_transform(item, info, fallback_pos)
    }

    pub(super) fn grid_window_visual_transform(
        &self,
        window: &W::Id,
    ) -> Option<(Point<f64, Logical>, f64)> {
        let go = self.grid_overview.as_ref().filter(|go| go.open)?;
        let (target_pos, target_scale) = self.grid_window_target_visual_transform(window)?;
        Some(go.window_visual_transform(window, target_pos, target_scale))
    }

    fn grid_window_target_visual_transform(
        &self,
        window: &W::Id,
    ) -> Option<(Point<f64, Logical>, f64)> {
        let go = self.grid_overview.as_ref().filter(|go| go.open)?;
        let item = self.grid_item_for_window(window)?;
        let (_, info) = go
            .layout
            .entries
            .iter()
            .find(|(entry, _)| entry.matches_animation_key(&item))?;
        let (visual_pos, visual_scale) = self.grid_item_visual_transform(go, &item, info);

        match item {
            GridItem::Column { .. } | GridItem::Tab { .. } => {
                let preview = self.scrolling.grid_preview_with_stable_origin(&item)?;
                let preview_tile = preview
                    .tiles
                    .into_iter()
                    .find(|tile| tile.tile.window().id() == window)?;
                let target_pos = visual_pos + preview_tile.pos.upscale(visual_scale);
                Some((target_pos, visual_scale))
            }
            GridItem::Floating { .. } => Some((visual_pos, visual_scale)),
        }
    }

    pub fn set_grid_focus_for_window(&mut self, id: &W::Id) -> bool {
        self.set_grid_focus_for_window_impl(id, true)
    }

    fn set_grid_focus_for_window_without_animation(&mut self, id: &W::Id) -> bool {
        self.set_grid_focus_for_window_impl(id, false)
    }

    fn set_grid_focus_for_window_impl(&mut self, id: &W::Id, animate: bool) -> bool {
        let Some(item) = self.grid_item_for_window(id) else {
            return false;
        };

        let column_tile_focus = if let GridItem::Column { col_idx, .. } = &item {
            self.scrolling
                .columns()
                .nth(*col_idx)
                .and_then(|col| col.position(id).map(|tile_idx| (*col_idx, tile_idx)))
        } else {
            None
        };

        let Some(go) = self.grid_overview.as_mut() else {
            return false;
        };

        let Some((row, col)) = go.find_grid_index_for_item(&item) else {
            return false;
        };

        if animate {
            go.set_focus((row, col));
        } else {
            go.set_focus_without_animation((row, col));
        }
        if let Some((col_idx, tile_idx)) = column_tile_focus {
            go.set_column_tile_focus(col_idx, tile_idx, id.clone());
        } else {
            go.set_focused_window_id(id.clone());
        }
        true
    }

    fn sync_grid_focus_to_active_window(&mut self) {
        let Some(id) = self.active_window().map(|w| w.id().clone()) else {
            return;
        };

        self.set_grid_focus_for_window(&id);
    }

    pub fn update_render_elements(&mut self, is_active: bool) {
        // Clear lingering focus-ring width overrides.
        for tile in self.scrolling.tiles_mut() {
            tile.focus_ring_mut().clear_width_override();
        }
        for tile in self.floating.tiles_mut() {
            tile.focus_ring_mut().clear_width_override();
        }

        self.scrolling
            .update_render_elements(is_active && !self.floating_is_active.get());

        let view_rect = Rectangle::from_size(self.view_size);
        self.floating
            .update_render_elements(is_active && self.floating_is_active.get(), view_rect);

        let grid_focused_window_id = self.grid_focused_window_id();
        if let Some(ref go) = self.grid_overview {
            if go.open || go.progress.is_some() {
                let is_closing = !go.open;

                for (item, info) in go.layout.entries.iter() {
                    if is_closing && !self.grid_item_visible_when_closing(item) {
                        continue;
                    }

                    let is_grid_focused = info.row == go.focus.0 && info.col == go.focus.1;
                    let focused_window = is_grid_focused
                        .then_some(())
                        .and_then(|_| grid_focused_window_id.as_ref());

                    if is_grid_focused {
                        let target_scale = info.target_scale;
                        if let Some(window_id) = focused_window {
                            for tile in self.scrolling.tiles_mut().chain(self.floating.tiles_mut())
                            {
                                if tile.window().id() == window_id {
                                    let normal_width = tile.focus_ring().config().width;
                                    if normal_width > 0. {
                                        let max_width = normal_width * 2.;
                                        let grid_width = (normal_width
                                            / target_scale.max(0.0001).sqrt())
                                        .clamp(normal_width, max_width);
                                        tile.focus_ring_mut().set_width_override(grid_width);
                                    }
                                    break;
                                }
                            }
                        }
                    }

                    self.scrolling.update_grid_item_render_elements(
                        item,
                        focused_window,
                        view_rect,
                    );
                    for tile in self.floating.tiles_mut() {
                        if tile.window().id() == item.window_id() {
                            tile.update_render_elements(is_grid_focused, view_rect);
                            break;
                        }
                    }
                }
            }
        }

        self.shadow.update_render_elements(
            self.view_size,
            true,
            CornerRadius::default(),
            self.scale.fractional_scale(),
            1.,
        );
    }

    pub fn update_config(&mut self, base_options: Rc<Options>) {
        let scale = self.scale.fractional_scale();
        let options = Rc::new(
            Options::clone(&base_options)
                .with_merged_layout(self.layout_config.as_ref())
                .adjusted_for_scale(scale),
        );

        self.scrolling.update_config(
            self.view_size,
            self.working_area,
            self.scale.fractional_scale(),
            options.clone(),
        );

        self.floating.update_config(
            self.view_size,
            self.working_area,
            self.scale.fractional_scale(),
            options.clone(),
        );

        let shadow_config =
            compute_workspace_shadow_config(options.overview.workspace_shadow, self.view_size);
        self.shadow.update_config(shadow_config);

        self.background_buffer
            .set_color(options.layout.background_color);

        self.base_options = base_options;
        self.options = options;
    }

    pub fn update_layout_config(&mut self, layout_config: Option<niri_config::LayoutPart>) {
        if self.layout_config == layout_config {
            return;
        }

        self.layout_config = layout_config;
        self.update_config(self.base_options.clone());
    }

    pub fn update_shaders(&mut self) {
        self.scrolling.update_shaders();
        self.floating.update_shaders();
        self.shadow.update_shaders();
    }

    pub fn windows(&self) -> impl Iterator<Item = &W> + '_ {
        self.tiles().map(Tile::window)
    }

    pub fn windows_mut(&mut self) -> impl Iterator<Item = &mut W> + '_ {
        self.tiles_mut().map(Tile::window_mut)
    }

    pub fn tiles(&self) -> impl Iterator<Item = &Tile<W>> + '_ {
        let scrolling = self.scrolling.tiles();
        let floating = self.floating.tiles();
        scrolling.chain(floating)
    }

    pub fn tiles_mut(&mut self) -> impl Iterator<Item = &mut Tile<W>> + '_ {
        let scrolling = self.scrolling.tiles_mut();
        let floating = self.floating.tiles_mut();
        scrolling.chain(floating)
    }

    pub fn is_floating(&self, id: &W::Id) -> bool {
        self.floating.has_window(id)
    }

    pub fn current_output(&self) -> Option<&Output> {
        self.output.as_ref()
    }

    pub fn active_window(&self) -> Option<&W> {
        if self.floating_is_active.get() {
            self.floating.active_window()
        } else {
            self.scrolling.active_window()
        }
    }

    pub fn active_window_mut(&mut self) -> Option<&mut W> {
        if self.floating_is_active.get() {
            self.floating.active_window_mut()
        } else {
            self.scrolling.active_window_mut()
        }
    }

    pub fn is_active_pending_fullscreen(&self) -> bool {
        self.scrolling.is_active_pending_fullscreen()
    }

    pub fn set_output(&mut self, output: Option<Output>) {
        if self.output == output {
            return;
        }

        if let Some(output) = self.output.take() {
            for win in self.windows() {
                win.output_leave(&output);
            }
        }

        self.output = output;

        if let Some(output) = &self.output {
            // Normalize original output: possibly replace connector with make/model/serial.
            if self.original_output.matches(output) {
                self.original_output = OutputId::new(output);
            }

            self.update_output_size();

            for win in self.windows() {
                self.enter_output_for_window(win);
            }
        }
    }

    fn enter_output_for_window(&self, window: &W) {
        if let Some(output) = &self.output {
            window.set_preferred_scale_transform(self.scale, self.transform);
            window.output_enter(output);
        }
    }

    pub fn update_output_size(&mut self) {
        let output = self.output.as_ref().unwrap();
        let scale = output.current_scale();
        let transform = output.current_transform();
        let view_size = output_size(output);
        let working_area = compute_working_area(output);
        self.set_view_size(scale, transform, view_size, working_area);
    }

    fn set_view_size(
        &mut self,
        scale: smithay::output::Scale,
        transform: Transform,
        size: Size<f64, Logical>,
        working_area: Rectangle<f64, Logical>,
    ) {
        let scale_transform_changed = self.transform != transform
            || self.scale.integer_scale() != scale.integer_scale()
            || self.scale.fractional_scale() != scale.fractional_scale();
        if !scale_transform_changed && self.view_size == size && self.working_area == working_area {
            return;
        }

        let fractional_scale_changed = self.scale.fractional_scale() != scale.fractional_scale();

        self.scale = scale;
        self.transform = transform;
        self.view_size = size;
        self.working_area = working_area;

        if fractional_scale_changed {
            // Options need to be recomputed for the new scale.
            self.update_config(self.base_options.clone());
        } else {
            // Pass our existing options as is.
            self.scrolling.update_config(
                size,
                working_area,
                scale.fractional_scale(),
                self.options.clone(),
            );
            self.floating.update_config(
                size,
                working_area,
                scale.fractional_scale(),
                self.options.clone(),
            );

            let shadow_config =
                compute_workspace_shadow_config(self.options.overview.workspace_shadow, size);
            self.shadow.update_config(shadow_config);
        }

        self.background_buffer.resize(size);

        if scale_transform_changed {
            for window in self.windows() {
                window.set_preferred_scale_transform(self.scale, self.transform);
            }
        }
    }

    pub fn view_size(&self) -> Size<f64, Logical> {
        self.view_size
    }

    pub fn make_tile(&self, window: W) -> Tile<W> {
        Tile::new(
            window,
            self.view_size,
            self.scale.fractional_scale(),
            self.clock.clone(),
            self.options.clone(),
        )
    }

    pub fn add_tile(
        &mut self,
        mut tile: Tile<W>,
        target: WorkspaceAddWindowTarget<W>,
        activate: ActivateWindow,
        width: ColumnWidth,
        is_full_width: bool,
        is_floating: bool,
    ) {
        self.enter_output_for_window(tile.window());
        tile.restore_to_floating = is_floating;

        match target {
            WorkspaceAddWindowTarget::Auto => {
                // Don't steal focus from an active fullscreen window.
                let activate = activate.map_smart(|| !self.is_active_pending_fullscreen());

                // If the tile is pending maximized or fullscreen, open it in the scrolling layout
                // where it can do that.
                if is_floating && tile.window().pending_sizing_mode().is_normal() {
                    self.floating.add_tile(tile, activate);

                    if activate || self.scrolling.is_empty() {
                        self.floating_is_active = FloatingActive::Yes;
                    }
                } else {
                    let grid_col_idx = self.grid_overview.as_ref().and_then(|go| {
                        if !go.open {
                            return None;
                        }
                        let idx = go.focus.0 * go.layout.cols + go.focus.1;
                        let (item, _) = go.layout.entries.get(idx)?;
                        match item {
                            GridItem::Column { col_idx, .. } => Some(*col_idx),
                            GridItem::Tab { col_idx, .. } => Some(*col_idx),
                            GridItem::Floating { .. } => None,
                        }
                    });

                    self.scrolling.add_tile(
                        grid_col_idx.map(|c| c + 1),
                        tile,
                        activate,
                        width,
                        is_full_width,
                        None,
                    );

                    if activate {
                        self.floating_is_active = FloatingActive::No;
                    }
                }
            }
            WorkspaceAddWindowTarget::NewColumnAt(col_idx) => {
                let activate = activate.map_smart(|| false);
                self.scrolling
                    .add_tile(Some(col_idx), tile, activate, width, is_full_width, None);

                if activate {
                    self.floating_is_active = FloatingActive::No;
                }
            }
            WorkspaceAddWindowTarget::NextTo(next_to) => {
                let activate = activate.map_smart(|| self.active_window().unwrap().id() == next_to);

                let floating_has_window = self.floating.has_window(next_to);

                if is_floating && tile.window().pending_sizing_mode().is_normal() {
                    if floating_has_window {
                        self.floating.add_tile_above(next_to, tile, activate);
                    } else {
                        // FIXME: use static pos
                        let (next_to_tile, render_pos, _visible) = self
                            .scrolling
                            .tiles_with_render_positions()
                            .find(|(tile, _, _)| tile.window().id() == next_to)
                            .unwrap();

                        // Position the new tile in the center above the next_to tile. Think a
                        // dialog opening on top of a window.
                        let tile_size = tile.tile_size();
                        let pos = render_pos
                            + (next_to_tile.tile_size().to_point() - tile_size.to_point())
                                .downscale(2.);
                        let pos = self.floating.clamp_within_working_area(pos, tile_size);
                        let pos = self.floating.logical_to_size_frac(pos);
                        tile.floating_pos = Some(pos);

                        self.floating.add_tile(tile, activate);
                    }

                    if activate || self.scrolling.is_empty() {
                        self.floating_is_active = FloatingActive::Yes;
                    }
                } else if floating_has_window {
                    self.scrolling
                        .add_tile(None, tile, activate, width, is_full_width, None);

                    if activate {
                        self.floating_is_active = FloatingActive::No;
                    }
                } else {
                    self.scrolling
                        .add_tile_right_of(next_to, tile, activate, width, is_full_width);

                    if activate {
                        self.floating_is_active = FloatingActive::No;
                    }
                }
            }
        }
    }

    pub fn add_tile_to_column(
        &mut self,
        col_idx: usize,
        tile_idx: Option<usize>,
        tile: Tile<W>,
        activate: bool,
    ) {
        self.enter_output_for_window(tile.window());
        self.scrolling
            .add_tile_to_column(col_idx, tile_idx, tile, activate);

        if activate {
            self.floating_is_active = FloatingActive::No;
        }
    }

    pub fn add_column(&mut self, column: Column<W>, activate: bool) {
        for (tile, _) in column.tiles() {
            self.enter_output_for_window(tile.window());
        }

        self.scrolling.add_column(None, column, activate, None);

        if activate {
            self.floating_is_active = FloatingActive::No;
        }
    }

    fn update_focus_floating_tiling_after_removing(&mut self, removed_from_floating: bool) {
        if removed_from_floating {
            if self.floating.is_empty() {
                self.floating_is_active = FloatingActive::No;
            }
        } else {
            // Scrolling should remain focused if both are empty.
            if self.scrolling.is_empty() && !self.floating.is_empty() {
                self.floating_is_active = FloatingActive::Yes;
            }
        }
    }

    pub fn remove_tile(&mut self, id: &W::Id, transaction: Transaction) -> RemovedTile<W> {
        let mut from_floating = false;
        let removed = if self.floating.has_window(id) {
            from_floating = true;
            self.floating.remove_tile(id)
        } else {
            self.scrolling.remove_tile(id, transaction)
        };

        if let Some(output) = &self.output {
            removed.tile.window().output_leave(output);
        }

        self.update_focus_floating_tiling_after_removing(from_floating);

        removed
    }

    pub fn remove_active_tile(&mut self, transaction: Transaction) -> Option<RemovedTile<W>> {
        let from_floating = self.floating_is_active.get();
        let removed = if from_floating {
            self.floating.remove_active_tile()?
        } else {
            self.scrolling.remove_active_tile(transaction)?
        };

        if let Some(output) = &self.output {
            removed.tile.window().output_leave(output);
        }

        self.update_focus_floating_tiling_after_removing(from_floating);

        Some(removed)
    }

    pub fn remove_active_column(&mut self) -> Option<Column<W>> {
        let from_floating = self.floating_is_active.get();
        if from_floating {
            return None;
        }

        let column = self.scrolling.remove_active_column()?;

        if let Some(output) = &self.output {
            for (tile, _) in column.tiles() {
                tile.window().output_leave(output);
            }
        }

        self.update_focus_floating_tiling_after_removing(from_floating);

        Some(column)
    }

    pub fn resolve_default_width(
        &self,
        default_width: Option<Option<PresetSize>>,
        is_floating: bool,
    ) -> Option<PresetSize> {
        match default_width {
            Some(Some(width)) => Some(width),
            Some(None) => None,
            None if is_floating => None,
            None => self.options.layout.default_column_width,
        }
    }

    pub fn resolve_default_height(
        &self,
        default_height: Option<Option<PresetSize>>,
        is_floating: bool,
    ) -> Option<PresetSize> {
        match default_height {
            Some(Some(height)) => Some(height),
            Some(None) => None,
            None if is_floating => None,
            // We don't have a global default at the moment.
            None => None,
        }
    }

    pub fn new_window_size(
        &self,
        width: Option<PresetSize>,
        height: Option<PresetSize>,
        is_floating: bool,
        rules: &ResolvedWindowRules,
        (min_size, max_size): (Size<i32, Logical>, Size<i32, Logical>),
    ) -> Size<i32, Logical> {
        let mut size = if is_floating {
            self.floating.new_window_size(width, height, rules)
        } else {
            self.scrolling.new_window_size(width, height, rules)
        };

        // If the window has a fixed size, or we're picking some fixed size, apply min and max
        // size. This is to ensure that a fixed-size window rule works on open, while still
        // allowing the window freedom to pick its default size otherwise.
        let (min_size, max_size) = rules.apply_min_max_size(min_size, max_size);
        size.w = ensure_min_max_size_maybe_zero(size.w, min_size.w, max_size.w);
        // For scrolling (where height is > 0) only ensure fixed height, since at runtime scrolling
        // will only honor fixed height currently.
        if min_size.h == max_size.h {
            size.h = ensure_min_max_size(size.h, min_size.h, max_size.h);
        } else if size.h > 0 {
            // Also always honor min height, scrolling always does.
            size.h = max(size.h, min_size.h);
        }

        size
    }

    pub fn configure_new_window(
        &self,
        window: &Window,
        width: Option<PresetSize>,
        height: Option<PresetSize>,
        is_floating: bool,
        rules: &ResolvedWindowRules,
    ) {
        window.with_surfaces(|surface, data| {
            send_scale_transform(surface, data, self.scale, self.transform);
        });

        let toplevel = window.toplevel().expect("no x11 support");
        let (min_size, max_size) = with_states(toplevel.wl_surface(), |state| {
            let mut guard = state.cached_state.get::<SurfaceCachedState>();
            let current = guard.current();
            (current.min_size, current.max_size)
        });
        toplevel.with_pending_state(|state| {
            if state.states.contains(xdg_toplevel::State::Fullscreen) {
                state.size = Some(self.view_size.to_i32_round());
            } else if state.states.contains(xdg_toplevel::State::Maximized) {
                state.size = Some(self.working_area.size.to_i32_round());
            } else {
                let size =
                    self.new_window_size(width, height, is_floating, rules, (min_size, max_size));
                state.size = Some(size);
            }

            if is_floating {
                state.bounds = Some(self.floating.new_window_toplevel_bounds(rules));
            } else {
                state.bounds = Some(self.scrolling.new_window_toplevel_bounds(rules));
            }
        });
    }

    pub(super) fn resolve_scrolling_width(
        &self,
        window: &W,
        width: Option<PresetSize>,
    ) -> ColumnWidth {
        let width = width.unwrap_or_else(|| PresetSize::Fixed(window.size().w));
        match width {
            PresetSize::Fixed(fixed) => {
                let mut fixed = f64::from(fixed);

                // Add border width since ColumnWidth includes borders.
                let rules = window.rules();
                let border = self.options.layout.border.merged_with(&rules.border);
                if !border.off {
                    fixed += border.width * 2.;
                }

                ColumnWidth::Fixed(fixed)
            }
            PresetSize::Proportion(prop) => ColumnWidth::Proportion(prop),
        }
    }

    pub fn focus_left(&mut self) -> bool {
        if self.floating_is_active.get() {
            self.floating.focus_left()
        } else {
            self.scrolling.focus_left()
        }
    }

    pub fn focus_right(&mut self) -> bool {
        if self.floating_is_active.get() {
            self.floating.focus_right()
        } else {
            self.scrolling.focus_right()
        }
    }

    pub fn focus_column_first(&mut self) {
        if self.floating_is_active.get() {
            self.floating.focus_leftmost();
        } else {
            self.scrolling.focus_column_first();
        }
    }

    pub fn focus_column_last(&mut self) {
        if self.floating_is_active.get() {
            self.floating.focus_rightmost();
        } else {
            self.scrolling.focus_column_last();
        }
    }

    pub fn focus_column_right_or_first(&mut self) {
        if !self.focus_right() {
            self.focus_column_first();
        }
    }

    pub fn focus_column_left_or_last(&mut self) {
        if !self.focus_left() {
            self.focus_column_last();
        }
    }

    pub fn focus_column(&mut self, index: usize) {
        if self.floating_is_active.get() {
            self.focus_tiling();
        }
        self.scrolling.focus_column(index);
    }

    pub fn focus_window_in_column(&mut self, index: u8) {
        if self.floating_is_active.get() {
            return;
        }
        self.scrolling.focus_window_in_column(index);
    }

    pub fn focus_down(&mut self) -> bool {
        if self.floating_is_active.get() {
            self.floating.focus_down()
        } else {
            self.scrolling.focus_down()
        }
    }

    pub fn focus_up(&mut self) -> bool {
        if self.floating_is_active.get() {
            self.floating.focus_up()
        } else {
            self.scrolling.focus_up()
        }
    }

    pub fn focus_down_or_left(&mut self) {
        if self.floating_is_active.get() {
            self.floating.focus_down();
        } else {
            self.scrolling.focus_down_or_left();
        }
    }

    pub fn focus_down_or_right(&mut self) {
        if self.floating_is_active.get() {
            self.floating.focus_down();
        } else {
            self.scrolling.focus_down_or_right();
        }
    }

    pub fn focus_up_or_left(&mut self) {
        if self.floating_is_active.get() {
            self.floating.focus_up();
        } else {
            self.scrolling.focus_up_or_left();
        }
    }

    pub fn focus_up_or_right(&mut self) {
        if self.floating_is_active.get() {
            self.floating.focus_up();
        } else {
            self.scrolling.focus_up_or_right();
        }
    }

    pub fn focus_window_top(&mut self) {
        if self.floating_is_active.get() {
            self.floating.focus_topmost();
        } else {
            self.scrolling.focus_top();
        }
    }

    pub fn focus_window_bottom(&mut self) {
        if self.floating_is_active.get() {
            self.floating.focus_bottommost();
        } else {
            self.scrolling.focus_bottom();
        }
    }

    pub fn focus_window_down_or_top(&mut self) {
        if !self.focus_down() {
            self.focus_window_top();
        }
    }

    pub fn focus_window_up_or_bottom(&mut self) {
        if !self.focus_up() {
            self.focus_window_bottom();
        }
    }

    pub fn move_left(&mut self) -> bool {
        if self.floating_is_active.get() {
            self.floating.move_left();
            true
        } else {
            self.scrolling.move_left()
        }
    }

    pub fn move_right(&mut self) -> bool {
        if self.floating_is_active.get() {
            self.floating.move_right();
            true
        } else {
            self.scrolling.move_right()
        }
    }

    pub fn move_column_to_first(&mut self) {
        if self.floating_is_active.get() {
            return;
        }
        self.scrolling.move_column_to_first();
    }

    pub fn move_column_to_last(&mut self) {
        if self.floating_is_active.get() {
            return;
        }
        self.scrolling.move_column_to_last();
    }

    pub fn move_column_to_index(&mut self, index: usize) {
        if self.floating_is_active.get() {
            return;
        }
        self.scrolling.move_column_to_index(index);
    }

    pub fn move_down(&mut self) -> bool {
        if self.floating_is_active.get() {
            self.floating.move_down();
            true
        } else {
            self.scrolling.move_down()
        }
    }

    pub fn move_up(&mut self) -> bool {
        if self.floating_is_active.get() {
            self.floating.move_up();
            true
        } else {
            self.scrolling.move_up()
        }
    }

    pub fn consume_or_expel_window_left(&mut self, window: Option<&W::Id>) {
        if window.map_or(self.floating_is_active.get(), |id| {
            self.floating.has_window(id)
        }) {
            return;
        }
        self.scrolling.consume_or_expel_window_left(window);
    }

    pub fn consume_or_expel_window_right(&mut self, window: Option<&W::Id>) {
        if window.map_or(self.floating_is_active.get(), |id| {
            self.floating.has_window(id)
        }) {
            return;
        }
        self.scrolling.consume_or_expel_window_right(window);
    }

    pub fn consume_into_column(&mut self) {
        if self.floating_is_active.get() {
            return;
        }
        self.scrolling.consume_into_column();
    }

    pub fn expel_from_column(&mut self) {
        if self.floating_is_active.get() {
            return;
        }
        self.scrolling.expel_from_column();
    }

    pub fn swap_window_in_direction(&mut self, direction: ScrollDirection) {
        if self.floating_is_active.get() {
            return;
        }
        self.scrolling.swap_window_in_direction(direction);
    }

    pub fn toggle_column_tabbed_display(&mut self) {
        if self.floating_is_active.get() {
            return;
        }
        self.scrolling.toggle_column_tabbed_display();
    }

    pub fn set_column_display(&mut self, display: ColumnDisplay) {
        if self.floating_is_active.get() {
            return;
        }
        self.scrolling.set_column_display(display);
    }

    pub fn center_column(&mut self) {
        if self.floating_is_active.get() {
            self.floating.center_window(None);
        } else {
            self.scrolling.center_column();
        }
    }

    pub fn center_window(&mut self, id: Option<&W::Id>) {
        if id.map_or(self.floating_is_active.get(), |id| {
            self.floating.has_window(id)
        }) {
            self.floating.center_window(id);
        } else {
            self.scrolling.center_window(id);
        }
    }

    pub fn center_visible_columns(&mut self) {
        if self.floating_is_active.get() {
            return;
        }
        self.scrolling.center_visible_columns();
    }

    fn implicit_grid_window(&self, window: Option<&W::Id>) -> Option<W::Id> {
        if window.is_none() && self.is_grid_overview_open() {
            self.grid_focused_window_id()
        } else {
            None
        }
    }

    pub fn toggle_width(&mut self, forwards: bool) {
        if self.is_grid_overview_open() {
            if let Some(id) = self.grid_focused_window_id() {
                if self.floating.has_window(&id) {
                    self.floating.toggle_window_width(Some(&id), forwards);
                } else {
                    self.scrolling.toggle_window_width(Some(&id), forwards);
                }
                return;
            }
        }

        if self.floating_is_active.get() {
            self.floating.toggle_window_width(None, forwards);
        } else {
            self.scrolling.toggle_width(forwards);
        }
    }

    pub fn toggle_full_width(&mut self) {
        if self.is_grid_overview_open() {
            if let Some(id) = self.grid_focused_window_id() {
                if !self.floating.has_window(&id) {
                    self.scrolling.toggle_full_width_for_window(&id);
                }
                return;
            }
        }

        if self.floating_is_active.get() {
            // Leave this unimplemented for now. For good UX, this probably needs moving the tile
            // to be against the left edge of the working area while it is full-width.
            return;
        }
        self.scrolling.toggle_full_width();
    }

    pub fn set_column_width(&mut self, change: SizeChange) {
        if self.is_grid_overview_open() {
            if let Some(id) = self.grid_focused_window_id() {
                if self.floating.has_window(&id) {
                    self.floating.set_window_width(Some(&id), change, true);
                } else {
                    self.scrolling.set_window_width(Some(&id), change);
                }
                return;
            }
        }
        if self.floating_is_active.get() {
            self.floating.set_window_width(None, change, true);
        } else {
            self.scrolling.set_window_width(None, change);
        }
    }

    pub fn set_window_width(&mut self, window: Option<&W::Id>, change: SizeChange) {
        let grid_window = self.implicit_grid_window(window);
        let window = window.or(grid_window.as_ref());

        if window.map_or(self.floating_is_active.get(), |id| {
            self.floating.has_window(id)
        }) {
            self.floating.set_window_width(window, change, true);
        } else {
            self.scrolling.set_window_width(window, change);
        }
    }

    pub fn set_window_height(&mut self, window: Option<&W::Id>, change: SizeChange) {
        let grid_window = self.implicit_grid_window(window);
        let window = window.or(grid_window.as_ref());

        if window.map_or(self.floating_is_active.get(), |id| {
            self.floating.has_window(id)
        }) {
            self.floating.set_window_height(window, change, true);
        } else {
            self.scrolling.set_window_height(window, change);
        }
    }

    pub fn reset_window_height(&mut self, window: Option<&W::Id>) {
        let grid_window = self.implicit_grid_window(window);
        let window = window.or(grid_window.as_ref());

        if window.map_or(self.floating_is_active.get(), |id| {
            self.floating.has_window(id)
        }) {
            return;
        }
        self.scrolling.reset_window_height(window);
    }

    pub fn toggle_window_width(&mut self, window: Option<&W::Id>, forwards: bool) {
        let grid_window = self.implicit_grid_window(window);
        let window = window.or(grid_window.as_ref());

        if window.map_or(self.floating_is_active.get(), |id| {
            self.floating.has_window(id)
        }) {
            self.floating.toggle_window_width(window, forwards);
        } else {
            self.scrolling.toggle_window_width(window, forwards);
        }
    }

    pub fn toggle_window_height(&mut self, window: Option<&W::Id>, forwards: bool) {
        let grid_window = self.implicit_grid_window(window);
        let window = window.or(grid_window.as_ref());

        if window.map_or(self.floating_is_active.get(), |id| {
            self.floating.has_window(id)
        }) {
            self.floating.toggle_window_height(window, forwards);
        } else {
            self.scrolling.toggle_window_height(window, forwards);
        }
    }

    pub fn expand_column_to_available_width(&mut self) {
        if self.floating_is_active.get() {
            return;
        }
        self.scrolling.expand_column_to_available_width();
    }

    pub fn set_fullscreen(&mut self, window: &W::Id, is_fullscreen: bool) {
        let mut restore_to_floating = false;
        if self.floating.has_window(window) {
            if is_fullscreen {
                restore_to_floating = true;
                self.toggle_window_floating(Some(window));
            } else {
                // Floating windows are never fullscreen, so this is an unfullscreen request for an
                // already unfullscreen window.
                return;
            }
        } else if !is_fullscreen {
            // The window is in the scrolling layout and we're requesting an unfullscreen. If it is
            // indeed fullscreen (i.e. this isn't a duplicate unfullscreen request), then we may
            // need to unfullscreen into floating.
            let col = self
                .scrolling
                .columns()
                .find(|col| col.contains(window))
                .unwrap();

            // When going from fullscreen to maximized, don't consider restore_to_floating yet.
            if col.is_pending_fullscreen() && !col.is_pending_maximized() {
                let (tile, _) = col
                    .tiles()
                    .find(|(tile, _)| tile.window().id() == window)
                    .unwrap();
                if tile.restore_to_floating {
                    // Unfullscreen and float in one call so it has a chance to notice and request a
                    // (0, 0) size, rather than the scrolling column size.
                    self.toggle_window_floating(Some(window));
                    return;
                }
            }
        }

        let tile = self
            .scrolling
            .tiles()
            .find(|tile| tile.window().id() == window)
            .unwrap();
        let was_normal = tile.window().pending_sizing_mode().is_normal();

        self.scrolling.set_fullscreen(window, is_fullscreen);

        // When going from normal to fullscreen, remember if we should unfullscreen to floating.
        let tile = self
            .scrolling
            .tiles_mut()
            .find(|tile| tile.window().id() == window)
            .unwrap();
        if was_normal && !tile.window().pending_sizing_mode().is_normal() {
            tile.restore_to_floating = restore_to_floating;
        }
    }

    pub fn toggle_fullscreen(&mut self, window: &W::Id) {
        let tile = self
            .tiles()
            .find(|tile| tile.window().id() == window)
            .unwrap();
        let current = tile.window().pending_sizing_mode().is_fullscreen();
        self.set_fullscreen(window, !current);
    }

    pub fn set_maximized(&mut self, window: &W::Id, maximize: bool) {
        let mut restore_to_floating = false;
        if self.floating.has_window(window) {
            if maximize {
                restore_to_floating = true;
                self.toggle_window_floating(Some(window));
            } else {
                // Floating windows are never maximized, so this is an unmaximize request for an
                // already unmaximized window.
                return;
            }
        } else if !maximize {
            // The window is in the scrolling layout and we're requesting to unmaximize. If it is
            // indeed maximized (i.e. this isn't a duplicate unmaximize request), then we may
            // need to unmaximize into floating.
            let tile = self
                .scrolling
                .tiles()
                .find(|tile| tile.window().id() == window)
                .unwrap();
            // The tile cannot unmaximize into fullscreen (pending_sizing_mode() will be fullscreen
            // in that case and not maximized), so this check works.
            if tile.window().pending_sizing_mode().is_maximized() && tile.restore_to_floating {
                // Unmaximize and float in one call so it has a chance to notice and request a
                // (0, 0) size, rather than the scrolling column size.
                self.toggle_window_floating(Some(window));
                return;
            }
        }

        let tile = self
            .scrolling
            .tiles()
            .find(|tile| tile.window().id() == window)
            .unwrap();
        let was_normal = tile.window().pending_sizing_mode().is_normal();

        self.scrolling.set_maximized(window, maximize);

        // When going from normal to maximized, remember if we should unmaximize to floating.
        let tile = self
            .scrolling
            .tiles_mut()
            .find(|tile| tile.window().id() == window)
            .unwrap();
        if was_normal && !tile.window().pending_sizing_mode().is_normal() {
            tile.restore_to_floating = restore_to_floating;
        }
    }

    pub fn toggle_maximized(&mut self, window: &W::Id) {
        let mut current = false;

        // We have to check the column property in case the window is in the scrolling layout and
        // both maximized and fullscreen. In this case, only the column knows whether it's
        // maximized.
        //
        // In the floating layout, windows cannot be maximized.
        if let Some(col) = self.scrolling.columns().find(|col| col.contains(window)) {
            current = col.is_pending_maximized();
        }

        self.set_maximized(window, !current);
    }

    pub fn toggle_window_floating(&mut self, id: Option<&W::Id>) {
        let active_id = self.active_window().map(|win| win.id().clone());
        let target_is_active = id.is_none_or(|id| Some(id) == active_id.as_ref());
        let Some(id) = id.cloned().or(active_id) else {
            return;
        };

        let (_, render_pos, _) = self
            .tiles_with_render_positions()
            .find(|(tile, _, _)| *tile.window().id() == id)
            .unwrap();

        if self.floating.has_window(&id) {
            let removed = self.floating.remove_tile(&id);
            // FIXME: compute closest pos?
            self.scrolling.add_tile(
                None,
                removed.tile,
                target_is_active,
                removed.width,
                removed.is_full_width,
                None,
            );
            if target_is_active {
                self.floating_is_active = FloatingActive::No;
            }
        } else {
            let mut removed = self.scrolling.remove_tile(&id, Transaction::new());
            removed.tile.stop_move_animations();

            // Come up with a default floating position close to the tile position.
            let stored_or_default = self.floating.stored_or_default_tile_pos(&removed.tile);
            if stored_or_default.is_none() {
                let offset =
                    if self.options.layout.center_focused_column == CenterFocusedColumn::Always {
                        Point::from((0., 0.))
                    } else {
                        Point::from((50., 50.))
                    };
                let pos = render_pos + offset;
                let size = removed.tile.tile_size();
                let pos = self.floating.clamp_within_working_area(pos, size);
                let pos = self.floating.logical_to_size_frac(pos);
                removed.tile.floating_pos = Some(pos);
            }

            self.floating.add_tile(removed.tile, target_is_active);
            if target_is_active {
                self.floating_is_active = FloatingActive::Yes;
            }
        }

        let (tile, new_render_pos) = self
            .tiles_with_render_positions_mut(false)
            .find(|(tile, _)| *tile.window().id() == id)
            .unwrap();

        tile.animate_move_from(render_pos - new_render_pos);
    }

    pub fn set_window_floating(&mut self, id: Option<&W::Id>, floating: bool) {
        if id.map_or(self.floating_is_active.get(), |id| {
            self.floating.has_window(id)
        }) == floating
        {
            return;
        }

        self.toggle_window_floating(id);
    }

    pub fn focus_floating(&mut self) {
        if !self.floating_is_active.get() {
            self.switch_focus_floating_tiling();
        }
    }

    pub fn focus_tiling(&mut self) {
        if self.floating_is_active.get() {
            self.switch_focus_floating_tiling();
        }
    }

    pub fn switch_focus_floating_tiling(&mut self) {
        if self.floating.is_empty() {
            // If floating is empty, keep focus on scrolling.
            return;
        } else if self.scrolling.is_empty() {
            // If floating isn't empty but scrolling is, keep focus on floating.
            return;
        }

        self.floating_is_active = if self.floating_is_active.get() {
            FloatingActive::No
        } else {
            FloatingActive::Yes
        };
    }

    pub fn move_floating_window(
        &mut self,
        id: Option<&W::Id>,
        x: PositionChange,
        y: PositionChange,
        animate: bool,
    ) {
        if id.map_or(self.floating_is_active.get(), |id| {
            self.floating.has_window(id)
        }) {
            self.floating.move_window(id, x, y, animate);
        } else {
            // If the target tile isn't floating, set its stored floating position.
            let tile = if let Some(id) = id {
                self.scrolling
                    .tiles_mut()
                    .find(|tile| tile.window().id() == id)
                    .unwrap()
            } else if let Some(tile) = self.scrolling.active_tile_mut() {
                tile
            } else {
                return;
            };

            let pos = self.floating.stored_or_default_tile_pos(tile);

            // If there's no stored floating position, we can only set both components at once, not
            // adjust.
            let pos = pos.or_else(|| {
                (matches!(
                    x,
                    PositionChange::SetFixed(_) | PositionChange::SetProportion(_)
                ) && matches!(
                    y,
                    PositionChange::SetFixed(_) | PositionChange::SetProportion(_)
                ))
                .then_some(Point::default())
            });

            let Some(mut pos) = pos else {
                return;
            };

            let working_area = self.floating.working_area();
            let available_width = working_area.size.w;
            let available_height = working_area.size.h;
            let working_area_loc = working_area.loc;

            const MAX_F: f64 = 10000.;

            match x {
                PositionChange::SetFixed(x) => pos.x = x + working_area_loc.x,
                PositionChange::SetProportion(prop) => {
                    let prop = (prop / 100.).clamp(0., MAX_F);
                    pos.x = available_width * prop + working_area_loc.x;
                }
                PositionChange::AdjustFixed(x) => pos.x += x,
                PositionChange::AdjustProportion(prop) => {
                    let current_prop = (pos.x - working_area_loc.x) / available_width.max(1.);
                    let prop = (current_prop + prop / 100.).clamp(0., MAX_F);
                    pos.x = available_width * prop + working_area_loc.x;
                }
            }
            match y {
                PositionChange::SetFixed(y) => pos.y = y + working_area_loc.y,
                PositionChange::SetProportion(prop) => {
                    let prop = (prop / 100.).clamp(0., MAX_F);
                    pos.y = available_height * prop + working_area_loc.y;
                }
                PositionChange::AdjustFixed(y) => pos.y += y,
                PositionChange::AdjustProportion(prop) => {
                    let current_prop = (pos.y - working_area_loc.y) / available_height.max(1.);
                    let prop = (current_prop + prop / 100.).clamp(0., MAX_F);
                    pos.y = available_height * prop + working_area_loc.y;
                }
            }

            let pos = self.floating.logical_to_size_frac(pos);
            tile.floating_pos = Some(pos);
        }
    }

    pub fn has_windows(&self) -> bool {
        self.windows().next().is_some()
    }

    pub fn has_window(&self, window: &W::Id) -> bool {
        self.windows().any(|win| win.id() == window)
    }

    pub fn find_wl_surface(&self, wl_surface: &WlSurface) -> Option<&W> {
        self.windows().find(|win| win.is_wl_surface(wl_surface))
    }

    pub fn find_wl_surface_mut(&mut self, wl_surface: &WlSurface) -> Option<&mut W> {
        self.windows_mut().find(|win| win.is_wl_surface(wl_surface))
    }

    pub fn tiles_with_render_positions(
        &self,
    ) -> impl Iterator<Item = (&Tile<W>, Point<f64, Logical>, bool)> {
        let scrolling = self.scrolling.tiles_with_render_positions();

        let floating = self.floating.tiles_with_render_positions();
        let visible = self.is_floating_visible();
        let floating = floating.map(move |(tile, pos)| (tile, pos, visible));

        floating.chain(scrolling)
    }

    pub fn tiles_with_render_positions_mut(
        &mut self,
        round: bool,
    ) -> impl Iterator<Item = (&mut Tile<W>, Point<f64, Logical>)> {
        let scrolling = self.scrolling.tiles_with_render_positions_mut(round);
        let floating = self.floating.tiles_with_render_positions_mut(round);
        floating.chain(scrolling)
    }

    pub fn tiles_with_ipc_layouts(&self) -> impl Iterator<Item = (&Tile<W>, WindowLayout)> {
        let scrolling = self.scrolling.tiles_with_ipc_layouts();
        let floating = self.floating.tiles_with_ipc_layouts();
        floating.chain(scrolling)
    }

    pub fn active_window_visual_rectangle(&self) -> Option<Rectangle<f64, Logical>> {
        if let Some(go) = &self.grid_overview {
            if go.is_fully_open() {
                if let Some(info) = go.focused_info() {
                    return Some(Rectangle::new(info.target_pos, info.target_size));
                }
            }
        }
        if self.floating_is_active.get() {
            self.floating.active_window_visual_rectangle()
        } else {
            self.scrolling.active_window_visual_rectangle()
        }
    }

    pub fn popup_target_rect(&self, window: &W::Id) -> Option<Rectangle<f64, Logical>> {
        if self.floating.has_window(window) {
            self.floating.popup_target_rect(window)
        } else {
            self.scrolling.popup_target_rect(window)
        }
    }

    pub fn render_scrolling<R: NiriRenderer>(
        &self,
        ctx: RenderCtx<R>,
        xray_pos: XrayPos,
        focus_ring: bool,
        push: &mut dyn FnMut(WorkspaceRenderElement<R>),
    ) {
        let scrolling_focus_ring = focus_ring && !self.floating_is_active();
        self.scrolling
            .render(ctx, xray_pos, scrolling_focus_ring, &mut |elem| {
                push(elem.into())
            });
    }

    pub fn render_floating<R: NiriRenderer>(
        &self,
        ctx: RenderCtx<R>,
        xray_pos: XrayPos,
        focus_ring: bool,
        push: &mut dyn FnMut(WorkspaceRenderElement<R>),
    ) {
        if !self.is_floating_visible() {
            return;
        }

        let view_rect = Rectangle::from_size(self.view_size);
        let floating_focus_ring = focus_ring && self.floating_is_active();
        self.floating
            .render(ctx, xray_pos, view_rect, floating_focus_ring, &mut |elem| {
                push(elem.into())
            });
    }

    pub fn render_shadow<R: NiriRenderer>(
        &self,
        renderer: &mut R,
        push: &mut dyn FnMut(ShadowRenderElement),
    ) {
        self.shadow.render(renderer, Point::from((0., 0.)), push);
    }

    pub fn render_background(&self) -> SolidColorRenderElement {
        SolidColorRenderElement::from_buffer(
            &self.background_buffer,
            Point::new(0., 0.),
            1.,
            Kind::Unspecified,
        )
    }

    pub fn render_grid_overview<R: NiriRenderer>(
        &self,
        mut ctx: RenderCtx<R>,
        push: &mut dyn FnMut(WorkspaceRenderElement<R>),
        base_xray_pos: XrayPos,
        focus_ring: bool,
    ) {
        let scale = self.scale.fractional_scale();
        let overview_zoom = base_xray_pos.zoom;

        let go = match &self.grid_overview {
            Some(g) => g,
            None => return,
        };
        let layout = &go.layout;
        let focus = go.focus;
        let is_closing = !go.open;
        let is_opening = go
            .progress
            .as_ref()
            .is_some_and(|progress| go.open && progress.is_animation());
        let should_render_grid_item =
            |item: &GridItem<W>| !is_closing || self.grid_item_visible_when_closing(item);

        if self.is_floating_visible() {
            let active_floating_id = (focus_ring && self.floating_is_active())
                .then(|| self.floating.active_window())
                .flatten()
                .map(|win| win.id());
            for (tile, tile_pos) in self
                .floating
                .tiles_with_render_positions()
                .filter(|(tile, _)| Self::tile_ignores_grid_overview(tile))
            {
                let xray_pos = base_xray_pos.offset(tile_pos);
                let render_focus_ring = active_floating_id == Some(tile.window().id());
                tile.render(
                    ctx.r(),
                    tile_pos,
                    xray_pos,
                    render_focus_ring,
                    &mut |elem| {
                        let elem: FloatingSpaceRenderElement<R> = elem.into();
                        push(elem.into());
                    },
                );
            }
        }

        {
            let render_tile = |ctx: &mut RenderCtx<R>,
                               push: &mut dyn FnMut(WorkspaceRenderElement<R>),
                               tile: &Tile<W>,
                               tile_rel_pos: Point<f64, Logical>,
                               item_visual_pos: Point<f64, Logical>,
                               item_visual_scale: f64,
                               render_focus_ring: bool,
                               suppress_decorations: bool,
                               suppress_shadow: bool,
                               ignore_alpha_animation: bool| {
                let geo = base_xray_pos.pos_in_backdrop.upscale(overview_zoom);
                let tile_visual_pos = item_visual_pos + tile_rel_pos.upscale(item_visual_scale);
                let xray_pos = XrayPos::new(
                    geo + tile_visual_pos.upscale(overview_zoom),
                    item_visual_scale * overview_zoom,
                );

                let mut push_grid_elem = |elem: TileRenderElement<R>| {
                    if suppress_shadow && matches!(&elem, TileRenderElement::Shadow(_)) {
                        return;
                    }
                    if suppress_decorations {
                        match elem {
                            TileRenderElement::FocusRing(_) | TileRenderElement::Border(_) => {
                                return;
                            }
                            _ => {}
                        }
                    }
                    let elem: ScrollingSpaceRenderElement<R> = elem.into();
                    let origin = Point::<i32, smithay::utils::Physical>::from((0, 0));
                    let elem = RescaleRenderElement::from_element(elem, origin, item_visual_scale);
                    let phys_pos = item_visual_pos.to_physical_precise_round(scale);
                    let elem =
                        RelocateRenderElement::from_element(elem, phys_pos, Relocate::Relative);
                    push(elem.into());
                };

                if ignore_alpha_animation {
                    tile.render_ignoring_alpha_animation(
                        ctx.r(),
                        tile_rel_pos,
                        xray_pos,
                        render_focus_ring,
                        &mut push_grid_elem,
                    );
                } else {
                    tile.render(
                        ctx.r(),
                        tile_rel_pos,
                        xray_pos,
                        render_focus_ring,
                        &mut push_grid_elem,
                    );
                }
            };

            let render_tab_indicator =
                |ctx: &mut RenderCtx<R>,
                 push: &mut dyn FnMut(WorkspaceRenderElement<R>),
                 tab_indicator: &TabIndicator,
                 tab_indicator_rel_pos: Point<f64, Logical>,
                 item_visual_pos: Point<f64, Logical>,
                 item_visual_scale: f64| {
                    let mut push_grid_elem =
                        |elem: super::tab_indicator::TabIndicatorRenderElement| {
                            let elem: ScrollingSpaceRenderElement<R> = elem.into();
                            let origin = Point::<i32, smithay::utils::Physical>::from((0, 0));
                            let elem =
                                RescaleRenderElement::from_element(elem, origin, item_visual_scale);
                            let phys_pos = item_visual_pos.to_physical_precise_round(scale);
                            let elem = RelocateRenderElement::from_element(
                                elem,
                                phys_pos,
                                Relocate::Relative,
                            );
                            push(elem.into());
                        };

                    tab_indicator.render(ctx.renderer, tab_indicator_rel_pos, &mut push_grid_elem);
                };

            let tab_is_active = |col_idx: usize, window_id: &W::Id| {
                let active_idx = self.scrolling.column_active_tile_idx(col_idx);
                self.scrolling
                    .columns()
                    .nth(col_idx)
                    .and_then(|col| col.tiles().nth(active_idx))
                    .map_or(false, |(tile, _)| tile.window().id() == window_id)
            };
            let is_active_tab_item = |item: &GridItem<W>| match item {
                GridItem::Tab {
                    col_idx, window_id, ..
                } => tab_is_active(*col_idx, window_id),
                _ => false,
            };
            let is_inactive_tab_item = |item: &GridItem<W>| {
                matches!(item, GridItem::Tab { .. }) && !is_active_tab_item(item)
            };
            let renders_on_top_when_closing = |item: &GridItem<W>| {
                is_closing && self.grid_item_renders_on_top_when_grid_closing(item)
            };

            let mut render_grid_item = |ctx: &mut RenderCtx<R>,
                                        item: &GridItem<W>,
                                        info: &GridEntryInfo,
                                        is_focused: bool| {
                let (visual_pos, visual_scale) = go.entry_visual_transform(
                    item,
                    info,
                    self.grid_item_normal_render_pos(item, false)
                        .unwrap_or(info.target_pos),
                );

                let is_tab = matches!(item, GridItem::Tab { .. });
                let is_active_tab = is_active_tab_item(item);
                let suppress_decorations = is_closing && is_tab && !is_focused;
                let suppress_shadow = suppress_decorations && !is_active_tab;

                match item {
                    GridItem::Column { col_idx, .. } => {
                        let Some(preview) = self.scrolling.grid_preview_with_stable_origin(item)
                        else {
                            return;
                        };
                        let grid_tile_idx = go.get_column_tile_focus(*col_idx);
                        // Render lower tiles on top of upper ones. Tiles are anchored at their
                        // top edge, so any transient overflow (e.g. a freshly merged-in window
                        // still at its full pre-resize height) extends downwards; drawing the
                        // lower neighbor above it hides the overflow, and the neighbor's top
                        // edge cleanly reveals the new window as the sizes settle.
                        for preview_tile in preview.tiles.into_iter().rev() {
                            let is_grid_focused = preview_tile.tile_idx == grid_tile_idx;
                            let target_tile_pos =
                                visual_pos + preview_tile.pos.upscale(visual_scale);
                            let (tile_visual_pos, tile_visual_scale) = go.window_visual_transform(
                                preview_tile.tile.window().id(),
                                target_tile_pos,
                                visual_scale,
                            );
                            let item_visual_pos =
                                tile_visual_pos - preview_tile.pos.upscale(tile_visual_scale);
                            render_tile(
                                ctx,
                                push,
                                preview_tile.tile,
                                preview_tile.pos,
                                item_visual_pos,
                                tile_visual_scale,
                                is_focused && is_grid_focused,
                                false,
                                false,
                                true,
                            );
                        }
                    }
                    GridItem::Tab { window_id, .. } => {
                        let Some(preview) = self.scrolling.grid_preview_with_stable_origin(item)
                        else {
                            return;
                        };

                        for preview_tile in preview.tiles {
                            let target_tile_pos =
                                visual_pos + preview_tile.pos.upscale(visual_scale);
                            let (tile_visual_pos, tile_visual_scale) = go.window_visual_transform(
                                preview_tile.tile.window().id(),
                                target_tile_pos,
                                visual_scale,
                            );
                            let item_visual_pos =
                                tile_visual_pos - preview_tile.pos.upscale(tile_visual_scale);
                            render_tile(
                                ctx,
                                push,
                                preview_tile.tile,
                                preview_tile.pos,
                                item_visual_pos,
                                tile_visual_scale,
                                is_focused && preview_tile.tile.window().id() == window_id,
                                suppress_decorations,
                                suppress_shadow,
                                true,
                            );
                        }

                        if is_closing {
                            if let (Some(tab_indicator), Some(tab_indicator_pos)) =
                                (preview.tab_indicator, preview.tab_indicator_pos)
                            {
                                render_tab_indicator(
                                    ctx,
                                    push,
                                    tab_indicator,
                                    tab_indicator_pos,
                                    visual_pos,
                                    visual_scale,
                                );
                            }
                        }
                    }
                    GridItem::Floating { window_id } => {
                        let Some((tile, _)) = self
                            .floating
                            .tiles_with_render_positions()
                            .find(|(tile, _)| tile.window().id() == window_id)
                        else {
                            return;
                        };

                        let (tile_visual_pos, tile_visual_scale) =
                            go.window_visual_transform(window_id, visual_pos, visual_scale);

                        render_tile(
                            ctx,
                            push,
                            tile,
                            Point::from((0., 0.)),
                            tile_visual_pos,
                            tile_visual_scale,
                            is_focused,
                            false,
                            false,
                            false,
                        );
                    }
                }
            };

            if is_closing {
                for (item, info) in &layout.entries {
                    let is_focused = info.row == focus.0 && info.col == focus.1;
                    if should_render_grid_item(item) && renders_on_top_when_closing(item) {
                        render_grid_item(&mut ctx, item, info, is_focused);
                    }
                }
            }

            // Render elements are queued top-to-bottom. Keep floating grid items above tiling grid
            // items while preserving the existing within-layer focus and tab ordering.
            for render_floating_layer in [true, false] {
                let is_item_in_layer = |item: &GridItem<W>| {
                    matches!(item, GridItem::Floating { .. }) == render_floating_layer
                };

                for (item, info) in &layout.entries {
                    let is_focused = info.row == focus.0 && info.col == focus.1;
                    if is_focused
                        && should_render_grid_item(item)
                        && is_item_in_layer(item)
                        && !renders_on_top_when_closing(item)
                        && !(is_closing && is_inactive_tab_item(item))
                    {
                        render_grid_item(&mut ctx, item, info, true);
                    }
                }

                // Keep the real active tab above its inactive tabs while they split out of or
                // merge back into the tabbed column.
                if is_opening || is_closing {
                    for (item, info) in &layout.entries {
                        let is_focused = info.row == focus.0 && info.col == focus.1;
                        if !is_focused
                            && should_render_grid_item(item)
                            && is_item_in_layer(item)
                            && !renders_on_top_when_closing(item)
                            && is_active_tab_item(item)
                        {
                            render_grid_item(&mut ctx, item, info, false);
                        }
                    }
                }

                if is_closing {
                    for (item, info) in &layout.entries {
                        let is_focused = info.row == focus.0 && info.col == focus.1;
                        if is_focused
                            && should_render_grid_item(item)
                            && is_item_in_layer(item)
                            && !renders_on_top_when_closing(item)
                            && is_inactive_tab_item(item)
                        {
                            render_grid_item(&mut ctx, item, info, false);
                        }
                    }
                }

                for (item, info) in &layout.entries {
                    let is_focused = info.row == focus.0 && info.col == focus.1;
                    if is_focused || !should_render_grid_item(item) || !is_item_in_layer(item) {
                        continue;
                    }
                    if renders_on_top_when_closing(item) {
                        continue;
                    }
                    if (is_opening || is_closing) && is_active_tab_item(item) {
                        continue;
                    }

                    render_grid_item(&mut ctx, item, info, false);
                }
            }
        }

        // Render closing windows behind live grid tiles, preserving floating-above-tiling order.
        let view_rect = Rectangle::new(Point::from((0., 0.)), self.view_size);
        if self.is_floating_visible() {
            for closing in self.floating.closing_windows() {
                let elem = closing.render(ctx.as_gles(), view_rect, Scale::from(scale));
                let elem: FloatingSpaceRenderElement<R> = elem.into();
                push(elem.into());
            }
        }
        for closing in self.scrolling.closing_windows() {
            let elem = closing.render(ctx.as_gles(), view_rect, Scale::from(scale));
            let elem: ScrollingSpaceRenderElement<R> = elem.into();
            push(elem.into());
        }
    }

    pub fn ignored_floating_window_under(&self, pos: Point<f64, Logical>) -> Option<(&W, HitType)> {
        if !self.is_floating_visible() {
            return None;
        }

        self.floating
            .tiles_with_render_positions()
            .find_map(|(tile, tile_pos)| {
                if !Self::tile_ignores_grid_overview(tile) {
                    return None;
                }

                HitType::hit_tile(tile, tile_pos, pos)
            })
    }

    pub fn render_above_top_layer(&self) -> bool {
        if self
            .grid_overview
            .as_ref()
            .is_some_and(|go| go.open || go.progress.is_some() || go.are_animations_ongoing())
        {
            return false;
        }

        self.scrolling.render_above_top_layer()
    }

    pub fn is_floating_visible(&self) -> bool {
        // If the focus is on a fullscreen scrolling window, hide the floating windows.
        matches!(
            self.floating_is_active,
            FloatingActive::Yes | FloatingActive::NoButRaised
        ) || !self.render_above_top_layer()
    }

    pub fn store_unmap_snapshot_if_empty(
        &mut self,
        renderer: &mut GlesRenderer,
        xray: Option<&mut Xray>,
        xray_has_blocked_out_layers: bool,
        xray_pos: XrayPos,
        window: &W::Id,
    ) {
        let view_size = self.view_size();
        for (tile, tile_pos) in self.tiles_with_render_positions_mut(false) {
            if tile.window().id() == window {
                let view_pos = Point::from((-tile_pos.x, -tile_pos.y));
                let view_rect = Rectangle::new(view_pos, view_size);
                tile.update_render_elements(false, view_rect);
                let xray_pos = xray_pos.offset(tile_pos);
                tile.store_unmap_snapshot_if_empty(
                    renderer,
                    xray,
                    xray_has_blocked_out_layers,
                    xray_pos,
                );
                return;
            }
        }
    }

    pub fn clear_unmap_snapshot(&mut self, window: &W::Id) {
        for tile in self.tiles_mut() {
            if tile.window().id() == window {
                let _ = tile.take_unmap_snapshot();
                return;
            }
        }
    }

    pub fn start_close_animation_for_window(
        &mut self,
        renderer: &mut GlesRenderer,
        window: &W::Id,
        blocker: TransactionBlocker,
    ) {
        let grid_target = {
            let go = self.grid_overview.as_ref().filter(|go| go.open);
            go.and_then(|go| {
                let item = self.grid_item_for_window(window)?;
                let (_, info) = go
                    .layout
                    .entries
                    .iter()
                    .find(|(entry, _)| entry.matches_animation_key(&item))?;
                let (visual_pos, visual_scale) = self.grid_item_visual_transform(go, &item, info);

                if self.floating.has_window(window) {
                    let tile_size = self
                        .floating
                        .tiles()
                        .find(|tile| tile.window().id() == window)
                        .map(|tile| tile.tile_size().upscale(visual_scale))
                        .unwrap_or(info.target_size);
                    return Some((true, tile_size, visual_pos));
                }

                let preview = self.scrolling.grid_preview_with_stable_origin(&item)?;
                let preview_tile = preview
                    .tiles
                    .into_iter()
                    .find(|tile| tile.tile.window().id() == window)?;
                let tile_size = preview_tile.tile.tile_size().upscale(visual_scale);
                let tile_pos = visual_pos + preview_tile.pos.upscale(visual_scale);
                Some((false, tile_size, tile_pos))
            })
        };

        if let Some((is_floating, tile_size, tile_pos)) = grid_target {
            if is_floating {
                self.floating.start_close_animation_for_window_at(
                    renderer, window, tile_size, tile_pos, blocker,
                );
            } else {
                self.scrolling.start_close_animation_for_window_at(
                    renderer, window, tile_size, tile_pos, blocker,
                );
            }
            return;
        }

        if self.floating.has_window(window) {
            self.floating
                .start_close_animation_for_window(renderer, window, blocker);
        } else {
            self.scrolling
                .start_close_animation_for_window(renderer, window, blocker);
        }
    }

    pub fn start_close_animation_for_tile(
        &mut self,
        renderer: &mut GlesRenderer,
        snapshot: TileRenderSnapshot,
        tile_size: Size<f64, Logical>,
        tile_pos: Point<f64, Logical>,
        blocker: TransactionBlocker,
    ) {
        self.floating
            .start_close_animation_for_tile(renderer, snapshot, tile_size, tile_pos, blocker);
    }

    pub fn start_open_animation(&mut self, id: &W::Id) -> bool {
        self.scrolling.start_open_animation(id) || self.floating.start_open_animation(id)
    }

    pub fn grid_window_at(&self, pos: Point<f64, Logical>) -> Option<W::Id> {
        let go = self.grid_overview.as_ref()?;
        if !go.open {
            return None;
        }

        for pass in 0..3 {
            for (item, info) in &go.layout.entries {
                let is_focused = info.row == go.focus.0 && info.col == go.focus.1;
                let is_previous_focus = go.previous_focus == Some((info.row, info.col));
                let should_check = match pass {
                    0 => is_focused,
                    1 => !is_focused && is_previous_focus,
                    _ => !is_focused && !is_previous_focus,
                };
                if !should_check {
                    continue;
                }

                let (visual_pos, visual_scale) = self.grid_item_visual_transform(go, item, info);
                let source_size = info.target_size.downscale(info.target_scale.max(0.0001));
                let visual_size = source_size.upscale(visual_scale);
                let rect = Rectangle::new(visual_pos, visual_size);

                if rect.contains(pos) {
                    match item {
                        GridItem::Column { .. } | GridItem::Tab { .. } => {
                            if let Some(preview) =
                                self.scrolling.grid_preview_with_stable_origin(item)
                            {
                                let rel = (pos - visual_pos).downscale(visual_scale.max(0.0001));
                                // Match the render order: lower tiles draw on top of upper ones.
                                for preview_tile in preview.tiles.into_iter().rev() {
                                    let tile_size = preview_tile.tile.animated_tile_size();
                                    let tile_rect = Rectangle::new(preview_tile.pos, tile_size);
                                    if tile_rect.contains(rel) {
                                        return Some(preview_tile.tile.window().id().clone());
                                    }
                                }
                            }
                        }
                        GridItem::Floating { .. } => (),
                    }

                    return Some(item.window_id().clone());
                }
            }
        }
        None
    }

    pub fn window_under(&self, pos: Point<f64, Logical>) -> Option<(&W, HitType)> {
        // This logic is consistent with tiles_with_render_positions().
        if self.is_floating_visible() {
            if let Some(rv) = self
                .floating
                .tiles_with_render_positions()
                .find_map(|(tile, tile_pos)| HitType::hit_tile(tile, tile_pos, pos))
            {
                return Some(rv);
            }
        }

        self.scrolling.window_under(pos)
    }

    pub fn resize_edges_under(&self, pos: Point<f64, Logical>) -> Option<ResizeEdge> {
        self.tiles_with_render_positions()
            .find_map(|(tile, tile_pos, visible)| {
                // This logic should be consistent with window_under() in when it returns Some vs.
                // None.
                if !visible {
                    return None;
                }

                let pos_within_tile = pos - tile_pos;

                if tile.hit(pos_within_tile).is_some() {
                    let size = tile.tile_size().to_f64();

                    let mut edges = ResizeEdge::empty();
                    if pos_within_tile.x < size.w / 3. {
                        edges |= ResizeEdge::LEFT;
                    } else if 2. * size.w / 3. < pos_within_tile.x {
                        edges |= ResizeEdge::RIGHT;
                    }
                    if pos_within_tile.y < size.h / 3. {
                        edges |= ResizeEdge::TOP;
                    } else if 2. * size.h / 3. < pos_within_tile.y {
                        edges |= ResizeEdge::BOTTOM;
                    }
                    return Some(edges);
                }

                None
            })
    }

    pub fn descendants_added(&mut self, id: &W::Id) -> bool {
        self.floating.descendants_added(id)
    }

    pub fn update_window(&mut self, window: &W::Id, serial: Option<Serial>) {
        if !self.floating.update_window(window, serial) {
            self.scrolling.update_window(window, serial);
        }
        if self.is_grid_overview_open() {
            self.recompute_grid_overview_layout(false);
        }
    }

    pub fn refresh(&mut self, is_active: bool, is_focused: bool) {
        self.scrolling
            .refresh(is_active && !self.floating_is_active.get(), is_focused);
        self.floating
            .refresh(is_active && self.floating_is_active.get(), is_focused);
    }

    pub fn scroll_amount_to_activate(&self, window: &W::Id) -> f64 {
        if self.floating.has_window(window) {
            return 0.;
        }

        self.scrolling.scroll_amount_to_activate(window)
    }

    pub fn is_urgent(&self) -> bool {
        self.windows().any(|win| win.is_urgent())
    }

    pub fn activate_window_silent(&mut self, window: &W::Id) -> bool {
        if self.floating.has_window(window) {
            self.floating.activate_window(window);
            self.floating_is_active = FloatingActive::Yes;
            return true;
        }

        if self.scrolling.set_active_window_silent(window) {
            self.floating_is_active = FloatingActive::No;
            true
        } else {
            false
        }
    }

    pub fn activate_window_from_grid(&mut self, window: &W::Id) -> bool {
        let previous_window = self
            .grid_overview
            .as_ref()
            .and_then(|go| go.saved_active_window_id.clone());
        let previous_view_pos = self.grid_overview.as_ref().map(|go| go.saved_view_offset);

        if self.floating.activate_window(window) {
            self.floating_is_active = FloatingActive::Yes;
            true
        } else if self.scrolling.activate_window_from_grid(
            window,
            previous_window.as_ref(),
            previous_view_pos,
        ) {
            self.floating_is_active = FloatingActive::No;
            true
        } else {
            false
        }
    }

    pub fn activate_window(&mut self, window: &W::Id) -> bool {
        if self.floating.activate_window(window) {
            self.floating_is_active = FloatingActive::Yes;
            true
        } else if self.scrolling.activate_window(window) {
            self.floating_is_active = FloatingActive::No;
            true
        } else {
            false
        }
    }

    pub fn activate_window_without_raising(&mut self, window: &W::Id) -> bool {
        if self.floating.activate_window_without_raising(window) {
            self.floating_is_active = FloatingActive::Yes;
            true
        } else if self.scrolling.activate_window(window) {
            self.floating_is_active = match self.floating_is_active {
                FloatingActive::No => FloatingActive::No,
                FloatingActive::NoButRaised => FloatingActive::NoButRaised,
                FloatingActive::Yes => FloatingActive::NoButRaised,
            };
            true
        } else {
            false
        }
    }

    pub(super) fn scrolling_insert_position(&self, pos: Point<f64, Logical>) -> InsertPosition {
        self.scrolling.insert_position(pos)
    }

    pub(super) fn grid_insert_position(&self, pos: Point<f64, Logical>) -> Option<InsertPosition> {
        self.grid_insert_position_and_hint_area(pos)
            .map(|(position, _)| position)
    }

    pub(super) fn grid_insert_position_and_hint_area(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<(InsertPosition, Rectangle<f64, Logical>)> {
        let go = self.grid_overview.as_ref().filter(|go| go.open)?;
        let column_count = self.scrolling.columns().count();

        for pass in 0..3 {
            for (item, info) in &go.layout.entries {
                let is_focused = info.row == go.focus.0 && info.col == go.focus.1;
                let is_previous_focus = go.previous_focus == Some((info.row, info.col));
                let should_check = match pass {
                    0 => is_focused,
                    1 => !is_focused && is_previous_focus,
                    _ => !is_focused && !is_previous_focus,
                };
                if !should_check {
                    continue;
                }

                let (visual_pos, visual_scale) = self.grid_item_visual_transform(go, item, info);
                let source_size = info.target_size.downscale(info.target_scale.max(0.0001));
                let visual_size = source_size.upscale(visual_scale);
                if !Rectangle::new(visual_pos, visual_size).contains(pos) {
                    continue;
                }

                let position = self.grid_insert_position_for_item(
                    item,
                    pos,
                    visual_pos,
                    visual_scale,
                    source_size,
                    column_count,
                );
                let hint_area = self.grid_insert_hint_area_for_item(
                    go,
                    item,
                    info,
                    position,
                    visual_pos,
                    visual_scale,
                    source_size,
                )?;
                return Some((position, hint_area));
            }
        }

        let mut nearest = None;
        for (item, info) in &go.layout.entries {
            let (visual_pos, visual_scale) = self.grid_item_visual_transform(go, item, info);
            let source_size = info.target_size.downscale(info.target_scale.max(0.0001));
            let visual_size = source_size.upscale(visual_scale);
            let rect = Rectangle::new(visual_pos, visual_size);

            let dx = if pos.x < rect.loc.x {
                rect.loc.x - pos.x
            } else if pos.x > rect.loc.x + rect.size.w {
                pos.x - (rect.loc.x + rect.size.w)
            } else {
                0.
            };
            let dy = if pos.y < rect.loc.y {
                rect.loc.y - pos.y
            } else if pos.y > rect.loc.y + rect.size.h {
                pos.y - (rect.loc.y + rect.size.h)
            } else {
                0.
            };
            let dist_sq = dx * dx + dy * dy;

            if nearest
                .as_ref()
                .is_none_or(|(_, _, _, _, _, best_dist_sq)| dist_sq < *best_dist_sq)
            {
                nearest = Some((item, info, visual_pos, visual_scale, source_size, dist_sq));
            }
        }

        let (item, info, visual_pos, visual_scale, source_size, dist_sq) = nearest?;
        let max_distance = go.layout.gap.max(100.);
        if dist_sq > max_distance * max_distance {
            return None;
        }

        let position = self.grid_insert_position_for_item(
            item,
            pos,
            visual_pos,
            visual_scale,
            source_size,
            column_count,
        );
        let hint_area = self.grid_insert_hint_area_for_item(
            go,
            item,
            info,
            position,
            visual_pos,
            visual_scale,
            source_size,
        )?;
        Some((position, hint_area))
    }

    fn grid_insert_position_for_item(
        &self,
        item: &GridItem<W>,
        pos: Point<f64, Logical>,
        visual_pos: Point<f64, Logical>,
        visual_scale: f64,
        source_size: Size<f64, Logical>,
        column_count: usize,
    ) -> InsertPosition {
        let source_pos = (pos - visual_pos).downscale(visual_scale.max(0.0001));
        let edge = source_size.w * 0.25;

        match item {
            GridItem::Column { col_idx, .. } => {
                if source_pos.x < edge {
                    InsertPosition::NewColumn(*col_idx)
                } else if source_pos.x > source_size.w - edge {
                    InsertPosition::NewColumn(col_idx + 1)
                } else {
                    let tile_idx = self.grid_in_column_tile_insert_idx(item, source_pos.y);
                    InsertPosition::InColumn(*col_idx, tile_idx)
                }
            }
            GridItem::Tab {
                col_idx, tile_idx, ..
            } => {
                if source_pos.x < edge {
                    InsertPosition::NewColumn(*col_idx)
                } else if source_pos.x > source_size.w - edge {
                    InsertPosition::NewColumn(col_idx + 1)
                } else {
                    let after = source_pos.y >= source_size.h / 2.;
                    InsertPosition::InColumn(*col_idx, tile_idx + usize::from(after))
                }
            }
            GridItem::Floating { .. } => InsertPosition::NewColumn(column_count),
        }
    }

    fn grid_insert_hint_area_for_item(
        &self,
        go: &GridOverview<W>,
        item: &GridItem<W>,
        info: &GridEntryInfo,
        position: InsertPosition,
        visual_pos: Point<f64, Logical>,
        visual_scale: f64,
        source_size: Size<f64, Logical>,
    ) -> Option<Rectangle<f64, Logical>> {
        if let InsertPosition::NewColumn(column_idx) = position {
            let insert_left = match item {
                GridItem::Column { col_idx, .. } | GridItem::Tab { col_idx, .. } => {
                    column_idx <= *col_idx
                }
                GridItem::Floating { .. } => false,
            };
            return self.grid_new_column_insert_hint_area(
                go,
                info,
                column_idx,
                insert_left,
                visual_pos,
                visual_scale,
                source_size,
            );
        }

        let source_rect = match position {
            InsertPosition::NewColumn(_) => unreachable!(),
            InsertPosition::InColumn(_, tile_idx) => {
                let height = (source_size.h * 0.16).clamp(20., 150.);
                let y = self.grid_insert_hint_source_y(item, tile_idx, height)?;
                Rectangle::new(Point::from((0., y)), Size::from((source_size.w, height)))
            }
            InsertPosition::Floating => return None,
        };

        Some(Rectangle::new(
            visual_pos + source_rect.loc.upscale(visual_scale),
            source_rect.size.upscale(visual_scale),
        ))
    }

    fn grid_new_column_insert_hint_area(
        &self,
        go: &GridOverview<W>,
        info: &GridEntryInfo,
        column_idx: usize,
        insert_left: bool,
        visual_pos: Point<f64, Logical>,
        visual_scale: f64,
        source_size: Size<f64, Logical>,
    ) -> Option<Rectangle<f64, Logical>> {
        let current_rect = Rectangle::new(visual_pos, source_size.upscale(visual_scale));
        let left_rect = column_idx
            .checked_sub(1)
            .and_then(|idx| self.grid_column_group_edge_rect(go, idx, info.row, true));
        let right_rect = self.grid_column_group_edge_rect(go, column_idx, info.row, false);

        match (left_rect, right_rect) {
            (Some(left_rect), Some(right_rect)) => {
                let left_right = left_rect.loc.x + left_rect.size.w;
                let gap = (right_rect.loc.x - left_right).max(0.);
                let width = (gap + left_rect.size.w.min(right_rect.size.w) * 0.25).max(30.);
                let center_x = (left_right + right_rect.loc.x) / 2.;
                let top = left_rect.loc.y.min(right_rect.loc.y);
                let bottom =
                    (left_rect.loc.y + left_rect.size.h).max(right_rect.loc.y + right_rect.size.h);

                Some(Rectangle::new(
                    Point::from((center_x - width / 2., top)),
                    Size::from((width, bottom - top)),
                ))
            }
            (None, Some(right_rect)) => {
                let width = (right_rect.size.w * 0.25).clamp(30., 300.);
                Some(Rectangle::new(
                    Point::from((right_rect.loc.x - go.layout.gap - width, right_rect.loc.y)),
                    Size::from((width, right_rect.size.h)),
                ))
            }
            (Some(left_rect), None) => {
                let width = (left_rect.size.w * 0.25).clamp(30., 300.);
                Some(Rectangle::new(
                    Point::from((
                        left_rect.loc.x + left_rect.size.w + go.layout.gap,
                        left_rect.loc.y,
                    )),
                    Size::from((width, left_rect.size.h)),
                ))
            }
            (None, None) => {
                let width = (current_rect.size.w * 0.25).clamp(30., 300.);
                let x = if insert_left {
                    current_rect.loc.x - go.layout.gap - width
                } else {
                    current_rect.loc.x + current_rect.size.w + go.layout.gap
                };
                Some(Rectangle::new(
                    Point::from((x, current_rect.loc.y)),
                    Size::from((width, current_rect.size.h)),
                ))
            }
        }
    }

    fn grid_column_group_edge_rect(
        &self,
        go: &GridOverview<W>,
        col_idx: usize,
        preferred_row: usize,
        right_edge: bool,
    ) -> Option<Rectangle<f64, Logical>> {
        let mut best = None;
        for (item, info) in &go.layout.entries {
            let item_col_idx = match item {
                GridItem::Column { col_idx, .. } | GridItem::Tab { col_idx, .. } => *col_idx,
                GridItem::Floating { .. } => continue,
            };
            if item_col_idx != col_idx {
                continue;
            }
            if info.row != preferred_row {
                continue;
            }

            let (pos, scale) = self.grid_item_visual_transform(go, item, info);
            let source_size = info.target_size.downscale(info.target_scale.max(0.0001));
            let rect = Rectangle::new(pos, source_size.upscale(scale));
            let row_distance = info.row.abs_diff(preferred_row);
            let edge = if right_edge {
                rect.loc.x + rect.size.w
            } else {
                -rect.loc.x
            };

            if best.as_ref().is_none_or(|(best_row, best_edge, _)| {
                row_distance < *best_row || row_distance == *best_row && edge > *best_edge
            }) {
                best = Some((row_distance, edge, rect));
            }
        }

        best.map(|(_, _, rect)| rect)
    }

    fn grid_insert_hint_source_y(
        &self,
        item: &GridItem<W>,
        tile_idx: usize,
        hint_height: f64,
    ) -> Option<f64> {
        let preview = self.scrolling.grid_preview_with_stable_origin(item)?;
        let mut last_bottom = 0.;
        for preview_tile in preview.tiles {
            let top = preview_tile.pos.y;
            let bottom = top + preview_tile.tile.tile_size().h;
            if tile_idx <= preview_tile.tile_idx {
                return Some((top - hint_height / 2.).max(0.));
            }
            last_bottom = bottom;
        }

        Some((last_bottom - hint_height).max(0.))
    }

    fn grid_in_column_tile_insert_idx(&self, item: &GridItem<W>, y: f64) -> usize {
        let Some(preview) = self.scrolling.grid_preview_with_stable_origin(item) else {
            return 0;
        };

        for preview_tile in preview.tiles {
            let top = preview_tile.pos.y;
            let bottom = top + preview_tile.tile.tile_size().h;
            if y < (top + bottom) / 2. {
                return preview_tile.tile_idx;
            }
        }

        self.scrolling.column_tile_count(match item {
            GridItem::Column { col_idx, .. } | GridItem::Tab { col_idx, .. } => *col_idx,
            GridItem::Floating { .. } => 0,
        })
    }

    pub(super) fn insert_hint_area(
        &self,
        position: InsertPosition,
    ) -> Option<Rectangle<f64, Logical>> {
        self.scrolling.insert_hint_area(position)
    }

    pub fn view_offset_gesture_begin(&mut self, is_touchpad: bool) {
        self.scrolling.view_offset_gesture_begin(is_touchpad);
    }

    pub fn view_offset_gesture_update(
        &mut self,
        delta_x: f64,
        timestamp: Duration,
        is_touchpad: bool,
    ) -> Option<bool> {
        self.scrolling
            .view_offset_gesture_update(delta_x, timestamp, is_touchpad)
    }

    pub fn view_offset_gesture_end(&mut self, is_touchpad: Option<bool>) -> bool {
        self.scrolling.view_offset_gesture_end(is_touchpad)
    }

    pub fn dnd_scroll_gesture_begin(&mut self) {
        self.scrolling.dnd_scroll_gesture_begin();
    }

    pub fn dnd_scroll_gesture_scroll(&mut self, pos: Point<f64, Logical>, speed: f64) -> bool {
        let config = &self.options.gestures.dnd_edge_view_scroll;
        let trigger_width = config.trigger_width;

        // This working area intentionally does not include extra struts from Options.
        let x = pos.x - self.working_area.loc.x;
        let width = self.working_area.size.w;

        let x = x.clamp(0., width);
        let trigger_width = trigger_width.clamp(0., width / 2.);

        let delta = if x < trigger_width {
            -(trigger_width - x)
        } else if width - x < trigger_width {
            trigger_width - (width - x)
        } else {
            0.
        };

        let delta = if trigger_width < 0.01 {
            // Sanity check for trigger-width 0 or small window sizes.
            0.
        } else {
            // Normalize to [0, 1].
            delta / trigger_width
        };
        let delta = delta * speed;

        self.scrolling.dnd_scroll_gesture_scroll(delta)
    }

    pub fn dnd_scroll_gesture_end(&mut self) {
        self.scrolling.dnd_scroll_gesture_end();
    }

    pub fn interactive_resize_begin(&mut self, window: W::Id, edges: ResizeEdge) -> bool {
        if self.floating.has_window(&window) {
            self.floating.interactive_resize_begin(window, edges)
        } else {
            self.scrolling.interactive_resize_begin(window, edges)
        }
    }

    pub fn interactive_resize_update(
        &mut self,
        window: &W::Id,
        delta: Point<f64, Logical>,
    ) -> bool {
        if self.floating.has_window(window) {
            self.floating.interactive_resize_update(window, delta)
        } else {
            self.scrolling.interactive_resize_update(window, delta)
        }
    }

    pub fn interactive_resize_end(&mut self, window: Option<&W::Id>) {
        if let Some(window) = window {
            if self.floating.has_window(window) {
                self.floating.interactive_resize_end(Some(window));
            } else {
                self.scrolling.interactive_resize_end(Some(window));
            }
        } else {
            self.floating.interactive_resize_end(None);
            self.scrolling.interactive_resize_end(None);
        }
    }

    pub fn floating_is_active(&self) -> bool {
        self.floating_is_active.get()
    }

    pub fn floating_logical_to_size_frac(
        &self,
        logical_pos: Point<f64, Logical>,
    ) -> Point<f64, SizeFrac> {
        self.floating.logical_to_size_frac(logical_pos)
    }

    pub fn working_area(&self) -> Rectangle<f64, Logical> {
        self.working_area
    }

    pub fn layout_config(&self) -> Option<&niri_config::LayoutPart> {
        self.layout_config.as_ref()
    }

    #[cfg(test)]
    pub fn grid_overview(&self) -> Option<&GridOverview<W>> {
        self.grid_overview.as_ref()
    }

    #[cfg(test)]
    pub fn scrolling(&self) -> &ScrollingSpace<W> {
        &self.scrolling
    }

    #[cfg(test)]
    pub fn floating(&self) -> &FloatingSpace<W> {
        &self.floating
    }

    #[cfg(test)]
    pub fn verify_invariants(&self, move_win_id: Option<&W::Id>) {
        use approx::assert_abs_diff_eq;

        let scale = self.scale.fractional_scale();
        assert!(scale > 0.);
        assert!(scale.is_finite());

        let options = Options::clone(&self.base_options)
            .with_merged_layout(self.layout_config.as_ref())
            .adjusted_for_scale(scale);
        assert_eq!(
            &*self.options, &options,
            "options must be base options adjusted for scale"
        );

        assert!(self.view_size.w > 0.);
        assert!(self.view_size.h > 0.);

        assert_eq!(self.background_buffer.size(), self.view_size);
        assert_eq!(
            self.background_buffer.color().components(),
            options.layout.background_color.to_array_unpremul(),
        );

        assert_eq!(self.view_size, self.scrolling.view_size());
        assert_eq!(self.working_area, self.scrolling.parent_area());
        assert_eq!(&self.clock, self.scrolling.clock());
        assert!(Rc::ptr_eq(&self.options, self.scrolling.options()));
        self.scrolling.verify_invariants();

        assert_eq!(self.view_size, self.floating.view_size());
        assert_eq!(self.working_area, self.floating.working_area());
        assert_eq!(&self.clock, self.floating.clock());
        assert!(Rc::ptr_eq(&self.options, self.floating.options()));
        self.floating.verify_invariants();

        if self.floating.is_empty() {
            assert!(
                !self.floating_is_active.get(),
                "when floating is empty it must never be active"
            );
        } else if self.scrolling.is_empty() {
            assert!(
                self.floating_is_active.get(),
                "when scrolling is empty but floating isn't, floating should be active"
            );
        }

        for (tile, tile_pos, visible) in self.tiles_with_render_positions() {
            if Some(tile.window().id()) != move_win_id {
                assert_eq!(tile.interactive_move_offset, Point::from((0., 0.)));
            }

            let rounded_pos = tile_pos.to_physical_precise_round(scale).to_logical(scale);

            // Tile positions must be rounded to physical pixels.
            assert_abs_diff_eq!(tile_pos.x, rounded_pos.x, epsilon = 1e-5);
            assert_abs_diff_eq!(tile_pos.y, rounded_pos.y, epsilon = 1e-5);

            if let Some(alpha) = &tile.alpha_animation {
                let anim = &alpha.anim;
                if visible {
                    assert_eq!(anim.to(), 1., "visible tiles can animate alpha only to 1");
                }

                assert!(
                    !alpha.hold_after_done,
                    "tiles in the layout cannot have held alpha animation"
                );
            }
        }
    }
}

pub(super) fn compute_working_area(output: &Output) -> Rectangle<f64, Logical> {
    layer_map_for_output(output).non_exclusive_zone().to_f64()
}

fn compute_workspace_shadow_config(
    config: niri_config::WorkspaceShadow,
    view_size: Size<f64, Logical>,
) -> niri_config::Shadow {
    // Gaps between workspaces are a multiple of the view height, so shadow settings should also be
    // normalized to the view height to prevent them from overlapping on lower resolutions.
    let norm = view_size.h / 1080.;

    let mut config = niri_config::Shadow::from(config);
    config.softness *= norm;
    config.spread *= norm;
    config.offset.x.0 *= norm;
    config.offset.y.0 *= norm;

    config
}
