use std::rc::Rc;
use std::time::Duration;

use niri_config::animations::Kind;
use niri_config::{CornerRadius, GridFocusRingTransition};
use smithay::utils::{Logical, Point, Rectangle, Size};

use super::focus_ring::{FocusRing, FocusRingRenderElement};
use super::tab_indicator::{TabIndicator, TabIndicatorRenderElement, TabInfo};
use super::{Animation, Clock, LayoutElement, Options, OverviewProgress};
use crate::render_helpers::renderer::NiriRenderer;

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
    pub focus: (usize, usize),
    /// Whether `focus` is only the first cell because nothing else was there to focus.
    ///
    /// This is the case when the grid opens while the active window has no cell (it is an
    /// `ignore-grid-overview` floating window, or every window is minimized) and the focus hasn't
    /// moved since. Closing the grid then must not activate that arbitrary first cell.
    pub(super) focus_is_fallback: bool,
    /// The window that `focus` is on.
    ///
    /// The scrolling layout changes underneath the grid (a column to the left closes, the grid
    /// reflows to another column count), so `focus` alone, a cell position, would end up on a
    /// different window. After every layout recompute, `focus` is moved back to this window's
    /// cell.
    pub(super) focused_window: Option<W::Id>,
    pub saved_active_window_id: Option<W::Id>,
    pub saved_view_offset: f64,
    pub entry_positions: Vec<(GridItem<W>, Point<f64, Logical>)>,
    pub entry_scales: Vec<(GridItem<W>, f64)>,
    pub focus_boosts: Vec<(GridItem<W>, f64)>,
    pub window_transition_starts: Vec<(W::Id, Point<f64, Logical>, f64)>,
    pub close_start_progress: f64,
    pub rearrange_anim: Option<Animation>,
    pub previous_focus: Option<(usize, usize)>,
    pub focus_boost_anim: Option<Animation>,
    /// While a window is interactively grabbed, the focus belongs to the grabbed window
    /// itself, so no grid cell shows the focus boost.
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
            focus: (0, 0),
            focus_is_fallback: false,
            focused_window: None,
            saved_active_window_id: None,
            saved_view_offset: 0.,
            entry_positions: Vec::new(),
            entry_scales: Vec::new(),
            focus_boosts: Vec::new(),
            window_transition_starts: Vec::new(),
            close_start_progress: 1.,
            rearrange_anim: None,
            previous_focus: None,
            focus_boost_anim: None,
            grabbed_window: None,
            flying_in_windows: Vec::new(),
            added_window_ids: Vec::new(),
            restored_window_ids: Vec::new(),
            minimized_highlights: Vec::new(),
            focus_rings: Vec::new(),
            ring_focus: None,
            ring_slide: None,
            tab_indicators: Vec::new(),
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
        self.progress.as_ref().map_or(false, |p| p.is_animation()) || self.rearrange_anim.is_some()
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
    /// visuals. Starting from zero keeps the focus boost and scale math from jumping.
    pub(super) fn reopen(&mut self) {
        self.open = true;
        self.rearrange_anim = None;
        self.window_transition_starts.clear();
        self.flying_in_windows.clear();
        self.previous_focus = None;
        self.focus_boost_anim = None;
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
        items: &[(GridItem<W>, Size<f64, Logical>)],
        area: Rectangle<f64, Logical>,
        restart_rearrange: bool,
    ) -> bool {
        let old_layout = self.layout.clone();
        let new_layout = GridLayout::compute(items, area, &self.options.grid_overview);
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
        let mut new_focus_boosts = Vec::new();
        for (item, _) in &self.layout.entries {
            let boost = Self::matching_old_value(&self.focus_boosts, item, &new_ids)
                .copied()
                .unwrap_or(1.);
            new_focus_boosts.push((item.clone(), boost));
        }
        self.focus_boosts = new_focus_boosts;
        if self.layout.entries.is_empty() {
            self.focus = (0, 0);
        } else {
            self.focus.0 = self.focus.0.min(self.layout.rows.saturating_sub(1));
            self.focus.1 = self.focus.1.min(self.layout.cols.saturating_sub(1));
        }

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

        let anim_p = self.rearrange_anim.as_ref().map_or(p, |anim| anim.value());
        let is_rearranging = self.rearrange_anim.is_some();
        let is_opening = self.open
            && self
                .progress
                .as_ref()
                .is_some_and(|progress| progress.is_animation());

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

        let item_focus_boost = self.entry_focus_boost(item, info);
        let base_size = item_source_size.upscale(final_scale);
        let visual_size = base_size.upscale(item_focus_boost);
        let grow = Point::from((
            (visual_size.w - base_size.w) / 2.,
            (visual_size.h - base_size.h) / 2.,
        ));
        let visual_pos = lerp_pos - grow;

        (visual_pos, final_scale * item_focus_boost)
    }

    pub(super) fn entry_focus_boost(&self, item: &GridItem<W>, info: &GridEntryInfo) -> f64 {
        let configured_focus_boost = self
            .options
            .grid_overview
            .focused_column_scale
            .clamp(1., 2.);
        let base_boost = 1. + (configured_focus_boost - 1.) * self.progress_value().clamp(0., 1.);
        let target_focus_boost =
            if self.grabbed_window.is_none() && self.focus == (info.row, info.col) {
                base_boost
            } else {
                1.
            };

        let boost = if let Some(anim) = &self.focus_boost_anim {
            let from = Self::matching_value(&self.focus_boosts, item)
                .copied()
                .unwrap_or(1.);
            from + (target_focus_boost - from) * anim.clamped_value()
        } else {
            target_focus_boost
        };

        // Prevent spring overshoot from shrinking the item below its normal size.
        boost.max(1.)
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

    /// Animation used for grid focus boost.
    ///
    /// Focus boost is an internal scale animation; it must be monotonic. User window-resize
    /// springs can be underdamped, which makes `clamped_value()` freeze at the target with
    /// non-zero velocity and causes a visible hitch when a window shrinks back.
    fn focus_boost_animation(&self) -> niri_config::Animation {
        let mut anim = self.options.animations.window_resize.anim;
        if let Kind::Spring(params) = &mut anim.kind {
            params.damping_ratio = params.damping_ratio.max(1.0);
        }
        anim
    }

    pub fn set_focus(&mut self, focus: (usize, usize)) {
        if self.focus == focus {
            return;
        }
        self.focus_is_fallback = false;

        let old_focus = self.focus;
        if self.open {
            // Preserve the current focus-boost velocity when retargeting. Restarting from zero
            // every time makes rapidly-switched windows visibly hitch.
            let clock_rate = self.clock.rate();
            let velocity = self.focus_boost_anim.as_ref().map_or(0., |anim| {
                let now = self.clock.now();
                let dt = Duration::from_millis(1);
                let prev = anim.value_at(now.saturating_sub(dt));
                let raw = (anim.value_at(now) - prev) / dt.as_secs_f64();
                // Animation::new() divides the initial velocity by the clock rate, so convert
                // the derivative back to unadjusted time.
                raw * clock_rate
            });

            self.snapshot_focus_boosts();
            self.previous_focus = Some(old_focus);
            self.focus_boost_anim = Some(Animation::new(
                self.clock.clone(),
                0.,
                1.,
                velocity,
                self.focus_boost_animation(),
            ));
        }

        self.focus = focus;
    }

    pub fn set_grabbed_window(&mut self, id: Option<W::Id>) {
        if self.grabbed_window == id {
            return;
        }

        if self.open {
            // Animate the focus boost changes caused by the grab state change.
            self.snapshot_focus_boosts();
            self.focus_boost_anim = Some(Animation::new(
                self.clock.clone(),
                0.,
                1.,
                0.,
                self.focus_boost_animation(),
            ));
        }

        self.grabbed_window = id;
    }

    pub fn set_focus_without_animation(&mut self, focus: (usize, usize)) {
        if self.focus == focus {
            return;
        }
        self.focus_is_fallback = false;

        self.focus = focus;
        self.previous_focus = None;
        self.focus_boost_anim = None;
        self.focus_boosts.clear();
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
        self.focus_boosts.clear();
        self.close_start_progress = self.progress_value().max(0.0001);
    }

    fn snapshot_focus_boosts(&mut self) {
        // Capture the current animated boost values, so that an in-progress transition
        // continues smoothly from where it visually is rather than from its steady state.
        self.focus_boosts = self
            .layout
            .entries
            .iter()
            .map(|(item, info)| (item.clone(), self.entry_focus_boost(item, info)))
            .collect();
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

        let (row, col) = self.focus;
        let idx = row * self.layout.cols + col;

        // Try switching tile within a Column.
        let current_col_idx = self.layout.entries.get(idx).and_then(|(item, _)| {
            if let GridItem::Column { col_idx, .. } = item {
                Some(*col_idx)
            } else {
                None
            }
        });

        if let Some(col_idx) = current_col_idx {
            match dir {
                GridDirection::Up => {
                    let current = self.get_column_tile_focus(col_idx);
                    if current > 0 {
                        let new_tile_idx = current - 1;
                        self.set_column_tile_focus_raw(col_idx, new_tile_idx);
                        self.snapshot_focus_boosts();
                        self.focus_boost_anim = Some(Animation::new(
                            self.clock.clone(),
                            0.,
                            1.,
                            0.,
                            self.focus_boost_animation(),
                        ));
                        return Some((col_idx, new_tile_idx));
                    }
                }
                GridDirection::Down => {
                    let current = self.get_column_tile_focus(col_idx);
                    let count = get_tile_count(col_idx);
                    if count > 1 && current + 1 < count {
                        let new_tile_idx = current + 1;
                        self.set_column_tile_focus_raw(col_idx, new_tile_idx);
                        self.snapshot_focus_boosts();
                        self.focus_boost_anim = Some(Animation::new(
                            self.clock.clone(),
                            0.,
                            1.,
                            0.,
                            self.focus_boost_animation(),
                        ));
                        return Some((col_idx, new_tile_idx));
                    }
                }
                _ => {}
            }
        }

        // Cross-cell movement. Up and down go to the cell in that row whose center is closest
        // horizontally: a shorter last row is centered, so its cells don't sit under the same
        // column indices of the rows above.
        let closest_in_row = |target_row: usize| {
            let center_x = |info: &GridEntryInfo| info.target_pos.x + info.target_size.w / 2.;
            let current = self.layout.entries.get(row * self.layout.cols + col)?;
            let current_x = center_x(&current.1);
            self.layout
                .entries
                .iter()
                .filter(|(_, info)| info.row == target_row)
                .min_by(|(_, a), (_, b)| {
                    let da = (center_x(a) - current_x).abs();
                    let db = (center_x(b) - current_x).abs();
                    da.total_cmp(&db)
                })
                .map(|(_, info)| (info.row, info.col))
        };
        let mut new_focus = match dir {
            GridDirection::Up => {
                if row > 0 {
                    closest_in_row(row - 1).unwrap_or((row - 1, col))
                } else {
                    (row, col)
                }
            }
            GridDirection::Down => {
                if row + 1 < self.layout.rows {
                    closest_in_row(row + 1).unwrap_or((row + 1, col))
                } else {
                    (row, col)
                }
            }
            GridDirection::Left => {
                if col > 0 {
                    (row, col - 1)
                } else {
                    (row, col)
                }
            }
            GridDirection::Right => {
                if col + 1 < self.layout.cols {
                    (row, col + 1)
                } else {
                    (row, col)
                }
            }
        };
        let idx = new_focus.0 * self.layout.cols + new_focus.1;
        if idx >= self.layout.entries.len() {
            let last_idx = self.layout.entries.len().saturating_sub(1);
            new_focus = (last_idx / self.layout.cols, last_idx % self.layout.cols);
        }

        self.set_focus(new_focus);
        None
    }

    pub fn focused_id(&self) -> Option<W::Id> {
        let idx = self.focus.0 * self.layout.cols + self.focus.1;
        let (item, _) = self.layout.entries.get(idx)?;
        match item {
            GridItem::Column { .. } => {
                // For Column items, the window_id is kept in sync by
                // set_column_tile_focus to match the currently focused tile.
                Some(item.window_id().clone())
            }
            _ => Some(item.window_id().clone()),
        }
    }

    pub fn focused_item(&self) -> Option<&GridItem<W>> {
        let idx = self.focus.0 * self.layout.cols + self.focus.1;
        self.layout.entries.get(idx).map(|(item, _)| item)
    }

    pub fn set_focused_window_id(&mut self, id: W::Id) {
        let idx = self.focus.0 * self.layout.cols + self.focus.1;
        if let Some((item, _)) = self.layout.entries.get_mut(idx) {
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

    fn advance_focus_rings(&mut self) {
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
    pub(super) fn update_tab_indicators(
        &mut self,
        targets: Vec<GridTabTarget<W>>,
        is_active: bool,
        view_size: Size<f64, Logical>,
        scale: f64,
    ) {
        // They fade in as the grid opens.
        let alpha = self.progress_value().clamp(0., 1.) as f32;

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
                },
            };
            let indicator = &mut tab.indicator;
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
    }

    /// Windows whose tab cell has a tab indicator.
    #[cfg(test)]
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
        let idx = self.focus.0 * self.layout.cols + self.focus.1;
        self.layout.entries.get(idx).map(|(_, info)| info)
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

    pub fn find_grid_index(&self, id: &W::Id) -> Option<(usize, usize)> {
        self.layout
            .entries
            .iter()
            .enumerate()
            .find_map(|(idx, (item, _))| {
                if item.window_id() == id {
                    Some((idx / self.layout.cols, idx % self.layout.cols))
                } else {
                    None
                }
            })
    }

    pub fn find_grid_index_for_item(&self, item: &GridItem<W>) -> Option<(usize, usize)> {
        self.layout
            .entries
            .iter()
            .enumerate()
            .find_map(|(idx, (entry, _))| {
                if entry == item {
                    Some((idx / self.layout.cols, idx % self.layout.cols))
                } else {
                    None
                }
            })
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
                if self.open && self.rearrange_anim.is_none() && self.focus_boost_anim.is_none() {
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
        if let Some(anim) = &mut self.focus_boost_anim {
            if anim.is_done() {
                self.previous_focus = None;
                self.focus_boost_anim = None;
                self.focus_boosts.clear();
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
        self.focus_boosts = self
            .layout
            .entries
            .iter()
            .map(|(item, _)| (item.clone(), 1.))
            .collect();
        self.window_transition_starts.clear();
        self.flying_in_windows.clear();
    }

    pub fn are_animations_ongoing(&self) -> bool {
        self.progress.as_ref().map_or(false, |p| p.is_animation())
            || self.rearrange_anim.is_some()
            || self.focus_boost_anim.is_some()
            || self.minimized_highlights_animating()
            || self.focus_rings_animating()
    }
}

#[derive(Debug)]
pub struct GridLayout<W: LayoutElement> {
    pub cols: usize,
    pub rows: usize,
    pub gap: f64,
    pub entries: Vec<(GridItem<W>, GridEntryInfo)>,
}

const TILING_GRID_SCALE_WEIGHT: f64 = 0.5;
const MIXED_FLOATING_GRID_SCALE_WEIGHT: f64 = 0.5;

impl<W: LayoutElement> Clone for GridLayout<W> {
    fn clone(&self) -> Self {
        Self {
            cols: self.cols,
            rows: self.rows,
            gap: self.gap,
            entries: self.entries.clone(),
        }
    }
}

impl<W: LayoutElement> GridLayout<W> {
    pub fn empty() -> Self {
        Self {
            cols: 0,
            rows: 0,
            gap: 0.,
            entries: Vec::new(),
        }
    }

    pub fn compute(
        entries: &[(GridItem<W>, Size<f64, Logical>)],
        area: Rectangle<f64, Logical>,
        config: &niri_config::GridOverview,
    ) -> Self {
        let n = entries.len();
        if n == 0 {
            return Self::empty();
        }

        let gap = config.gap;
        let padding = config.padding;

        let content_w = (area.size.w - padding.left - padding.right).max(1.);
        let content_h = (area.size.h - padding.top - padding.bottom).max(1.);
        let aspect = content_w / content_h;

        let mut cols = ((n as f64 * aspect).sqrt()).ceil().max(1.) as usize;
        let mut rows = n.div_ceil(cols);
        if (cols - 1) * rows >= n && cols > 1 {
            cols -= 1;
            rows = n.div_ceil(cols);
        }

        // A gap too large for the grid would leave the cells with negative sizes, and the windows
        // with negative scales. Shrink it so every cell keeps at least a pixel.
        let max_gap = |content: f64, count: usize| {
            if count > 1 {
                (content - count as f64) / (count - 1) as f64
            } else {
                f64::INFINITY
            }
        };
        let gap = gap
            .min(max_gap(content_w, cols))
            .min(max_gap(content_h, rows))
            .max(0.);

        let cell_w = (content_w - gap * (cols as f64 - 1.)) / cols as f64;
        let cell_h = (content_h - gap * (rows as f64 - 1.)) / rows as f64;
        let cell_ar = cell_w / cell_h;

        let tiling_scale = entries
            .iter()
            .filter(|(item, _)| !matches!(item, GridItem::Floating { .. }))
            .map(|(_, in_size)| Self::entry_fit_scale(*in_size, cell_w, cell_h, cell_ar, config))
            .reduce(f64::min);

        let mut entry_sizes = Vec::with_capacity(n);
        let mut row_content_widths = vec![0.0_f64; rows];
        let mut row_heights = vec![0.0_f64; rows];
        let mut row_counts = vec![0usize; rows];

        for (idx, (item, in_size)) in entries.iter().enumerate() {
            let row = idx / cols;
            let col = idx % cols;

            let is_floating = matches!(item, GridItem::Floating { .. });
            let target_scale = if is_floating {
                if let Some(target_scale) = tiling_scale {
                    let independent_scale =
                        Self::entry_fit_scale(*in_size, cell_w, cell_h, cell_ar, config);
                    Self::blend_scale(
                        target_scale,
                        independent_scale,
                        MIXED_FLOATING_GRID_SCALE_WEIGHT,
                    )
                } else {
                    Self::entry_fit_scale(*in_size, cell_w, cell_h, cell_ar, config)
                }
            } else {
                let independent_scale =
                    Self::entry_fit_scale(*in_size, cell_w, cell_h, cell_ar, config);
                let uniform_scale = tiling_scale.unwrap_or(independent_scale);
                Self::blend_scale(uniform_scale, independent_scale, TILING_GRID_SCALE_WEIGHT)
            };
            let scaled_w = in_size.w * target_scale;
            let scaled_h = in_size.h * target_scale;

            let target_size = Size::from((scaled_w, scaled_h));

            row_content_widths[row] += target_size.w;
            row_heights[row] = row_heights[row].max(target_size.h);
            row_counts[row] += 1;

            entry_sizes.push((row, col, target_size, target_scale));
        }

        let fill_scale_for_width = row_content_widths
            .iter()
            .zip(&row_counts)
            .filter(|(_, count)| **count > 0)
            .map(|(content_width, count)| {
                let gap_width = gap * count.saturating_sub(1) as f64;
                ((content_w - gap_width).max(1.) / content_width.max(1.)).max(0.)
            })
            .reduce(f64::min)
            .unwrap_or(1.);
        let row_gap_height = gap * rows.saturating_sub(1) as f64;
        let fill_scale_for_height = ((content_h - row_gap_height).max(1.)
            / row_heights.iter().sum::<f64>().max(1.))
        .max(0.);
        let fill_scale = fill_scale_for_width.min(fill_scale_for_height);

        if fill_scale.is_finite() && fill_scale != 1. {
            for (_, _, target_size, target_scale) in &mut entry_sizes {
                *target_size = target_size.upscale(fill_scale);
                *target_scale *= fill_scale;
            }
        }

        row_content_widths.fill(0.);
        row_heights.fill(0.);
        for ((item, in_size), (row, _, target_size, target_scale)) in
            entries.iter().zip(&mut entry_sizes)
        {
            if matches!(item, GridItem::Floating { .. }) && *target_scale > 1. {
                *target_scale = 1.;
                *target_size = *in_size;
            }

            row_content_widths[*row] += target_size.w;
            row_heights[*row] = row_heights[*row].max(target_size.h);
        }

        let packed_h = row_heights.iter().sum::<f64>() + gap * (rows - 1) as f64;
        let start_y = area.loc.y + padding.top + (content_h - packed_h) / 2.;
        let mut row_offsets = Vec::with_capacity(rows);
        let mut next_y = 0.;
        for height in &row_heights {
            row_offsets.push(next_y);
            next_y += height + gap;
        }

        let mut row_next_x = vec![0.; rows];
        let mut out_entries = Vec::with_capacity(n);

        for ((item, _), (row, col, target_size, target_scale)) in entries.iter().zip(entry_sizes) {
            let row_width =
                row_content_widths[row] + gap * row_counts[row].saturating_sub(1) as f64;
            let row_start_x = area.loc.x + padding.left + (content_w - row_width) / 2.;
            let target_pos = Point::from((
                row_start_x + row_next_x[row],
                start_y + row_offsets[row] + (row_heights[row] - target_size.h) / 2.,
            ));
            row_next_x[row] += target_size.w + gap;

            out_entries.push((
                item.clone(),
                GridEntryInfo {
                    row,
                    col,
                    target_pos,
                    target_size,
                    target_scale,
                },
            ));
        }

        Self {
            cols,
            rows,
            gap,
            entries: out_entries,
        }
    }

    fn entry_fit_size(
        in_size: Size<f64, Logical>,
        cell_w: f64,
        cell_h: f64,
        cell_ar: f64,
        config: &niri_config::GridOverview,
    ) -> (f64, f64, f64) {
        let tile_ar = in_size.w / in_size.h.max(1.);

        let (scaled_w, scaled_h) = if tile_ar > cell_ar {
            let sw = cell_w;
            let sh = sw / tile_ar;
            (sw, sh.max(cell_h * config.min_scale))
        } else {
            let sh = cell_h;
            let sw = sh * tile_ar;
            (sw.max(cell_w * config.min_scale), sh)
        };

        let target_scale = scaled_w / in_size.w.max(1.);
        (scaled_w, scaled_h, target_scale)
    }

    fn entry_fit_scale(
        in_size: Size<f64, Logical>,
        cell_w: f64,
        cell_h: f64,
        cell_ar: f64,
        config: &niri_config::GridOverview,
    ) -> f64 {
        Self::entry_fit_size(in_size, cell_w, cell_h, cell_ar, config).2
    }

    fn blend_scale(uniform_scale: f64, independent_scale: f64, independent_weight: f64) -> f64 {
        uniform_scale * (independent_scale / uniform_scale).powf(independent_weight)
    }
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
    pub row: usize,
    pub col: usize,
    pub target_pos: Point<f64, Logical>,
    pub target_size: Size<f64, Logical>,
    pub target_scale: f64,
}

impl GridEntryInfo {
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
            && self.col == other.col
            && self.same_position_target(other)
            && (self.target_size.w - other.target_size.w).abs() <= EPSILON
            && (self.target_size.h - other.target_size.h).abs() <= EPSILON
            && self.same_scale_target(other)
    }
}
