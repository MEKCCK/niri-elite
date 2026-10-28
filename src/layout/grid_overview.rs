use std::ops::Range;
use std::rc::Rc;
use std::time::Duration;

use bands::{Arrangement, BandLine, Bands, ARRANGEMENTS};
use niri_config::{CornerRadius, GridFocusRingTransition};
use rows::{Rows, MAX_FLOATING_ROWS};
use smithay::utils::{Logical, Point, Rectangle, Size};

use super::axis::AxisMap;
use super::focus_ring::{FocusRing, FocusRingRenderElement};
use super::tab_indicator::{TabIndicator, TabIndicatorRenderElement, TabInfo};
use super::{Animation, Clock, LayoutElement, Options, OverviewGesture, OverviewProgress};
use crate::input::swipe_tracker::SwipeTracker;
use crate::render_helpers::renderer::NiriRenderer;

mod bands;
mod rows;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GridDirection {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Debug)]
pub enum GridItem<W: LayoutElement> {
    Column {
        col_idx: usize,
        window_id: W::Id,
    },
    Tab {
        col_idx: usize,
        tile_idx: usize,
        window_id: W::Id,
    },
    Floating {
        window_id: W::Id,
    },
}

impl<W: LayoutElement> Clone for GridItem<W> {
    fn clone(&self) -> Self {
        match self {
            GridItem::Column { col_idx, window_id } => GridItem::Column {
                col_idx: *col_idx,
                window_id: window_id.clone(),
            },
            GridItem::Tab {
                col_idx,
                tile_idx,
                window_id,
            } => GridItem::Tab {
                col_idx: *col_idx,
                tile_idx: *tile_idx,
                window_id: window_id.clone(),
            },
            GridItem::Floating { window_id } => GridItem::Floating {
                window_id: window_id.clone(),
            },
        }
    }
}

impl<W: LayoutElement> GridItem<W> {
    pub fn window_id(&self) -> &W::Id {
        match self {
            GridItem::Column { window_id, .. }
            | GridItem::Tab { window_id, .. }
            | GridItem::Floating { window_id } => window_id,
        }
    }

    pub fn set_column_window_id(&mut self, id: W::Id) {
        if let GridItem::Column { window_id, .. } = self {
            *window_id = id;
        }
    }

    pub fn is_column(&self) -> bool {
        matches!(self, GridItem::Column { .. })
    }

    pub(super) fn same_animation_identity(&self, other: &Self) -> bool {
        match (self, other) {
            (GridItem::Column { window_id: a, .. }, GridItem::Column { window_id: b, .. })
            | (GridItem::Tab { window_id: a, .. }, GridItem::Tab { window_id: b, .. })
            | (GridItem::Floating { window_id: a }, GridItem::Floating { window_id: b }) => a == b,
            _ => false,
        }
    }

    pub(super) fn matches_animation_key(&self, other: &Self) -> bool {
        self.same_animation_identity(other) || self == other
    }

    /// Whether this is the same item showing the same window.
    fn same_item_and_window(&self, other: &Self) -> bool {
        self.same_animation_identity(other) && self == other
    }

    pub(super) fn col_idx(&self) -> Option<usize> {
        match self {
            GridItem::Column { col_idx, .. } | GridItem::Tab { col_idx, .. } => Some(*col_idx),
            GridItem::Floating { .. } => None,
        }
    }
}

/// What the grid lays out for one item.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridSource {
    /// Size of the item on the workspace.
    pub size: Size<f64, Logical>,
    /// For a floating window, its position on the workspace, which the grid keeps it close to.
    pub pos: Option<Point<f64, Logical>>,
}

impl<W: LayoutElement> PartialEq for GridItem<W> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (GridItem::Column { col_idx: a, .. }, GridItem::Column { col_idx: b, .. }) => a == b,
            (GridItem::Tab { window_id: a, .. }, GridItem::Tab { window_id: b, .. }) => a == b,
            (GridItem::Floating { window_id: a }, GridItem::Floating { window_id: b }) => a == b,
            _ => false,
        }
    }
}

#[derive(Debug)]
pub struct GridOverview<W: LayoutElement> {
    pub open: bool,
    pub(super) progress: Option<OverviewProgress>,
    pub layout: GridLayout<W>,
    /// What the layout was last computed from, to skip recomputing an unchanged one.
    layout_inputs: Option<GridLayoutInputs<W>>,
    /// What the grid showed when it last closed on this workspace, for this one to pick up from.
    pub(super) resume_from: Option<GridLayout<W>>,
    /// Index of the focused cell in `layout.entries`.
    pub focus: usize,
    /// Whether `focus` is only the first cell because nothing else was there to focus.
    ///
    /// This is the case when the grid opens while the active window has no cell (it is an
    /// `ignore-grid-overview` floating window, or every window is minimized) and the focus hasn't
    /// moved since. Closing the grid then must not activate that arbitrary first cell.
    pub(super) focus_is_fallback: bool,
    /// The window that `focus` is on.
    ///
    /// The scrolling layout changes underneath the grid (a column to the left closes, the grid
    /// reflows), so `focus` alone, a cell index, would end up on a different window. After every
    /// layout recompute, `focus` is moved back to this window's cell.
    pub(super) focused_window: Option<W::Id>,
    pub saved_active_window_id: Option<W::Id>,
    pub saved_view_offset: f64,
    pub entry_positions: Vec<(GridItem<W>, Point<f64, Logical>)>,
    pub entry_scales: Vec<(GridItem<W>, f64)>,
    pub window_transition_starts: Vec<(W::Id, Point<f64, Logical>, f64)>,
    pub close_start_progress: f64,
    pub rearrange_anim: Option<Animation>,
    /// While a window is interactively grabbed, the focus belongs to the grabbed window itself.
    pub grabbed_window: Option<W::Id>,
    /// Windows currently flying into this grid from another workspace's grid cell.
    ///
    /// This grid renders above the other grids while the fly-in is ongoing, so that the flying
    /// windows stay visible above the source grid they are crossing.
    pub flying_in_windows: Vec<W::Id>,
    added_window_ids: Vec<W::Id>,
    /// Windows restored from a minimized cell during this grid session.
    ///
    /// An `ignore-grid-overview` window only has a cell while it is minimized. Restored from
    /// there, it keeps that cell until the grid opens again: dropping it right away popped the
    /// window out over the grid at its floating position, with no animation, and threw the grid
    /// focus onto some other cell. Kept through the close animation, the cell carries the window
    /// back into place like any other.
    restored_window_ids: Vec<W::Id>,
    /// Filled highlights drawn behind the cells of minimized windows.
    ///
    /// They live here rather than on the tiles because they are sized in screen space: a marker
    /// that scaled with the cell would get thinner the more windows the grid shows.
    minimized_highlights: Vec<MinimizedHighlight<W>>,
    /// Focus rings drawn around the windows of grid cells.
    ///
    /// The grid draws these instead of the windows' own focus rings, which would scale down with
    /// the cells. One ring per window: the focused one, plus, with the grow transition, the ones
    /// still shrinking back on cells that lost the focus.
    focus_rings: Vec<GridFocusRing<W>>,
    /// The window the focus ring was on as of the last update, to tell when the focus moves.
    ring_focus: Option<W::Id>,
    /// With the slide transition, where the ring slides from.
    ring_slide: Option<GridRingSlide>,
    /// Tab indicators next to the cells of tabs, which tell the tabs of one column apart from
    /// other windows.
    tab_indicators: Vec<GridTabIndicator<W>>,
    /// Whether the tab indicators were laid out since the grid opened. The ones that show up
    /// later belong to tabs that joined a column while the grid showed.
    tab_indicators_laid_out: bool,
    /// col_idx → tile_idx for Column items that have multiple tiles, with the window at that
    /// tile, so that the choice can follow its column when column indices shift.
    pub column_tile_focus: Vec<(usize, usize, Option<W::Id>)>,
    pub clock: Clock,
    pub options: Rc<Options>,
}

impl<W: LayoutElement> GridOverview<W> {
    pub fn new(clock: Clock, options: Rc<Options>) -> Self {
        Self {
            open: false,
            progress: None,
            layout: GridLayout::empty(),
            layout_inputs: None,
            resume_from: None,
            focus: 0,
            focus_is_fallback: false,
            focused_window: None,
            saved_active_window_id: None,
            saved_view_offset: 0.,
            entry_positions: Vec::new(),
            entry_scales: Vec::new(),
            window_transition_starts: Vec::new(),
            close_start_progress: 1.,
            rearrange_anim: None,
            grabbed_window: None,
            flying_in_windows: Vec::new(),
            added_window_ids: Vec::new(),
            restored_window_ids: Vec::new(),
            minimized_highlights: Vec::new(),
            focus_rings: Vec::new(),
            ring_focus: None,
            ring_slide: None,
            tab_indicators: Vec::new(),
            tab_indicators_laid_out: false,
            column_tile_focus: Vec::new(),
            clock,
            options,
        }
    }

    /// Picks up a new config.
    ///
    /// The overview is created once and then lives as long as its workspace, so without this it
    /// would keep serving whatever options were in effect when it was first opened: gap, padding,
    /// scales, animations and the minimized highlight would all freeze at that point.
    pub(super) fn update_config(&mut self, options: Rc<Options>) {
        self.options = options;
    }

