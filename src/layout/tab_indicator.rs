use std::iter::zip;
use std::mem;

use niri_config::{CornerRadius, Gradient, GradientRelativeTo, TabIndicatorPosition};
use smithay::utils::{Logical, Point, Rectangle, Size};

use super::tile::Tile;
use super::LayoutElement;
use crate::animation::{Animation, Clock};
use crate::niri_render_elements;
use crate::render_helpers::border::BorderRenderElement;
use crate::render_helpers::renderer::NiriRenderer;
use crate::utils::{
    floor_logical_in_physical_max1, round_logical_in_physical, round_logical_in_physical_max1,
};

#[derive(Debug)]
pub struct TabIndicator {
    shader_locs: Vec<Point<f64, Logical>>,
    shaders: Vec<BorderRenderElement>,
    open_anim: Option<Animation>,
    config: niri_config::TabIndicator,
    /// Opacity of the whole indicator.
    alpha: f32,
    /// The rectangle the tabs cover, as of the last update, in the coordinates of the area.
    bounds: Option<Rectangle<f64, Logical>>,
    /// Where the tabs were drawn on the last update, relative to the area.
    shown_rects: Vec<Rectangle<f64, Logical>>,
    /// The tabs moving to their new places after the tab count changed, from where they were.
    count_anim: Option<(Vec<Rectangle<f64, Logical>>, Animation)>,
    /// Clock and timing for `count_anim`, if the tab count changes animate.
    count_anim_config: Option<(Clock, niri_config::Animation)>,
}

#[derive(Debug)]
pub struct TabInfo {
    /// Gradient for the tab indicator.
    pub gradient: Gradient,
    /// Tab geometry in the same coordinate system as the area.
    pub geometry: Rectangle<f64, Logical>,
}

niri_render_elements! {
    TabIndicatorRenderElement => {
        Gradient = BorderRenderElement,
    }
}

impl TabIndicator {
    pub fn new(config: niri_config::TabIndicator) -> Self {
        Self {
            shader_locs: Vec::new(),
            shaders: Vec::new(),
            open_anim: None,
            config,
            alpha: 1.,
            bounds: None,
            shown_rects: Vec::new(),
            count_anim: None,
            count_anim_config: None,
        }
    }

    /// Animates the tabs to their new places when the tab count changes, with this timing.
    pub fn set_count_animation(&mut self, clock: Clock, config: niri_config::Animation) {
        self.count_anim_config = Some((clock, config));
    }

    /// The rectangle the tabs cover, as of the last update.
    pub fn bounds(&self) -> Option<Rectangle<f64, Logical>> {
        self.bounds
    }

    pub fn update_config(&mut self, config: niri_config::TabIndicator) {
        self.config = config;
    }

    /// Sets the opacity of the whole indicator, applied on the next update.
    pub fn set_alpha(&mut self, alpha: f32) {
        self.alpha = alpha;
    }

    pub fn update_shaders(&mut self) {
        for elem in &mut self.shaders {
            elem.damage_all();
        }
    }

    pub fn advance_animations(&mut self) {
        if let Some(anim) = &mut self.open_anim {
            if anim.is_done() {
                self.open_anim = None;
            }
        }
        if self
            .count_anim
            .as_ref()
            .is_some_and(|(_, anim)| anim.is_done())
        {
            self.count_anim = None;
        }
    }

    pub fn are_animations_ongoing(&self) -> bool {
        self.open_anim.is_some() || self.count_anim.is_some()
    }

    pub fn start_open_animation(&mut self, clock: Clock, config: niri_config::Animation) {
        self.open_anim = Some(Animation::new(clock, 0., 1., 0., config));
    }

