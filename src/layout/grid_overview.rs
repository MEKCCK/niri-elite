use std::rc::Rc;

use smithay::utils::{Logical, Point, Rectangle, Size};

use super::{Animation, Clock, LayoutElement, Options, OverviewProgress};

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
    added_window_ids: Vec<W::Id>,
    /// col_idx → tile_idx for Column items that have multiple tiles.
    pub column_tile_focus: Vec<(usize, usize)>,
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
            added_window_ids: Vec::new(),
            column_tile_focus: Vec::new(),
            clock,
            options,
        }
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

    pub fn compute_layout(
        &mut self,
        items: &[(GridItem<W>, Size<f64, Logical>)],
        area: Rectangle<f64, Logical>,
        restart_rearrange: bool,
    ) {
        let old_layout = self.layout.clone();
        let new_layout = GridLayout::compute(items, area, &self.options.grid_overview);
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
        let should_rearrange = self.open && !old_layout.entries.is_empty();
        let targets_changed = old_layout.entries.len() != new_layout.entries.len()
            || new_layout.entries.iter().any(|(item, info)| {
                Self::matching_value(&old_layout.entries, item)
                    .is_none_or(|old_info| !old_info.same_target(info))
            });
        let restart_rearrange = should_rearrange && restart_rearrange && targets_changed;
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
            let old_info = Self::matching_value(&old_layout.entries, item);
            let entry_pos = Self::matching_value(&self.entry_positions, item).copied();
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
            let old_info = Self::matching_value(&old_layout.entries, item);
            let entry_scale = Self::matching_value(&self.entry_scales, item).copied();
            let scale_target_unchanged =
                old_info.is_some_and(|old_info| old_info.same_scale_target(info));
            let current_scale = match (rearrange_value, entry_scale, old_info) {
                (Some(value), Some(entry), Some(old_info)) => {
                    entry + (old_info.target_scale - entry) * value
                }
                (_, entry, Some(old_info)) if self.open => {
                    let entry = entry.unwrap_or(1.);
                    entry + (old_info.target_scale - entry) * progress_value
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
            let boost = Self::matching_value(&self.focus_boosts, item)
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
            from + (target_focus_boost - from) * anim.value().clamp(0., 1.)
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

    pub fn set_focus(&mut self, focus: (usize, usize)) {
        if self.focus == focus {
            return;
        }

        let old_focus = self.focus;
        if self.open {
            self.snapshot_focus_boosts();
            self.previous_focus = Some(old_focus);
            self.focus_boost_anim = Some(Animation::new(
                self.clock.clone(),
                0.,
                1.,
                0.,
                self.options.animations.window_resize.anim,
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
                self.options.animations.window_resize.anim,
            ));
        }

        self.grabbed_window = id;
    }

    pub fn set_focus_without_animation(&mut self, focus: (usize, usize)) {
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
            .find(|(c, _)| *c == col_idx)
            .map(|(_, t)| *t)
            .unwrap_or(0)
    }

    fn set_column_tile_focus_raw(&mut self, col_idx: usize, tile_idx: usize) {
        if let Some(entry) = self
            .column_tile_focus
            .iter_mut()
            .find(|(c, _)| *c == col_idx)
        {
            entry.1 = tile_idx;
        } else {
            self.column_tile_focus.push((col_idx, tile_idx));
        }
    }

    pub fn set_column_tile_focus(&mut self, col_idx: usize, tile_idx: usize, window_id: W::Id) {
        self.set_column_tile_focus_raw(col_idx, tile_idx);
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
                            self.options.animations.window_resize.anim,
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
                            self.options.animations.window_resize.anim,
                        ));
                        return Some((col_idx, new_tile_idx));
                    }
                }
                _ => {}
            }
        }

        // Cross-cell movement.
        let mut new_focus = match dir {
            GridDirection::Up => {
                if row > 0 {
                    (row - 1, col)
                } else {
                    (row, col)
                }
            }
            GridDirection::Down => {
                if row + 1 < self.layout.rows {
                    (row + 1, col)
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

    pub(super) fn record_added_window(&mut self, id: W::Id) {
        if !self.added_window_ids.contains(&id) {
            self.added_window_ids.push(id);
        }
    }

    pub(super) fn window_was_added_while_open(&self, id: &W::Id) -> bool {
        self.added_window_ids.contains(id)
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
    }

    pub fn are_animations_ongoing(&self) -> bool {
        self.progress.as_ref().map_or(false, |p| p.is_animation())
            || self.rearrange_anim.is_some()
            || self.focus_boost_anim.is_some()
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