    pub fn is_fully_open(&self) -> bool {
        self.open && matches!(self.progress, Some(OverviewProgress::Open))
    }

    pub fn progress_value(&self) -> f64 {
        self.progress.as_ref().map_or(0., |p| p.value())
    }

    pub fn is_animation(&self) -> bool {
        self.progress.as_ref().map_or(false, |p| p.is_moving()) || self.rearrange_anim.is_some()
    }

    /// Shows the grid `value` of the way open, following a touchpad gesture.
    pub(super) fn set_gesture_progress(&mut self, value: f64) {
        self.progress = Some(OverviewProgress::Gesture(OverviewGesture {
            tracker: SwipeTracker::new(),
            start: value,
            value,
        }));
    }

    /// Finishes a touchpad gesture, going on from `from` to `to` with the gesture's `velocity`.
    pub(super) fn end_gesture(&mut self, from: f64, to: f64, velocity: f64) {
        self.progress = Some(OverviewProgress::Animation(Animation::new(
            self.clock.clone(),
            from,
            to,
            velocity,
            self.options.animations.grid_overview_open_close.0,
        )));
    }

    pub fn toggle(&mut self) {
        self.open = !self.open;
        self.rearrange_anim = None;
        self.window_transition_starts.clear();
        self.flying_in_windows.clear();

        let from = self.progress.take().map_or(0., |p| p.value());
        let to = if self.open { 1. } else { 0. };
        if self.open {
            self.close_start_progress = 1.;
        } else {
            self.close_start_progress = from.max(0.0001);
        }

        self.progress = Some(OverviewProgress::Animation(Animation::new(
            self.clock.clone(),
            from,
            to,
            0.,
            self.options.animations.grid_overview_open_close.0,
        )));
    }

    /// Reopens the grid from its current closing visual state.
    ///
    /// Unlike [`GridOverview::toggle`], this starts the opening progress from zero while the
    /// caller has already retargeted `entry_positions`/`entry_scales` to the current closing
    /// visuals. Starting from zero keeps the scale math from jumping.
    pub(super) fn reopen(&mut self) {
        self.open = true;
        self.rearrange_anim = None;
        self.window_transition_starts.clear();
        self.flying_in_windows.clear();
        self.close_start_progress = 1.;
        self.progress = Some(OverviewProgress::Animation(Animation::new(
            self.clock.clone(),
            0.,
            1.,
            0.,
            self.options.animations.grid_overview_open_close.0,
        )));
    }

    pub fn compute_layout(
        &mut self,
        items: &[(GridItem<W>, GridSource)],
        area: Rectangle<f64, Logical>,
        restart_rearrange: bool,
    ) -> bool {
        let old_layout = self.layout.clone();
        let inputs = GridLayoutInputs {
            items: items.to_vec(),
            area,
            config: self.options.grid_overview,
        };
        let resumed = self.resume_from.take();
        let new_layout = if self
            .layout_inputs
            .as_ref()
            .is_some_and(|old| old.same(&inputs))
        {
            old_layout.clone()
        } else {
            // The new layout starts from the one the grid shows, or opening again, the one it
            // showed when it closed, so that it keeps its wrap width and scale unless another one
            // is clearly better.
            let prev = if self.open && !old_layout.entries.is_empty() {
                Some(&old_layout)
            } else {
                resumed.as_ref()
            };
            GridLayout::compute_with_axis(
                items,
                area,
                &self.options.grid_overview,
                prev,
                AxisMap::new(self.options.layout.main_axis),
            )
        };
        self.layout_inputs = Some(inputs);
        let new_ids: Vec<_> = new_layout
            .entries
            .iter()
            .map(|(item, _)| item.window_id().clone())
            .collect();
        let progress_value = self.progress_value();
        let rearrange_value = self.rearrange_anim.as_ref().map(|anim| anim.value());
        let opening_retarget_value = if self.open
            && rearrange_value.is_none()
            && !restart_rearrange
            && self.progress.as_ref().is_some_and(|p| p.is_animation())
        {
            Some(progress_value)
        } else {
            None
        };
        // A grid that becomes empty (e.g. its only column was moved to another workspace)
        // keeps an empty layout. When windows are added back into it with an explicit refresh,
        // the fly-in transition must restart the rearrange animation even though the old layout
        // had no entries. Passive recomputes must not start an animation on an empty layout.
        let should_rearrange = self.open && (!old_layout.entries.is_empty() || restart_rearrange);
        let targets_changed = old_layout.entries.len() != new_layout.entries.len()
            || new_layout.entries.iter().any(|(item, info)| {
                Self::matching_old_value(&old_layout.entries, item, &new_ids)
                    .is_none_or(|old_info| !old_info.same_target(info))
            });
        // Retargeting a running animation extrapolates its start point by 1 / (1 - value). Late in
        // the animation, and past its end with an underdamped spring, that blows up the rest of
        // the movement, overshoot included, or makes the cell jump. Restart from the current
        // visual state there instead.
        let late_to_retarget = rearrange_value
            .or(opening_retarget_value)
            .is_some_and(|value| value > 0.9);
        let restart_rearrange =
            should_rearrange && (restart_rearrange || late_to_retarget) && targets_changed;
        let opening_retarget_value = opening_retarget_value.filter(|_| !restart_rearrange);
        if restart_rearrange {
            self.rearrange_anim = Some(Animation::new(
                self.clock.clone(),
                0.,
                1.,
                0.,
                self.options.animations.window_movement.0,
            ));
        }
        self.layout = new_layout;
        let mut new_entries = Vec::new();
        for (item, info) in &self.layout.entries {
            let old_info = Self::matching_old_value(&old_layout.entries, item, &new_ids);
            let entry_pos =
                Self::matching_old_value(&self.entry_positions, item, &new_ids).copied();
            let position_target_unchanged =
                old_info.is_some_and(|old_info| old_info.same_position_target(info));
            let current_pos = match (rearrange_value, entry_pos, old_info) {
                (Some(value), Some(entry), Some(old_info)) => {
                    Self::lerp_point(entry, old_info.target_pos, value)
                }
                (_, entry, Some(old_info)) if self.open => {
                    let entry = entry.unwrap_or(old_info.target_pos);
                    Self::lerp_point(entry, old_info.target_pos, progress_value)
                }
                _ => entry_pos
                    .or_else(|| old_info.map(|old_info| old_info.target_pos))
                    .unwrap_or(info.target_pos),
            };
            let pos = match (rearrange_value, opening_retarget_value, entry_pos, old_info) {
                (_, _, Some(entry), Some(_)) if !restart_rearrange && position_target_unchanged => {
                    entry
                }
                (Some(value), _, Some(_), Some(_)) if !restart_rearrange => {
                    Self::retarget_point(current_pos, info.target_pos, value)
                }
                (None, Some(value), Some(_), Some(_)) => {
                    Self::retarget_point(current_pos, info.target_pos, value)
                }
                _ => current_pos,
            };
            new_entries.push((item.clone(), pos));
        }
        self.entry_positions = new_entries;
        let mut new_scales = Vec::new();
        for (item, info) in &self.layout.entries {
            let old_info = Self::matching_old_value(&old_layout.entries, item, &new_ids);
            let entry_scale = Self::matching_old_value(&self.entry_scales, item, &new_ids).copied();
            let scale_target_unchanged =
                old_info.is_some_and(|old_info| old_info.same_scale_target(info));
            // Like entry_visual_transform(), which this must match: the scale doesn't overshoot.
            let lerp_scale = |entry: f64, target: f64, value: f64| {
                let scale = entry + (target - entry) * value;
                if target < entry {
                    scale.max(target)
                } else {
                    scale.min(target)
                }
            };
            let current_scale = match (rearrange_value, entry_scale, old_info) {
                (Some(value), Some(entry), Some(old_info)) => {
                    lerp_scale(entry, old_info.target_scale, value)
                }
                (_, entry, Some(old_info)) if self.open => {
                    let entry = entry.unwrap_or(1.);
                    lerp_scale(entry, old_info.target_scale, progress_value)
                }
                _ => entry_scale
                    .or_else(|| old_info.map(|old_info| old_info.target_scale))
                    .unwrap_or(if self.open { info.target_scale } else { 1. }),
            };
            let scale = match (
                rearrange_value,
                opening_retarget_value,
                entry_scale,
                old_info,
            ) {
                (_, _, Some(entry), Some(_)) if !restart_rearrange && scale_target_unchanged => {
                    entry
                }
                (Some(value), _, Some(_), Some(_)) if !restart_rearrange => {
                    Self::retarget_value(current_scale, info.target_scale, value)
                }
                (None, Some(value), Some(_), Some(_)) => {
                    Self::retarget_value(current_scale, info.target_scale, value)
                }
                _ => current_scale,
            };
            new_scales.push((item.clone(), scale));
        }
        self.entry_scales = new_scales;
        self.focus = self.focus.min(self.layout.entries.len().saturating_sub(1));

        restart_rearrange
    }