    fn tab_rects(
        &self,
        area: Rectangle<f64, Logical>,
        count: usize,
        scale: f64,
    ) -> impl Iterator<Item = Rectangle<f64, Logical>> {
        let round = |logical: f64| round_logical_in_physical(scale, logical);
        let round_max1 = |logical: f64| round_logical_in_physical_max1(scale, logical);

        let progress = self.open_anim.as_ref().map_or(1., |a| a.value().max(0.));

        let width = round_max1(self.config.width);
        let gap = self.config.gap;
        let gap = round_max1(gap.abs()).copysign(gap);
        let gaps_between = round_max1(self.config.gaps_between_tabs);

        let position = self.config.position;
        let side = match position {
            TabIndicatorPosition::Left | TabIndicatorPosition::Right => area.size.h,
            TabIndicatorPosition::Top | TabIndicatorPosition::Bottom => area.size.w,
        };
        let total_prop = self.config.length.total_proportion.unwrap_or(0.5);
        let min_length = round(side * total_prop.clamp(0., 2.));

        // Compute px_per_tab before applying the animation to gaps_between in order to avoid it
        // growing and shrinking over the duration of the animation.
        let pixel = 1. / scale;
        let shortest_length = count as f64 * (pixel + gaps_between) - gaps_between;
        let length = f64::max(min_length, shortest_length);
        let px_per_tab = (length + gaps_between) / count as f64 - gaps_between;

        let px_per_tab = px_per_tab * progress;
        let gaps_between = round(self.config.gaps_between_tabs * progress);

        let length = count as f64 * (px_per_tab + gaps_between) - gaps_between;
        let px_per_tab = floor_logical_in_physical_max1(scale, px_per_tab);
        let floored_length = count as f64 * (px_per_tab + gaps_between) - gaps_between;
        let mut ones_left = ((length - floored_length) / pixel).round() as usize;

        let mut shader_loc = Point::from((-gap - width, round((side - length) / 2.)));
        match position {
            TabIndicatorPosition::Left => (),
            TabIndicatorPosition::Right => shader_loc.x = area.size.w + gap,
            TabIndicatorPosition::Top => mem::swap(&mut shader_loc.x, &mut shader_loc.y),
            TabIndicatorPosition::Bottom => {
                shader_loc.x = shader_loc.y;
                shader_loc.y = area.size.h + gap;
            }
        }
        shader_loc += area.loc;

        (0..count).map(move |_| {
            let mut px_per_tab = px_per_tab;
            if ones_left > 0 {
                ones_left -= 1;
                px_per_tab += pixel;
            }

            let loc = shader_loc;

            match position {
                TabIndicatorPosition::Left | TabIndicatorPosition::Right => {
                    shader_loc.y += px_per_tab + gaps_between
                }
                TabIndicatorPosition::Top | TabIndicatorPosition::Bottom => {
                    shader_loc.x += px_per_tab + gaps_between
                }
            }

            let size = match position {
                TabIndicatorPosition::Left | TabIndicatorPosition::Right => {
                    Size::from((width, px_per_tab))
                }
                TabIndicatorPosition::Top | TabIndicatorPosition::Bottom => {
                    Size::from((px_per_tab, width))
                }
            };

            Rectangle::new(loc, size)
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update_render_elements(
        &mut self,
        enabled: bool,
        // Geometry of the tabs area.
        area: Rectangle<f64, Logical>,
        // View rect relative to the tabs area.
        area_view_rect: Rectangle<f64, Logical>,
        // Tab count, should match the tabs iterator length.
        tab_count: usize,
        tabs: impl Iterator<Item = TabInfo>,
        is_active: bool,
        scale: f64,
    ) {
        self.bounds = None;
        if !enabled || self.config.off {
            self.shader_locs.clear();
            self.shaders.clear();
            self.shown_rects.clear();
            self.count_anim = None;
            return;
        }

        let count = tab_count;
        if self.config.hide_when_single_tab && count == 1 {
            self.shader_locs.clear();
            self.shaders.clear();
            self.shown_rects.clear();
            self.count_anim = None;
            return;
        }

        self.shaders.resize_with(count, Default::default);
        self.shader_locs.resize_with(count, Default::default);

        let position = self.config.position;
        let radius = self.config.corner_radius as f32;
        let shared_rounded_corners = self.config.gaps_between_tabs == 0.;
        let mut tabs_left = tab_count;

        let rects = self.animated_tab_rects(area, count, scale);
        self.bounds = rects.iter().copied().reduce(|a, b| a.merge(b));
        for ((shader, loc), (tab, rect)) in zip(
            zip(&mut self.shaders, &mut self.shader_locs),
            zip(tabs, rects),
        ) {
            *loc = rect.loc;

            let mut gradient_area = match tab.gradient.relative_to {
                GradientRelativeTo::Window => tab.geometry,
                GradientRelativeTo::WorkspaceView => area_view_rect,
            };
            gradient_area.loc -= *loc;

            let mut color_from = tab.gradient.from;
            let mut color_to = tab.gradient.to;
            if !is_active {
                color_from *= 0.5;
                color_to *= 0.5;
            }

            let radius = if shared_rounded_corners && tab_count > 1 {
                if tabs_left == tab_count {
                    // First tab.
                    match position {
                        TabIndicatorPosition::Left | TabIndicatorPosition::Right => CornerRadius {
                            top_left: radius,
                            top_right: radius,
                            bottom_right: 0.,
                            bottom_left: 0.,
                        },
                        TabIndicatorPosition::Top | TabIndicatorPosition::Bottom => CornerRadius {
                            top_left: radius,
                            top_right: 0.,
                            bottom_right: 0.,
                            bottom_left: radius,
                        },
                    }
                } else if tabs_left == 1 {
                    // Last tab.
                    match position {
                        TabIndicatorPosition::Left | TabIndicatorPosition::Right => CornerRadius {
                            top_left: 0.,
                            top_right: 0.,
                            bottom_right: radius,
                            bottom_left: radius,
                        },
                        TabIndicatorPosition::Top | TabIndicatorPosition::Bottom => CornerRadius {
                            top_left: 0.,
                            top_right: radius,
                            bottom_right: radius,
                            bottom_left: 0.,
                        },
                    }
                } else {
                    // Tab in the middle.
                    CornerRadius::default()
                }
            } else {
                // Separate tabs, or the only tab.
                CornerRadius::from(radius)
            };
            let radius = radius.fit_to(rect.size.w as f32, rect.size.h as f32);
            tabs_left -= 1;

            shader.update(
                rect.size,
                gradient_area,
                tab.gradient.in_,
                color_from,
                color_to,
                ((tab.gradient.angle as f32) - 90.).to_radians(),
                Rectangle::from_size(rect.size),
                0.,
                radius,
                scale as f32,
                self.alpha,
            );
        }
    }

    /// The tab rectangles, moving from where they were when the tab count changed.
    ///
    /// The indicator only knows the count, not which tab came or went: a new tab grows in at the
    /// end, and when one goes, the rest stretch to fill its place.
    fn animated_tab_rects(
        &mut self,
        area: Rectangle<f64, Logical>,
        count: usize,
        scale: f64,
    ) -> Vec<Rectangle<f64, Logical>> {
        let target: Vec<_> = self
            .tab_rects(area, count, scale)
            .map(|rect| Rectangle::new(rect.loc - area.loc, rect.size))
            .collect();

        let prev = &self.shown_rects;
        if !prev.is_empty() && prev.len() != count {
            if let Some((clock, config)) = &self.count_anim_config {
                let mut from = prev.clone();
                if count > from.len() {
                    // New tabs start out empty at the end of the last one.
                    let last = *from.last().unwrap();
                    let (loc, size) = match self.config.position {
                        TabIndicatorPosition::Left | TabIndicatorPosition::Right => (
                            Point::from((last.loc.x, last.loc.y + last.size.h)),
                            Size::from((last.size.w, 0.)),
                        ),
                        TabIndicatorPosition::Top | TabIndicatorPosition::Bottom => (
                            Point::from((last.loc.x + last.size.w, last.loc.y)),
                            Size::from((0., last.size.h)),
                        ),
                    };
                    from.resize(count, Rectangle::new(loc, size));
                } else {
                    from.truncate(count);
                }
                let anim = Animation::new(clock.clone(), 0., 1., 0., *config);
                self.count_anim = Some((from, anim));
            }
        }

        let rects = match &self.count_anim {
            Some((from, anim)) if from.len() == count => {
                let t = anim.value();
                let lerp = |a: f64, b: f64| a + (b - a) * t;
                zip(from, &target)
                    .map(|(a, b)| {
                        Rectangle::new(
                            Point::from((lerp(a.loc.x, b.loc.x), lerp(a.loc.y, b.loc.y))),
                            Size::from((
                                lerp(a.size.w, b.size.w).max(0.),
                                lerp(a.size.h, b.size.h).max(0.),
                            )),
                        )
                    })
                    .collect()
            }
            _ => {
                self.count_anim = None;
                target
            }
        };

        self.shown_rects = rects.clone();
        rects
            .into_iter()
            .map(|rect| Rectangle::new(rect.loc + area.loc, rect.size))
            .collect()
    }

    pub fn hit(
        &self,
        area: Rectangle<f64, Logical>,
        tab_count: usize,
        scale: f64,
        point: Point<f64, Logical>,
    ) -> Option<usize> {
        if self.config.off {
            return None;
        }

        let count = tab_count;
        if self.config.hide_when_single_tab && count == 1 {
            return None;
        }

        self.tab_rects(area, count, scale)
            .enumerate()
            .find_map(|(idx, rect)| rect.contains(point).then_some(idx))
    }

    pub fn render(
        &self,
        renderer: &mut impl NiriRenderer,
        pos: Point<f64, Logical>,
        push: &mut dyn FnMut(TabIndicatorRenderElement),
    ) {
        let has_border_shader = BorderRenderElement::has_shader(renderer);
        if !has_border_shader {
            return;
        }

        for (shader, loc) in zip(&self.shaders, &self.shader_locs) {
            let elem = shader.clone().with_location(pos + *loc);
            push(TabIndicatorRenderElement::from(elem));
        }
    }

    /// Extra size occupied by the tab indicator.
    pub fn extra_size(&self, tab_count: usize, scale: f64) -> Size<f64, Logical> {
        if self.config.off
            || !self.config.place_within_column
            || (self.config.hide_when_single_tab && tab_count == 1)
        {
            return Size::from((0., 0.));
        }

        let round = |logical: f64| round_logical_in_physical(scale, logical);
        let width = round(self.config.width);
        let gap = round(self.config.gap);

        // No, I am *not* falling into the rabbit hole of "what if the tab indicator is wide enough
        // that it peeks from the other side of the window".
        let size = f64::max(0., width + gap);

        match self.config.position {
            TabIndicatorPosition::Left | TabIndicatorPosition::Right => Size::from((size, 0.)),
            TabIndicatorPosition::Top | TabIndicatorPosition::Bottom => Size::from((0., size)),
        }
    }

    /// Offset of the tabbed content due to space occupied by the tab indicator.
    pub fn content_offset(&self, tab_count: usize, scale: f64) -> Point<f64, Logical> {
        match self.config.position {
            TabIndicatorPosition::Left | TabIndicatorPosition::Top => {
                self.extra_size(tab_count, scale).to_point()
            }
            TabIndicatorPosition::Right | TabIndicatorPosition::Bottom => Point::from((0., 0.)),
        }
    }

    pub fn config(&self) -> niri_config::TabIndicator {
        self.config
    }
}

impl TabInfo {
    pub fn from_tile<W: LayoutElement>(
        tile: &Tile<W>,
        position: Point<f64, Logical>,
        is_active: bool,
        is_urgent: bool,
        config: &niri_config::TabIndicator,
    ) -> Self {
        let rules = tile.window().rules();
        let rule = rules.tab_indicator;

        let gradient_from_rule = || {
            let (color, gradient) = if is_urgent {
                (rule.urgent_color, rule.urgent_gradient)
            } else if is_active {
                (rule.active_color, rule.active_gradient)
            } else {
                (rule.inactive_color, rule.inactive_gradient)
            };
            let color = color.map(Gradient::from);
            gradient.or(color)
        };

        let gradient_from_config = || {
            let (color, gradient) = if is_urgent {
                (config.urgent_color, config.urgent_gradient)
            } else if is_active {
                (config.active_color, config.active_gradient)
            } else {
                (config.inactive_color, config.inactive_gradient)
            };
            let color = color.map(Gradient::from);
            gradient.or(color)
        };

        let gradient_from_border = || {
            // Come up with tab indicator gradient matching the focus ring or the border, whichever
            // one is enabled.
            let focus_ring_config = tile.focus_ring().config();
            let border_config = tile.border().config();
            let config = if focus_ring_config.off {
                border_config
            } else {
                focus_ring_config
            };

            let (color, gradient) = if is_urgent {
                (config.urgent_color, config.urgent_gradient)
            } else if is_active {
                (config.active_color, config.active_gradient)
            } else {
                (config.inactive_color, config.inactive_gradient)
            };
            gradient.unwrap_or_else(|| Gradient::from(color))
        };

        let gradient = gradient_from_rule()
            .or_else(gradient_from_config)
            .unwrap_or_else(gradient_from_border);

        let geometry = Rectangle::new(position, tile.animated_tile_size());

        TabInfo { gradient, geometry }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use niri_config::Color;

    use super::*;

    fn tabs(count: usize) -> impl Iterator<Item = TabInfo> {
        (0..count).map(|_| TabInfo {
            gradient: Gradient::from(Color::new_unpremul(1., 1., 1., 1.)),
            geometry: Rectangle::from_size(Size::from((100., 100.))),
        })
    }

    fn update(indicator: &mut TabIndicator, count: usize) {
        let area = Rectangle::from_size(Size::from((100., 400.)));
        indicator.update_render_elements(true, area, area, count, tabs(count), true, 1.);
    }

    #[test]
    fn tab_count_change_grows_new_tab_in() {
        let mut clock = Clock::with_time(Duration::ZERO);
        let mut indicator = TabIndicator::new(niri_config::TabIndicator::default());
        let config = niri_config::Animation {
            off: false,
            kind: niri_config::animations::Kind::Easing(niri_config::animations::EasingParams {
                duration_ms: 100,
                curve: niri_config::animations::Curve::Linear,
            }),
        };
        indicator.set_count_animation(clock.clone(), config);

        update(&mut indicator, 3);
        assert!(!indicator.are_animations_ongoing());
        let settled = indicator.bounds().unwrap();

        // A fourth tab starts out empty at the end, so the tabs still cover the same length.
        update(&mut indicator, 4);
        assert!(indicator.are_animations_ongoing());
        assert_eq!(indicator.shown_rects.len(), 4);
        assert_eq!(indicator.shown_rects[3].size.h, 0.);
        assert_eq!(indicator.bounds().unwrap(), settled);

        clock.set_unadjusted(Duration::from_millis(200));
        indicator.advance_animations();
        update(&mut indicator, 4);
        assert!(!indicator.are_animations_ongoing());
        assert!(indicator.shown_rects[3].size.h > 0.);
    }

    #[test]
    fn tab_count_change_is_instant_without_animation() {
        let mut indicator = TabIndicator::new(niri_config::TabIndicator::default());
        update(&mut indicator, 3);
        update(&mut indicator, 2);
        assert!(!indicator.are_animations_ongoing());
        assert_eq!(indicator.shown_rects.len(), 2);
    }
}