    /// Like [`Self::matching_value`], for matching new layout items against old state.
    ///
    /// A column whose shown window changed has no identity match, so it falls back to the old
    /// entry at the same column index. That fallback only takes old entries whose window is gone
    /// from the new layout: otherwise a column inserted in the middle would take over the old
    /// state of the column that was at its index and still exists further along.
    fn matching_old_value<'a, T>(
        entries: &'a [(GridItem<W>, T)],
        item: &GridItem<W>,
        new_ids: &[W::Id],
    ) -> Option<&'a T> {
        entries
            .iter()
            .find(|(entry, _)| entry.same_animation_identity(item))
            .or_else(|| {
                entries
                    .iter()
                    .find(|(entry, _)| entry == item && !new_ids.contains(entry.window_id()))
            })
            .map(|(_, value)| value)
    }

    fn matching_value<'a, T>(entries: &'a [(GridItem<W>, T)], item: &GridItem<W>) -> Option<&'a T> {
        entries
            .iter()
            .find(|(entry, _)| entry.same_animation_identity(item))
            .or_else(|| entries.iter().find(|(entry, _)| entry == item))
            .map(|(_, value)| value)
    }

    fn lerp_point(
        from: Point<f64, Logical>,
        to: Point<f64, Logical>,
        value: f64,
    ) -> Point<f64, Logical> {
        Point::from((
            from.x + (to.x - from.x) * value,
            from.y + (to.y - from.y) * value,
        ))
    }

    fn retarget_point(
        current: Point<f64, Logical>,
        target: Point<f64, Logical>,
        value: f64,
    ) -> Point<f64, Logical> {
        let remaining = 1. - value;
        if remaining <= 0.0001 {
            current
        } else {
            Point::from((
                (current.x - target.x * value) / remaining,
                (current.y - target.y * value) / remaining,
            ))
        }
    }

    fn retarget_value(current: f64, target: f64, value: f64) -> f64 {
        let remaining = 1. - value;
        if remaining <= 0.0001 {
            current
        } else {
            (current - target * value) / remaining
        }
    }

    pub(super) fn entry_visual_transform(
        &self,
        item: &GridItem<W>,
        info: &GridEntryInfo,
        fallback_pos: Point<f64, Logical>,
    ) -> (Point<f64, Logical>, f64) {
        let p = self.progress_value();
        let is_closing = !self.open;
        let item_source_size = info.target_size.downscale(info.target_scale.max(0.0001));

        if is_closing {
            let close_p = (p / self.close_start_progress.max(0.0001)).clamp(0., 1.);
            let t = 1. - close_p;
            let start_pos = Self::matching_value(&self.entry_positions, item)
                .copied()
                .unwrap_or(info.target_pos);
            let start_scale = Self::matching_value(&self.entry_scales, item)
                .copied()
                .unwrap_or(info.target_scale);
            let start_size = item_source_size.upscale(start_scale);
            let start_center = start_pos + Point::from((start_size.w / 2., start_size.h / 2.));
            let end_center =
                fallback_pos + Point::from((item_source_size.w / 2., item_source_size.h / 2.));
            let center = Point::from((
                start_center.x + (end_center.x - start_center.x) * t,
                start_center.y + (end_center.y - start_center.y) * t,
            ));
            let scale = start_scale + (1. - start_scale) * t;
            let size = item_source_size.upscale(scale);
            let pos = center - Point::from((size.w / 2., size.h / 2.));

            return (pos, scale);
        }

        // Past fully open or fully closed, like a gesture going on or a spring overshooting,
        // the windows stay where they'd be at the end.
        let anim_p = self
            .rearrange_anim
            .as_ref()
            .map_or(p.clamp(0., 1.), |anim| anim.value());
        let is_rearranging = self.rearrange_anim.is_some();
        let is_opening = self.open
            && self
                .progress
                .as_ref()
                .is_some_and(|progress| progress.is_moving());

        let normal_pos = Self::matching_value(&self.entry_positions, item)
            .copied()
            .unwrap_or(fallback_pos);

        let lerp_pos = Point::from((
            normal_pos.x + (info.target_pos.x - normal_pos.x) * anim_p,
            normal_pos.y + (info.target_pos.y - normal_pos.y) * anim_p,
        ));

        let entry_scale = if is_rearranging || is_opening {
            Self::matching_value(&self.entry_scales, item)
                .copied()
                .unwrap_or(1.)
        } else {
            1.
        };

        let final_scale = if info.target_scale < entry_scale {
            (entry_scale + (info.target_scale - entry_scale) * anim_p).max(info.target_scale)
        } else {
            (entry_scale + (info.target_scale - entry_scale) * anim_p).min(info.target_scale)
        };

        (lerp_pos, final_scale)
    }

    pub(super) fn set_window_transition_starts(
        &mut self,
        starts: Vec<(W::Id, Point<f64, Logical>, f64)>,
    ) {
        self.window_transition_starts = starts;
    }

    pub(super) fn window_visual_transform(
        &self,
        window: &W::Id,
        target_pos: Point<f64, Logical>,
        target_scale: f64,
    ) -> (Point<f64, Logical>, f64) {
        let Some(anim) = &self.rearrange_anim else {
            return (target_pos, target_scale);
        };
        let Some((_, start_pos, start_scale)) = self
            .window_transition_starts
            .iter()
            .find(|(id, _, _)| id == window)
        else {
            return (target_pos, target_scale);
        };

        let value = anim.value();
        (
            Self::lerp_point(*start_pos, target_pos, value),
            start_scale + (target_scale - start_scale) * value,
        )
    }

    /// Moves the focus to the cell at `focus`, an index into `layout.entries`.
    pub fn set_focus(&mut self, focus: usize) {
        if self.focus == focus {
            return;
        }
        self.focus_is_fallback = false;
        self.focus = focus;
    }

    pub fn set_grabbed_window(&mut self, id: Option<W::Id>) {
        self.grabbed_window = id;
    }

    pub(super) fn snapshot_close_start_visuals(
        &mut self,
        visuals: Vec<(GridItem<W>, Point<f64, Logical>, f64)>,
    ) {
        self.entry_positions = visuals
            .iter()
            .map(|(item, pos, _)| (item.clone(), *pos))
            .collect();
        self.entry_scales = visuals
            .into_iter()
            .map(|(item, _, scale)| (item, scale))
            .collect();
        self.close_start_progress = self.progress_value().max(0.0001);
    }

    pub fn get_column_tile_focus(&self, col_idx: usize) -> usize {
        self.column_tile_focus
            .iter()
            .find(|(c, _, _)| *c == col_idx)
            .map(|(_, t, _)| *t)
            .unwrap_or(0)
    }

    fn set_column_tile_focus_raw(&mut self, col_idx: usize, tile_idx: usize) {
        self.focus_is_fallback = false;
        if let Some(entry) = self
            .column_tile_focus
            .iter_mut()
            .find(|(c, _, _)| *c == col_idx)
        {
            entry.1 = tile_idx;
            entry.2 = None;
        } else {
            self.column_tile_focus.push((col_idx, tile_idx, None));
        }
    }

    pub fn set_column_tile_focus(&mut self, col_idx: usize, tile_idx: usize, window_id: W::Id) {
        self.set_column_tile_focus_raw(col_idx, tile_idx);
        if let Some(entry) = self
            .column_tile_focus
            .iter_mut()
            .find(|(c, _, _)| *c == col_idx)
        {
            entry.2 = Some(window_id.clone());
        }
        // Update the window_id in the layout entry.
        for (item, _) in &mut self.layout.entries {
            if let GridItem::Column {
                col_idx: c,
                window_id: wid,
                ..
            } = item
            {
                if *c == col_idx {
                    *wid = window_id.clone();
                }
            }
        }
    }

    pub fn navigate(
        &mut self,
        dir: GridDirection,
        get_tile_count: impl Fn(usize) -> usize,
    ) -> Option<(usize, usize)> {
        if self.layout.entries.is_empty() {
            return None;
        }

        // Up and down first go through the tiles of a column that shows several.
        let current_col_idx = self.layout.entries.get(self.focus).and_then(|(item, _)| {
            if let GridItem::Column { col_idx, .. } = item {
                Some(*col_idx)
            } else {
                None
            }
        });

        if let Some(col_idx) = current_col_idx {
            let current = self.get_column_tile_focus(col_idx);
            let new_tile_idx = match dir {
                GridDirection::Up => current.checked_sub(1),
                GridDirection::Down => {
                    (current + 1 < get_tile_count(col_idx)).then_some(current + 1)
                }
                _ => None,
            };
            if let Some(new_tile_idx) = new_tile_idx {
                self.set_column_tile_focus_raw(col_idx, new_tile_idx);
                return Some((col_idx, new_tile_idx));
            }
        }

        if let Some(idx) = self.layout.neighbor(self.focus, dir) {
            self.set_focus(idx);
        }
        None
    }

    pub fn focused_id(&self) -> Option<W::Id> {
        // For Column items, the window_id is kept in sync by set_column_tile_focus to match the
        // currently focused tile.
        self.focused_item().map(|item| item.window_id().clone())
    }

    pub fn focused_item(&self) -> Option<&GridItem<W>> {
        self.layout.entries.get(self.focus).map(|(item, _)| item)
    }

    pub fn set_focused_window_id(&mut self, id: W::Id) {
        if let Some((item, _)) = self.layout.entries.get_mut(self.focus) {
            item.set_column_window_id(id);
        }
    }

    /// Updates the highlights drawn behind minimized cells.
    ///
    /// `entries` carries the already-padded on-screen size of every minimized window's cell and
    /// whether that window is urgent. Rings are kept across frames so their buffers (and with
    /// them, damage tracking) stay stable.
    pub(super) fn update_minimized_highlights(
        &mut self,
        entries: &[MinimizedHighlightEntry<W>],
        scale: f64,
    ) {
        let config = self.options.grid_overview.minimized_highlight;
        if config.off || entries.is_empty() {
            self.minimized_highlights.clear();
            return;
        }

        // The marker fades in and out with the grid, like the minimized thumbnails themselves.
        let grid_alpha = self.progress_value().clamp(0., 1.);
        // An easing, not the window_movement spring: a spring starts from rest, so the first
        // ~200ms sit under 6% opacity and the frame reads as popping in late. An ease-out moves
        // right away and settles, which is what a fade should feel like. Global animation
        // settings still apply: `off` here, and slowdown through the clock.
        let fade_config = niri_config::Animation {
            off: self.options.animations.off,
            kind: niri_config::animations::Kind::Easing(niri_config::animations::EasingParams {
                duration_ms: 200,
                curve: niri_config::animations::Curve::EaseOutQuad,
            }),
        };

        let mut old = std::mem::take(&mut self.minimized_highlights);
        for entry in entries {
            let id = &entry.window;
            let target = if entry.is_minimized { 1. } else { 0. };

            let mut highlight = match old.iter().position(|h| &h.window == id) {
                Some(idx) => old.swap_remove(idx),
                // Nothing to fade out for a window that never had a highlight.
                None if target == 0. => continue,
                None => MinimizedHighlight {
                    window: id.clone(),
                    ring: FocusRing::new(niri_config::FocusRing {
                        off: false,
                        width: 0.,
                        active_gradient: None,
                        urgent_gradient: None,
                        ..Default::default()
                    }),
                    dim: FocusRing::new(niri_config::FocusRing {
                        off: false,
                        width: 0.,
                        active_color: niri_config::Color::new_unpremul(0., 0., 0., 1.),
                        active_gradient: None,
                        urgent_gradient: None,
                        ..Default::default()
                    }),
                    dim_loc: None,
                    fade: Animation::new(self.clock.clone(), 0., 1., 0., fade_config),
                },
            };

            // Minimizing or restoring while the grid is up would otherwise pop the frame in and
            // out; fade it instead, over the same animation that carries the cells themselves.
            if highlight.fade.to() != target {
                highlight.fade =
                    highlight
                        .fade
                        .restarted(highlight.fade.clamped_value(), target, 0.);
            }

            let fade = highlight.fade.clamped_value().clamp(0., 1.);
            if target == 0. && (fade <= 0.0001 || highlight.fade.is_clamped_done()) {
                continue;
            }

            // Concentric with the window's rounded corners unless the config sets a radius, so
            // that the frame's corners follow the focus ring's.
            let frame_radius = match config.corner_radius {
                Some(radius) => CornerRadius::from(radius as f32),
                None => entry
                    .tile_rect
                    .map_or(CornerRadius::default(), |(_, radius)| {
                        radius.expanded_by(config.padding as f32)
                    }),
            };
            let mut ring_config = *highlight.ring.config();
            ring_config.active_color = config.color;
            ring_config.urgent_color = config.urgent_color;
            highlight.ring.update_config(ring_config);
            highlight.ring.update_render_elements(
                entry.size,
                true,
                false,
                entry.is_urgent,
                Rectangle::default(),
                frame_radius,
                scale,
                (grid_alpha * fade) as f32,
            );

            highlight.dim_loc = None;
            if config.dim > 0. {
                if let Some((rect, radius)) = entry.tile_rect {
                    highlight.dim.update_render_elements(
                        rect.size,
                        true,
                        false,
                        false,
                        Rectangle::default(),
                        radius,
                        scale,
                        (config.dim * grid_alpha * fade) as f32,
                    );
                    highlight.dim_loc = Some(rect.loc);
                }
            }

            self.minimized_highlights.push(highlight);
        }
    }

    fn minimized_highlights_animating(&self) -> bool {
        self.minimized_highlights
            .iter()
            .any(|h| !h.fade.is_clamped_done())
    }

    pub(super) fn render_minimized_highlight(
        &self,
        renderer: &mut impl NiriRenderer,
        window: &W::Id,
        location: Point<f64, Logical>,
        push: &mut dyn FnMut(FocusRingRenderElement),
    ) {
        let Some(highlight) = self
            .minimized_highlights
            .iter()
            .find(|h| &h.window == window)
        else {
            return;
        };

        highlight.ring.render(renderer, location, push);
    }

    /// Renders the dark layer over a minimized window's thumbnail.
    pub(super) fn render_minimized_dim(
        &self,
        renderer: &mut impl NiriRenderer,
        window: &W::Id,
        push: &mut dyn FnMut(FocusRingRenderElement),
    ) {
        let Some(highlight) = self
            .minimized_highlights
            .iter()
            .find(|h| &h.window == window)
        else {
            return;
        };

        if let Some(loc) = highlight.dim_loc {
            highlight.dim.render(renderer, loc, push);
        }
    }

    /// Windows that have a focus ring in the grid.
    pub(super) fn focus_ring_windows(&self) -> impl Iterator<Item = &W::Id> {
        self.focus_rings.iter().map(|ring| &ring.window)
    }

    /// Updates the focus rings drawn around grid cells.
    ///
    /// `focus` is the focused window, if it has a ring, and `targets` has the current geometry of
    /// it and of every window from [`Self::focus_ring_windows`] still in the grid.
    pub(super) fn update_focus_rings(
        &mut self,
        focus: Option<&W::Id>,
        targets: &[GridRingTarget<W>],
        is_active: bool,
        view_size: Size<f64, Logical>,
        scale: f64,
    ) {
        let transition = self.options.grid_overview.focus_ring_transition;
        let anim_config = self.options.animations.focus_ring.0;
        let grid_width = self.options.grid_overview.focus_ring_width;
        let progress = self.progress_value().clamp(0., 1.);
        let target_of = |id: &W::Id| targets.iter().find(|t| &t.window == id);

        // Rings only animate when the focus moves from one cell to another, not when a ring first
        // shows up as the grid opens.
        let focus_moved =
            self.ring_focus.is_some() && focus.is_some() && self.ring_focus.as_ref() != focus;
        self.ring_focus = focus.cloned();

        let clock = self.clock.clone();
        let new_ring = |window: &W::Id| GridFocusRing {
            window: window.clone(),
            ring: FocusRing::new(niri_config::FocusRing::default()),
            shown: 1.,
            fade: None,
            rect: None,
        };

        match transition {
            GridFocusRingTransition::None | GridFocusRingTransition::Slide => {
                // One ring, on the focused window. Keep its buffers when it moves.
                self.focus_rings.truncate(1);
                match (focus, self.focus_rings.first_mut()) {
                    (Some(focus), Some(ring)) => {
                        ring.window = focus.clone();
                        ring.shown = 1.;
                        ring.fade = None;
                    }
                    (Some(focus), None) => self.focus_rings.push(new_ring(focus)),
                    (None, _) => self.focus_rings.clear(),
                }

                if transition == GridFocusRingTransition::Slide && focus_moved {
                    // Slide from where the ring is drawn right now, which is somewhere along the
                    // way if a slide is ongoing.
                    let from = self.focus_rings.first().and_then(|ring| ring.rect);
                    self.ring_slide = from.map(|(rect, radius)| GridRingSlide {
                        from: rect,
                        from_radius: radius,
                        anim: Animation::new(clock.clone(), 0., 1., 0., anim_config),
                    });
                } else if transition == GridFocusRingTransition::None {
                    self.ring_slide = None;
                }
            }
            GridFocusRingTransition::Grow => {
                self.ring_slide = None;

                for ring in &mut self.focus_rings {
                    let target = if Some(&ring.window) == focus { 1. } else { 0. };
                    if ring.shown != target {
                        // Start from wherever an ongoing fade has got to.
                        let from = ring.shown_value();
                        ring.shown = target;
                        ring.fade =
                            Some(Animation::new(clock.clone(), from, target, 0., anim_config));
                    }
                }
                if let Some(focus) = focus {
                    if !self.focus_rings.iter().any(|ring| &ring.window == focus) {
                        let mut ring = new_ring(focus);
                        if focus_moved {
                            ring.fade =
                                Some(Animation::new(clock.clone(), 0., 1., 0., anim_config));
                        }
                        self.focus_rings.push(ring);
                    }
                }
                self.drop_hidden_focus_rings();
            }
        }

        let now = self.clock.now();
        let slide = self
            .ring_slide
            .as_ref()
            .map(|slide| (slide, slide_progress(&slide.anim, now)));

        for ring in &mut self.focus_rings {
            let Some(target) = target_of(&ring.window) else {
                ring.rect = None;
                continue;
            };

            // A spring can grow the ring a little past its width, but never past full opacity.
            let grown = ring.shown_value();
            let shown = grown.clamp(0., 1.);
            let normal_width = target.config.width;
            let full_width = grid_width.map_or(normal_width, |grid_width| {
                normal_width + (grid_width - normal_width) * progress
            });
            let width = full_width * grown.max(0.);

            let (rect, radius) = match slide {
                Some((slide, (lead, trail))) => (
                    stretch_rect(slide.from, target.rect, lead, trail),
                    lerp_radius(
                        slide.from_radius,
                        target.radius,
                        ((lead + trail) / 2.) as f32,
                    ),
                ),
                None => (target.rect, target.radius),
            };

            let mut config = target.config;
            config.off = false;
            config.width = width;
            ring.ring.update_config(config);
            // Relative to the ring, for gradients relative to the workspace view.
            let view_rect = Rectangle::new(Point::from((-rect.loc.x, -rect.loc.y)), view_size);
            ring.ring.update_render_elements(
                rect.size,
                is_active,
                true,
                target.is_urgent,
                view_rect,
                radius.expanded_by(width as f32),
                scale,
                (shown * target.alpha) as f32,
            );
            ring.rect = Some((rect, radius));
        }
    }

    fn focus_rings_animating(&self) -> bool {
        self.ring_slide.is_some() || self.focus_rings.iter().any(|ring| ring.fade.is_some())
    }

    fn tab_indicators_animating(&self) -> bool {
        self.tab_indicators
            .iter()
            .any(|tab| tab.indicator.are_animations_ongoing())
    }

    fn advance_focus_rings(&mut self) {
        for tab in &mut self.tab_indicators {
            tab.indicator.advance_animations();
        }
        self.tab_indicators
            .retain(|tab| !(tab.leaving && tab.indicator.is_closed()));

        for ring in &mut self.focus_rings {
            if ring.fade.as_ref().is_some_and(|fade| fade.is_done()) {
                ring.fade = None;
            }
        }
        self.drop_hidden_focus_rings();

        if self
            .ring_slide
            .as_ref()
            .is_some_and(|slide| slide.anim.is_done())
        {
            self.ring_slide = None;
        }
    }

    /// Drops the rings that shrank all the way back.
    fn drop_hidden_focus_rings(&mut self) {
        self.focus_rings
            .retain(|ring| ring.shown > 0. || ring.fade.is_some());
    }

    /// Renders the focus rings of the grid cells.
    pub(super) fn render_focus_rings(
        &self,
        renderer: &mut impl NiriRenderer,
        push: &mut dyn FnMut(FocusRingRenderElement),
    ) {
        for ring in &self.focus_rings {
            if let Some((rect, _)) = ring.rect {
                ring.ring.render(renderer, rect.loc, push);
            }
        }
    }

    /// Updates the tab indicators next to the cells of tabs.
    ///
    /// `leaving` has the current on-screen rectangle of windows that have an indicator but are
    /// no longer tabs: theirs folds away there.
    pub(super) fn update_tab_indicators(
        &mut self,
        targets: Vec<GridTabTarget<W>>,
        leaving: &[(W::Id, Rectangle<f64, Logical>)],
        is_active: bool,
        view_size: Size<f64, Logical>,
        scale: f64,
    ) {
        // They fade in as the grid opens.
        let alpha = self.progress_value().clamp(0., 1.) as f32;

        // A tab that joins a column while the grid shows has its indicator grow in, like a
        // column's own indicator when it turns tabbed. The ones there as the grid opens fade in
        // with it instead.
        let grows_in = self.open && self.tab_indicators_laid_out;
        self.tab_indicators_laid_out = self.open;
        let movement = self.options.animations.window_movement.0;

        let mut old = std::mem::take(&mut self.tab_indicators);
        for target in targets {
            let mut tab = match old.iter().position(|t| t.window == target.window) {
                Some(idx) => old.swap_remove(idx),
                None => GridTabIndicator {
                    window: target.window.clone(),
                    indicator: TabIndicator::new(target.config),
                    slot: FocusRing::new(niri_config::FocusRing {
                        off: false,
                        width: 0.,
                        active_gradient: None,
                        urgent_gradient: None,
                        ..Default::default()
                    }),
                    slot_loc: None,
                    last_tabs: Vec::new(),
                    leaving: false,
                },
            };
            let is_new = tab.slot_loc.is_none() && tab.indicator.bounds().is_none();
            let indicator = &mut tab.indicator;
            if (is_new && grows_in) || tab.leaving {
                indicator.start_open_animation(self.clock.clone(), movement);
            }
            tab.leaving = false;
            tab.last_tabs.clone_from(&target.tabs);
            indicator.set_count_animation(self.clock.clone(), movement);
            indicator.update_config(target.config);
            indicator.set_alpha(alpha);
            // Relative to the tabs area, for gradients relative to the workspace view.
            let view_rect = Rectangle::new(
                Point::from((-target.area.loc.x, -target.area.loc.y)),
                view_size,
            );
            let count = target.tabs.len();
            indicator.update_render_elements(
                true,
                target.area,
                view_rect,
                count,
                target.tabs.into_iter(),
                is_active,
                scale,
            );

            // A slot in the background color around the indicator, a pixel wider on every side.
            // On a focused cell it cuts into the focus ring, so that the lit tab, which has the
            // ring's colors, doesn't run into it.
            tab.slot_loc = None;
            if let Some(bounds) = tab.indicator.bounds() {
                let mut slot_config = *tab.slot.config();
                slot_config.active_color = target.slot_color;
                tab.slot.update_config(slot_config);
                let size = bounds.size + Size::from((2., 2.));
                tab.slot.update_render_elements(
                    size,
                    true,
                    false,
                    false,
                    Rectangle::default(),
                    CornerRadius::from(target.config.corner_radius as f32 + 1.),
                    scale,
                    alpha,
                );
                tab.slot_loc = Some(bounds.loc - Point::from((1., 1.)));
            }

            self.tab_indicators.push(tab);
        }

        // A window that stopped being a tab folds its indicator away where its cell is now, with
        // the tabs it last had.
        for mut tab in old {
            let Some((_, area)) = leaving.iter().find(|(id, _)| id == &tab.window) else {
                continue;
            };
            if !self.open || tab.indicator.is_closed() {
                continue;
            }
            if !tab.leaving {
                tab.leaving = true;
                tab.indicator
                    .start_close_animation(self.clock.clone(), movement);
            }

            tab.indicator.set_alpha(alpha);
            let view_rect = Rectangle::new(Point::from((-area.loc.x, -area.loc.y)), view_size);
            let tabs = tab.last_tabs.clone();
            tab.indicator.update_render_elements(
                true,
                *area,
                view_rect,
                tabs.len(),
                tabs.into_iter(),
                is_active,
                scale,
            );
            tab.slot_loc = None;
            if let Some(bounds) = tab.indicator.bounds() {
                tab.slot.update_render_elements(
                    bounds.size + Size::from((2., 2.)),
                    true,
                    false,
                    false,
                    Rectangle::default(),
                    CornerRadius::from(tab.indicator.config().corner_radius as f32 + 1.),
                    scale,
                    alpha,
                );
                tab.slot_loc = Some(bounds.loc - Point::from((1., 1.)));
            }
            self.tab_indicators.push(tab);
        }
    }

    /// Windows whose cell has a tab indicator, including ones folding away.
    pub(super) fn tab_indicator_windows(&self) -> impl Iterator<Item = &W::Id> {
        self.tab_indicators.iter().map(|t| &t.window)
    }

    /// Renders the tab indicators next to the cells of tabs.
    pub(super) fn render_tab_indicators(
        &self,
        renderer: &mut impl NiriRenderer,
        push: &mut dyn FnMut(TabIndicatorRenderElement),
    ) {
        for tab in &self.tab_indicators {
            tab.indicator.render(renderer, Point::default(), push);
        }
    }

    /// Renders the slots under the tab indicators, see [`Self::update_tab_indicators`].
    pub(super) fn render_tab_indicator_slots(
        &self,
        renderer: &mut impl NiriRenderer,
        push: &mut dyn FnMut(FocusRingRenderElement),
    ) {
        for tab in &self.tab_indicators {
            if let Some(loc) = tab.slot_loc {
                tab.slot.render(renderer, loc, push);
            }
        }
    }

    pub(super) fn record_added_window(&mut self, id: W::Id) {
        if !self.added_window_ids.contains(&id) {
            self.added_window_ids.push(id);
        }
    }

    pub(super) fn clear_added_windows(&mut self) {
        self.added_window_ids.clear();
    }

    pub(super) fn window_was_added_while_open(&self, id: &W::Id) -> bool {
        self.added_window_ids.contains(id)
    }

    pub(super) fn record_restored_window(&mut self, id: W::Id) {
        if !self.restored_window_ids.contains(&id) {
            self.restored_window_ids.push(id);
        }
    }

    pub(super) fn clear_restored_windows(&mut self) {
        self.restored_window_ids.clear();
    }

    pub(super) fn window_was_restored(&self, id: &W::Id) -> bool {
        self.restored_window_ids.contains(id)
    }

    pub fn focused_info(&self) -> Option<&GridEntryInfo> {
        self.layout.entries.get(self.focus).map(|(_, info)| info)
    }

    pub fn find_grid_info(&self, id: &W::Id) -> Option<&GridEntryInfo> {
        self.layout.entries.iter().find_map(|(item, info)| {
            if item.window_id() == id {
                Some(info)
            } else {
                None
            }
        })
    }

    /// Index of the cell showing the window `id`.
    pub fn find_grid_index(&self, id: &W::Id) -> Option<usize> {
        self.layout
            .entries
            .iter()
            .position(|(item, _)| item.window_id() == id)
    }

    /// Index of the cell of `item`.
    pub fn find_grid_index_for_item(&self, item: &GridItem<W>) -> Option<usize> {
        self.layout
            .entries
            .iter()
            .position(|(entry, _)| entry == item)
    }

    pub fn advance_animations(&mut self) {
        self.advance_focus_rings();
        if let Some(OverviewProgress::Animation(anim)) = &mut self.progress {
            if anim.is_done() {
                self.progress = if self.open {
                    Some(OverviewProgress::Open)
                } else {
                    None
                };
                if self.open && self.rearrange_anim.is_none() {
                    self.sync_entries_to_layout();
                }
            }
        }
        if let Some(anim) = &mut self.rearrange_anim {
            if anim.is_done() {
                self.rearrange_anim = None;
                self.window_transition_starts.clear();
                self.flying_in_windows.clear();
                self.sync_entries_to_layout();
            }
        }
    }

    fn sync_entries_to_layout(&mut self) {
        self.entry_positions = self
            .layout
            .entries
            .iter()
            .map(|(item, info)| (item.clone(), info.target_pos))
            .collect();
        self.entry_scales = self
            .layout
            .entries
            .iter()
            .map(|(item, info)| (item.clone(), info.target_scale))
            .collect();
        self.window_transition_starts.clear();
        self.flying_in_windows.clear();
    }

    /// What the grid shows, for the next grid on this workspace to start from.
    pub(super) fn save_layout(self) -> GridLayout<W> {
        self.layout
    }

    pub fn are_animations_ongoing(&self) -> bool {
        self.progress.as_ref().map_or(false, |p| p.is_animation())
            || self.rearrange_anim.is_some()
            || self.minimized_highlights_animating()
            || self.focus_rings_animating()
            || self.tab_indicators_animating()
    }
}

#[derive(Debug)]
pub struct GridLayout<W: LayoutElement> {
    /// The scale the windows are laid out at. With tiled windows, it applies to their sizes
    /// squeezed towards each other's, so a window shows at its own scale.
    pub scale: f64,
    /// The width the rows of tiled windows wrap at, as a fraction of the grid's width.
    pub wrap: f64,
    pub gap: f64,
    pub entries: Vec<(GridItem<W>, GridEntryInfo)>,
    /// How the floating windows go around the tiled ones; `None` without tiled windows.
    arrangement: Option<Arrangement>,
    /// The lines of the floating windows in the bands around the tiled ones.
    lines: Vec<BandLine<W>>,
    /// Without tiled windows, the row every floating window is in.
    float_rows: Vec<(W::Id, usize)>,
}

impl<W: LayoutElement> GridLayout<W> {
    /// The same layout, expressed in transposed coordinates (main and cross axis swapped).
    fn transposed(&self, axis: AxisMap) -> Self {
        let mut layout = self.clone();
        layout.entries = layout
            .entries
            .into_iter()
            .map(|(item, info)| {
                (
                    item,
                    GridEntryInfo {
                        target_pos: axis.point_in(info.target_pos),
                        target_size: axis.size_in(info.target_size),
                        ..info
                    },
                )
            })
            .collect();
        layout.lines.clear();
        layout.arrangement = None;
        layout
    }
}

impl<W: LayoutElement> Clone for GridLayout<W> {
    fn clone(&self) -> Self {
        Self {
            scale: self.scale,
            wrap: self.wrap,
            gap: self.gap,
            entries: self.entries.clone(),
            arrangement: self.arrangement,
            lines: self.lines.clone(),
            float_rows: self.float_rows.clone(),
        }
    }
}

/// What a [`GridLayout`] was computed from.
#[derive(Debug)]
struct GridLayoutInputs<W: LayoutElement> {
    items: Vec<(GridItem<W>, GridSource)>,
    area: Rectangle<f64, Logical>,
    config: niri_config::GridOverview,
}

impl<W: LayoutElement> GridLayoutInputs<W> {
    fn same(&self, other: &Self) -> bool {
        self.area == other.area
            && self.config == other.config
            && self.items.len() == other.items.len()
            && self
                .items
                .iter()
                .zip(&other.items)
                .all(|((a, a_source), (b, b_source))| {
                    a.same_item_and_window(b) && a_source == b_source
                })
    }
}

/// Widths, as fractions of the grid's width, that the rows of tiled windows may wrap at. The
/// layout takes the one that shows the windows the largest.
const WRAP_WIDTHS: [f64; 5] = [1., 0.85, 0.7, 0.55, 0.42];
/// The grid keeps the wrap width and the arrangement of the bands around the tiled windows, or the
/// rows of floating windows only, it had as long as that shows the windows at least this fraction
/// of the largest the others could.
const KEEP_ARRANGEMENT_SCALE: f64 = 1. / 1.18;
/// The grid keeps the scale it had as long as the windows could only grow by less than this.
const SCALE_GROW_HYSTERESIS: f64 = 1. / 0.93;
/// Windows are never shown larger than they are.
const MAX_SCALE: f64 = 1.;
const EPSILON: f64 = 0.000001;

impl<W: LayoutElement> GridLayout<W> {
    pub fn empty() -> Self {
        Self {
            scale: 1.,
            wrap: 1.,
            gap: 0.,
            entries: Vec::new(),
            arrangement: None,
            lines: Vec::new(),
            float_rows: Vec::new(),
        }
    }

    /// Lays out `items`: the tiled ones in strip order, then the floating ones.
    ///
    /// Every window is shown at one scale, the largest at which they all fit. Tiled windows form
    /// a block in the middle in strip order, wrapped into rows, with the tabs of a column kept in
    /// one row. Floating windows start out where they are on the workspace, and push each other
    /// and the block apart until nothing overlaps; one that is in a gap the block leaves, like
    /// beside a short row, stays there. With `prev`, the layout shown before, the new one keeps
    /// its wrap width and scale unless another one is clearly better.
    pub fn compute(
        items: &[(GridItem<W>, GridSource)],
        area: Rectangle<f64, Logical>,
        config: &niri_config::GridOverview,
        prev: Option<&GridLayout<W>>,
    ) -> Self {
        Self::compute_with_axis(
            items,
            area,
            config,
            prev,
            AxisMap::new(niri_config::MainAxis::Horizontal),
        )
    }

    /// Lay the grid out for a given main axis.
    ///
    /// With a vertical main axis everything is mirrored: the grid is computed in scrolling space
    /// (X = main), so rows wrap by what is physically the height, and the result is transposed
    /// back. With a horizontal axis this is exactly [`Self::compute`].
    pub fn compute_with_axis(
        items: &[(GridItem<W>, GridSource)],
        area: Rectangle<f64, Logical>,
        config: &niri_config::GridOverview,
        prev: Option<&GridLayout<W>>,
        axis: AxisMap,
    ) -> Self {
        if !axis.is_vertical() {
            return Self::compute_inner(items, area, config, prev);
        }

        let items: Vec<_> = items
            .iter()
            .map(|(item, source)| {
                (
                    item.clone(),
                    GridSource {
                        size: axis.size_in(source.size),
                        pos: source.pos.map(|pos| axis.point_in(pos)),
                    },
                )
            })
            .collect();
        let area = axis.rect_in(area);
        let prev = prev.map(|prev| prev.transposed(axis));

        let mut layout = Self::compute_inner(&items, area, config, prev.as_ref());
        layout.entries = layout
            .entries
            .into_iter()
            .map(|(item, info)| {
                (
                    item,
                    GridEntryInfo {
                        target_pos: axis.point_out(info.target_pos),
                        target_size: axis.size_out(info.target_size),
                        ..info
                    },
                )
            })
            .collect();
        // The floating lines and the arrangement describe the transposed computation; drop them
        // so a later computation starts from the entries alone.
        layout.lines.clear();
        layout.arrangement = None;
        layout
    }

    fn compute_inner(
        items: &[(GridItem<W>, GridSource)],
        area: Rectangle<f64, Logical>,
        config: &niri_config::GridOverview,
        prev: Option<&GridLayout<W>>,
    ) -> Self {
        if items.is_empty() {
            return Self::empty();
        }

        let padding = config.padding;
        let content = Rectangle::new(
            Point::from((area.loc.x + padding.left, area.loc.y + padding.top)),
            Size::from((
                (area.size.w - padding.left - padding.right).max(1.),
                (area.size.h - padding.top - padding.bottom).max(1.),
            )),
        );
        // A gap too large for the grid would leave no room for the windows. Shrink it so that a
        // line of all the items still has some.
        let gap = config
            .gap
            .max(0.)
            .min(content.size.w.min(content.size.h) / (items.len() as f64 + 1.));

        let (groups, floating) = columns_and_floating(items);
        let min_scale = config.min_scale.clamp(0.001, MAX_SCALE);
        if !groups.is_empty() {
            let bands = Bands::new(items, area, content, gap, &groups, &floating);
            return Self::compute_bands(items, &bands, gap, min_scale, prev);
        }

        let rows = Rows::new(items, area, content, gap);
        Self::compute_rows(items, &rows, gap, min_scale, prev)
    }

    /// Lays out a grid with floating windows only, in rows.
    ///
    /// Of every number of rows, the one that shows the windows the largest wins. With `prev`,
    /// the layout shown before, the windows stay in the rows they were in unless starting over
    /// shows them clearly larger, and the scale stays unless the windows could grow by more than
    /// a little.
    fn compute_rows(
        items: &[(GridItem<W>, GridSource)],
        rows: &Rows<W>,
        gap: f64,
        min_scale: f64,
        prev: Option<&GridLayout<W>>,
    ) -> Self {
        let mut best: Option<(Vec<Vec<usize>>, f64)> = None;
        for count in 1..=items.len().min(MAX_FLOATING_ROWS) {
            let fresh = rows.fresh(count);
            let scale = rows.max_scale(&fresh, min_scale);
            if best.as_ref().is_none_or(|(_, b)| scale > b + EPSILON) {
                best = Some((fresh, scale));
            }
        }
        let (mut chosen, mut scale) = best.unwrap();

        // The rows the windows were in, or a small change to them, rather than new rows.
        if let Some(prev) = prev {
            if let Some(kept) = rows.kept(&prev.float_rows, min_scale) {
                let mut changes = rows.changes(kept).into_iter();
                let kept = changes.next().unwrap();
                let mut near = (rows.max_scale(&kept, min_scale), kept);
                for change in changes {
                    let change_scale = rows.max_scale(&change, min_scale);
                    // Only a change that shows the windows clearly larger.
                    if change_scale * KEEP_ARRANGEMENT_SCALE > near.0 {
                        near = (change_scale, change);
                    }
                }
                let (kept_scale, kept) = near;
                if kept_scale >= scale * KEEP_ARRANGEMENT_SCALE {
                    chosen = kept;
                    scale = kept_scale;
                    if prev.scale <= kept_scale && kept_scale <= prev.scale * SCALE_GROW_HYSTERESIS
                    {
                        scale = prev.scale;
                    }
                }
            }
        }

        let placed = rows.place(&chosen, scale);
        let entries = items
            .iter()
            .zip(placed)
            .enumerate()
            .map(|(idx, ((item, _), rect))| {
                let info = GridEntryInfo {
                    row: None,
                    target_pos: rect.loc,
                    target_size: rect.size,
                    target_scale: rows.shown_scale(idx, scale),
                };
                (item.clone(), info)
            })
            .collect();

        Self {
            scale,
            wrap: 1.,
            gap,
            entries,
            arrangement: None,
            lines: Vec::new(),
            float_rows: rows.ids(&chosen),
        }
    }

    /// Lays out a grid with tiled windows: the tiled windows in a block in the middle, the
    /// floating ones in bands around it, everything at sizes squeezed towards each other's.
    ///
    /// Of every wrap width and every arrangement of the bands, the one that shows the windows the
    /// largest wins. With `prev`, the layout shown before, the new one keeps its wrap width and
    /// arrangement unless another one is clearly better, keeps its scale unless the windows
    /// could grow by more than a little, and keeps the floating windows in the lines of the
    /// bands they were in unless starting over is clearly better.
    fn compute_bands(
        items: &[(GridItem<W>, GridSource)],
        bands: &Bands<W>,
        gap: f64,
        min_scale: f64,
        prev: Option<&GridLayout<W>>,
    ) -> Self {
        let prev_choice = prev.and_then(|p| p.arrangement.map(|a| (p.wrap, a, p.scale)));
        let run = |kept: Option<&[BandLine<W>]>| {
            // The lines a floating window was in only count for the arrangement they were in.
            let kept_for = |arrangement: Arrangement| {
                kept.filter(|_| prev_choice.is_some_and(|(_, a, _)| a == arrangement))
            };
            let fits = |wrap: f64, arrangement: Arrangement, scale: f64| {
                bands
                    .place(wrap, scale, arrangement, kept_for(arrangement), false)
                    .is_some()
            };

            let mut results = Vec::new();
            for wrap in WRAP_WIDTHS {
                for arrangement in ARRANGEMENTS {
                    let (yes, no) = if fits(wrap, arrangement, MAX_SCALE) {
                        (MAX_SCALE, MAX_SCALE)
                    } else if !fits(wrap, arrangement, min_scale) {
                        (min_scale, min_scale)
                    } else {
                        bisect(min_scale, MAX_SCALE, 16, |s| fits(wrap, arrangement, s))
                    };
                    results.push((wrap, arrangement, yes, no));
                }
            }
            let mut best = results[0];
            for &result in &results {
                if result.2 > best.2 + EPSILON {
                    best = result;
                }
            }

            let mut choice = best;
            let mut kept_scale = false;
            if let Some((wrap, arrangement, scale)) = prev_choice {
                if let Some(&k) = results.iter().find(|r| {
                    r.0 == wrap && r.1 == arrangement && r.2 >= best.2 * KEEP_ARRANGEMENT_SCALE
                }) {
                    choice = k;
                }
                // The scale shown before, as long as it still fits: the rough largest scale is
                // only good to compare with.
                if choice.0 == wrap
                    && choice.1 == arrangement
                    && scale <= choice.3
                    && choice.2 <= scale * SCALE_GROW_HYSTERESIS
                    && fits(wrap, arrangement, scale)
                {
                    choice.2 = scale;
                    kept_scale = true;
                }
            }
            let (wrap, arrangement, mut scale, no) = choice;
            if !kept_scale && no > scale {
                scale = bisect(scale, no, 14, |s| fits(wrap, arrangement, s)).0;
            }
            (wrap, arrangement, scale, kept_for(arrangement).is_some())
        };

        let fresh = run(None);
        let prev_lines = prev.map(|p| &p.lines[..]).filter(|lines| !lines.is_empty());
        let (wrap, arrangement, scale, uses_kept) = match prev_lines {
            Some(lines) => {
                let kept = run(Some(lines));
                if fresh.2 > kept.2 * SCALE_GROW_HYSTERESIS {
                    fresh
                } else {
                    kept
                }
            }
            None => fresh,
        };
        let kept = prev_lines.filter(|_| uses_kept);

        let placed = bands
            .place(wrap, scale, arrangement, kept, false)
            .or_else(|| bands.place(wrap, scale, arrangement, kept, true))
            .unwrap();

        // Every window shows at its own scale, never larger than it is.
        let entries = items
            .iter()
            .zip(placed.slots)
            .enumerate()
            .map(|(idx, ((item, source), (slot, row)))| {
                let shown = bands.shown_scale(idx, scale);
                let info = GridEntryInfo {
                    row,
                    target_pos: slot.loc,
                    target_size: source.size.upscale(shown),
                    target_scale: shown,
                };
                (item.clone(), info)
            })
            .collect();

        Self {
            scale,
            wrap,
            gap,
            entries,
            arrangement: Some(arrangement),
            lines: placed.lines,
            float_rows: Vec::new(),
        }
    }

    /// The cell next to the one at `from` in direction `dir`, by where the cells are on screen.
    ///
    /// Cells that line up with `from` across the direction come first, like the next cell in a
    /// row; past those, the nearest one, weighing the distance along the direction far more than
    /// the one across it, so that the next row wins over a cell further away that lines up
    /// better.
    pub fn neighbor(&self, from: usize, dir: GridDirection) -> Option<usize> {
        let (_, current) = self.entries.get(from)?;

        // The extent of a rectangle along the direction, increasing in it, and across it.
        let extents = |r: Rectangle<f64, Logical>| {
            let (left, right) = (r.loc.x, r.loc.x + r.size.w);
            let (top, bottom) = (r.loc.y, r.loc.y + r.size.h);
            match dir {
                GridDirection::Right => ((left, right), (top, bottom)),
                GridDirection::Left => ((-right, -left), (top, bottom)),
                GridDirection::Down => ((top, bottom), (left, right)),
                GridDirection::Up => ((-bottom, -top), (left, right)),
            }
        };
        let ((from_start, from_end), (from_lo, from_hi)) = extents(current.rect());
        let from_middle = (from_lo + from_hi) / 2.;

        self.entries
            .iter()
            .enumerate()
            .filter(|(idx, _)| *idx != from)
            .filter_map(|(idx, (_, info))| {
                let ((start, end), (lo, hi)) = extents(info.rect());
                if start <= from_start + EPSILON || end <= from_end + EPSILON {
                    return None;
                }
                let lines_up = lo < from_hi - EPSILON && from_lo < hi - EPSILON;
                let along = (start - from_end).max(0.);
                let across = ((lo + hi) / 2. - from_middle).abs();
                Some((!lines_up, 13. * along * along + across * across, idx))
            })
            .min_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)))
            .map(|(_, _, idx)| idx)
    }
}

/// The tiled columns among `items`, as ranges of item indices in strip order, a tabbed column with
/// one item per tab, and the indices of the floating windows.
fn columns_and_floating<W: LayoutElement>(
    items: &[(GridItem<W>, GridSource)],
) -> (Vec<Range<usize>>, Vec<usize>) {
    let mut groups: Vec<Range<usize>> = Vec::new();
    let mut floating = Vec::new();
    for (idx, (item, _)) in items.iter().enumerate() {
        let Some(col_idx) = item.col_idx() else {
            floating.push(idx);
            continue;
        };
        match groups.last_mut() {
            Some(group) if group.end == idx && items[group.start].0.col_idx() == Some(col_idx) => {
                group.end += 1;
            }
            _ => groups.push(idx..idx + 1),
        }
    }
    (groups, floating)
}

/// Narrows down, in `steps` halvings, the largest value that `fits` between `yes`, one that does,
/// and `no`, one that doesn't.
fn bisect(mut yes: f64, mut no: f64, steps: u32, fits: impl Fn(f64) -> bool) -> (f64, f64) {
    for _ in 0..steps {
        let mid = (yes + no) / 2.;
        if fits(mid) {
            yes = mid;
        } else {
            no = mid;
        }
    }
    (yes, no)
}

fn rect_center(r: Rectangle<f64, Logical>) -> Point<f64, Logical> {
    r.loc + Point::from((r.size.w / 2., r.size.h / 2.))
}

/// One grid cell, as input to [`GridOverview::update_minimized_highlights`].
pub(super) struct MinimizedHighlightEntry<W: LayoutElement> {
    pub window: W::Id,
    /// On-screen size of the cell, already grown by the configured padding.
    pub size: Size<f64, Logical>,
    pub is_urgent: bool,
    pub is_minimized: bool,
    /// The window's tile as drawn in the grid right now, with its corner radius, for the dim
    /// layer.
    pub tile_rect: Option<(Rectangle<f64, Logical>, CornerRadius)>,
}

/// Where and how to draw the focus ring of a window in the grid, as input to
/// [`GridOverview::update_focus_rings`].
pub(super) struct GridRingTarget<W: LayoutElement> {
    pub window: W::Id,
    /// The window's tile as drawn in the grid right now.
    pub rect: Rectangle<f64, Logical>,
    /// The tile's corner radius, scaled down with it.
    pub radius: CornerRadius,
    /// The window's own focus ring settings.
    pub config: niri_config::FocusRing,
    pub is_urgent: bool,
    /// Opacity of the ring, for windows whose own ring is hidden outside the grid.
    pub alpha: f64,
}

/// The tab indicator next to the cell of a tab, as input to
/// [`GridOverview::update_tab_indicators`].
pub(super) struct GridTabTarget<W: LayoutElement> {
    /// The tab's window.
    pub window: W::Id,
    /// The tab's tile as drawn in the grid right now.
    pub area: Rectangle<f64, Logical>,
    pub config: niri_config::TabIndicator,
    /// Every tab of the column, this one active.
    pub tabs: Vec<TabInfo>,
    /// Color of the slot around the indicator: the background behind the cells.
    pub slot_color: niri_config::Color,
}

/// The tab indicator next to the cell of a tab.
#[derive(Debug)]
struct GridTabIndicator<W: LayoutElement> {
    window: W::Id,
    indicator: TabIndicator,
    slot: FocusRing,
    slot_loc: Option<Point<f64, Logical>>,
    /// The column's tabs as of the last update, for folding the indicator away.
    last_tabs: Vec<TabInfo>,
    /// Whether the window stopped being a tab and the indicator is folding away.
    leaving: bool,
}

/// A focus ring the grid draws around a cell's window.
#[derive(Debug)]
struct GridFocusRing<W: LayoutElement> {
    window: W::Id,
    ring: FocusRing,
    /// How far the ring is grown in: 1 on the focused window, 0 once it lost the focus.
    shown: f64,
    /// The ongoing change of `shown`, with the grow transition.
    fade: Option<Animation>,
    /// Where the ring was last laid out: the tile's rectangle and corner radius.
    rect: Option<(Rectangle<f64, Logical>, CornerRadius)>,
}

impl<W: LayoutElement> GridFocusRing<W> {
    /// How far the ring is grown in. A spring can take it past 1.
    fn shown_value(&self) -> f64 {
        self.fade.as_ref().map_or(self.shown, |fade| fade.value())
    }
}

/// How far the trailing edges of a sliding ring lag behind its leading edges, as a share of the
/// slide. The ring stretches a little on the way, which reads as moving rather than being moved.
const SLIDE_STRETCH: f64 = 0.1;

/// Progress of the leading and of the trailing edges of a sliding ring.
///
/// Both follow the slide's own curve, the leading ones sped up and the trailing ones starting
/// late, so that both arrive when the slide ends.
fn slide_progress(anim: &Animation, now: Duration) -> (f64, f64) {
    let start = anim.start_time();
    let total = anim.duration().as_secs_f64();
    let elapsed = now.saturating_sub(start).as_secs_f64();
    let k = SLIDE_STRETCH;

    let lead = elapsed / (1. - k);
    let trail = (elapsed - k * total).max(0.) / (1. - k);
    (
        anim.value_at(start + Duration::from_secs_f64(lead)),
        anim.value_at(start + Duration::from_secs_f64(trail)),
    )
}

/// Moves each edge of `from` toward `to`, the edges in front in the direction of motion with the
/// `lead` progress and the ones behind with the `trail` progress.
fn stretch_rect(
    from: Rectangle<f64, Logical>,
    to: Rectangle<f64, Logical>,
    lead: f64,
    trail: f64,
) -> Rectangle<f64, Logical> {
    let lerp = |a: f64, b: f64, t: f64| a + (b - a) * t;
    let moves_right = to.loc.x + to.size.w / 2. >= from.loc.x + from.size.w / 2.;
    let moves_down = to.loc.y + to.size.h / 2. >= from.loc.y + from.size.h / 2.;
    let (left_t, right_t) = if moves_right {
        (trail, lead)
    } else {
        (lead, trail)
    };
    let (top_t, bottom_t) = if moves_down {
        (trail, lead)
    } else {
        (lead, trail)
    };

    let left = lerp(from.loc.x, to.loc.x, left_t);
    let right = lerp(from.loc.x + from.size.w, to.loc.x + to.size.w, right_t);
    let top = lerp(from.loc.y, to.loc.y, top_t);
    let bottom = lerp(from.loc.y + from.size.h, to.loc.y + to.size.h, bottom_t);
    Rectangle::new(
        Point::from((left, top)),
        Size::from(((right - left).max(0.), (bottom - top).max(0.))),
    )
}

/// The focus ring sliding to a new cell.
#[derive(Debug)]
struct GridRingSlide {
    from: Rectangle<f64, Logical>,
    from_radius: CornerRadius,
    anim: Animation,
}

fn lerp_radius(from: CornerRadius, to: CornerRadius, t: f32) -> CornerRadius {
    let lerp = |a: f32, b: f32| a + (b - a) * t;
    CornerRadius {
        top_left: lerp(from.top_left, to.top_left),
        top_right: lerp(from.top_right, to.top_right),
        bottom_right: lerp(from.bottom_right, to.bottom_right),
        bottom_left: lerp(from.bottom_left, to.bottom_left),
    }
}

/// A highlight drawn behind the cell of a minimized window, with its own fade.
#[derive(Debug)]
struct MinimizedHighlight<W: LayoutElement> {
    window: W::Id,
    ring: FocusRing,
    /// The dark layer over the thumbnail, a filled rectangle like `ring`.
    dim: FocusRing,
    /// Where `dim` goes, if it shows.
    dim_loc: Option<Point<f64, Logical>>,
    /// 0 while absent, 1 while fully shown; a fade-out is dropped once it reaches 0.
    fade: Animation,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridEntryInfo {
    /// The row of a tiled window's cell in the block of tiled windows; `None` for a floating
    /// window.
    pub row: Option<usize>,
    pub target_pos: Point<f64, Logical>,
    pub target_size: Size<f64, Logical>,
    pub target_scale: f64,
}

impl GridEntryInfo {
    pub fn rect(&self) -> Rectangle<f64, Logical> {
        Rectangle::new(self.target_pos, self.target_size)
    }

    fn same_position_target(&self, other: &Self) -> bool {
        const EPSILON: f64 = 0.000001;

        (self.target_pos.x - other.target_pos.x).abs() <= EPSILON
            && (self.target_pos.y - other.target_pos.y).abs() <= EPSILON
    }

    fn same_scale_target(&self, other: &Self) -> bool {
        const EPSILON: f64 = 0.000001;

        (self.target_scale - other.target_scale).abs() <= EPSILON
    }

    fn same_target(&self, other: &Self) -> bool {
        const EPSILON: f64 = 0.000001;

        self.row == other.row
            && self.same_position_target(other)
            && (self.target_size.w - other.target_size.w).abs() <= EPSILON
            && (self.target_size.h - other.target_size.h).abs() <= EPSILON
            && self.same_scale_target(other)
    }
}
