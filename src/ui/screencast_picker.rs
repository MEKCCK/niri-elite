#[cfg(not(feature = "xdp-gnome-screencast"))]
mod disabled {
    use crate::layout::LayoutElementRenderElement;
    use crate::niri_render_elements;

    // Keep OutputRenderElements feature-independent.
    niri_render_elements! {
        ScreenCastPickerRenderElement<R> => {
            Placeholder = LayoutElementRenderElement<R>,
        }
    }
}

#[cfg(not(feature = "xdp-gnome-screencast"))]
pub use disabled::ScreenCastPickerRenderElement;

#[cfg(feature = "xdp-gnome-screencast")]
mod enabled {
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;
    use std::mem;
    use std::rc::Rc;
    use std::time::Duration;

    use niri_config::{Color, Config, ScreenCastPicker};
    use pango::{EllipsizeMode, FontDescription, Weight, WrapMode};
    use pangocairo::cairo::{self, ImageSurface};
    use smithay::backend::allocator::Fourcc;
    use smithay::backend::renderer::element::utils::{
        Relocate, RelocateRenderElement, RescaleRenderElement,
    };
    use smithay::backend::renderer::element::Kind;
    use smithay::backend::renderer::element::RenderElement;
    use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
    use smithay::output::{Output, WeakOutput};
    use smithay::utils::{Logical, Physical, Point, Rectangle, Scale, Size, Transform};

    use crate::animation::{Animation, Clock};
    use crate::dbus::mutter_screen_cast::StreamTargetId;
    use crate::dbus::niri_portal_screen_cast::{
        PickSourcesReply, PickSourcesRequest, PickerPersistMode, PickerRequestId, PickerSelection,
        PickerSourceTypes,
    };
    use crate::layout::LayoutElement as _;
    use crate::niri::Niri;
    use crate::niri_render_elements;
    use crate::render_helpers::memory::MemoryBuffer;
    use crate::render_helpers::offscreen::{OffscreenBuffer, OffscreenRenderElement};
    use crate::render_helpers::primary_gpu_texture::PrimaryGpuTextureRenderElement;
    use crate::render_helpers::renderer::NiriRenderer;
    use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};
    use crate::render_helpers::texture::{TextureBuffer, TextureRenderElement};
    use crate::render_helpers::{RenderCtx, RenderIntent, RenderTarget};
    use crate::ui::i18n::{messages_from_env, Messages};
    use crate::utils::{output_size, to_physical_precise_round};

    const PANEL_MAX_WIDTH: f64 = 1040.;
    const PANEL_MAX_HEIGHT: f64 = 760.;
    const PANEL_MARGIN: f64 = 24.;
    const PANEL_PADDING: f64 = 24.;
    const HEADER_HEIGHT: f64 = 48.;
    const CONTROL_HEIGHT: f64 = 42.;
    const CARDS_TOP: f64 = 198.;
    const COMPACT_BREAKPOINT_HEIGHT: f64 = 520.;
    const COMPACT_CARDS_TOP: f64 = 124.;
    const CARD_GAP: f64 = 14.;
    const CARD_MAX_COLUMNS: usize = 3;
    const CARD_MAX_ROWS: usize = 3;
    const CARD_MIN_WIDTH: f64 = 260.;
    const CARD_SINGLE_MAX_WIDTH: f64 = 640.;
    const CARD_FOOTER_HEIGHT: f64 = 48.;
    const CARD_MIN_PREVIEW_HEIGHT: f64 = 96.;
    const FONT: &str = "sans";
    const BACKDROP_COLOR: [f32; 4] = [0., 0., 0., 0.55];
    pub const PREVIEW_FRAME_INTERVAL: Duration = Duration::from_nanos(1_000_000_000 / 30);

    trait PickerColorExt {
        fn set_source(self, cr: &cairo::Context);
    }

    impl PickerColorExt for Color {
        fn set_source(self, cr: &cairo::Context) {
            cr.set_source_rgba(
                f64::from(self.r),
                f64::from(self.g),
                f64::from(self.b),
                f64::from(self.a),
            );
        }
    }

    #[derive(Clone, Copy, PartialEq)]
    struct PickerPalette {
        panel: Color,
        panel_border: Color,
        surface: Color,
        surface_hover: Color,
        surface_active: Color,
        preview_background: Color,
        border: Color,
        accent: Color,
        accent_hover: Color,
        text: Color,
        text_secondary: Color,
        text_disabled: Color,
        on_accent: Color,
        corner_radius: f64,
    }

    impl From<ScreenCastPicker> for PickerPalette {
        fn from(config: ScreenCastPicker) -> Self {
            let panel = config.background_color;
            let surface = config.text_color * 0.05;
            Self {
                corner_radius: config.corner_radius,
                panel,
                panel_border: config.text_color * 0.22,
                surface,
                surface_hover: config.text_color * 0.1,
                surface_active: config.accent_color * 0.16,
                preview_background: composite_color(panel, surface),
                border: config.text_color * 0.22,
                accent: config.accent_color,
                accent_hover: composite_color(config.accent_color, config.accent_text_color * 0.08),
                text: config.text_color,
                text_secondary: config.text_color * 0.7,
                text_disabled: config.text_color * 0.38,
                on_accent: config.accent_text_color,
            }
        }
    }

    fn composite_color(base: Color, overlay: Color) -> Color {
        let overlay_alpha = overlay.a.clamp(0., 1.);
        let base_alpha = base.a.clamp(0., 1.) * (1. - overlay_alpha);
        Color::from_array_premul([
            overlay.r * overlay_alpha + base.r * base_alpha,
            overlay.g * overlay_alpha + base.g * base_alpha,
            overlay.b * overlay_alpha + base.b * base_alpha,
            overlay_alpha + base_alpha,
        ])
    }

    #[derive(Clone, Debug)]
    pub struct PickerCandidate {
        pub target: StreamTargetId,
        pub title: String,
        pub subtitle: Option<String>,
    }

    impl PickerCandidate {
        pub fn monitor(name: String, title: String, subtitle: Option<String>) -> Self {
            Self {
                target: StreamTargetId::Output { name },
                title,
                subtitle,
            }
        }

        pub fn window(id: u64, title: Option<String>, app_id: Option<String>) -> Self {
            Self {
                target: StreamTargetId::Window { id },
                title: title.unwrap_or_else(|| String::from(messages_from_env().untitled_window)),
                subtitle: app_id,
            }
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum PickerView {
        Window,
        Display,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum HitTarget {
        None,
        Cancel,
        Share,
        Remember,
        WindowTab,
        DisplayTab,
        Candidate(usize),
    }

    struct OpenPicker {
        request: PickSourcesRequest,
        output: WeakOutput,
        displays: Vec<PickerCandidate>,
        windows: Vec<PickerCandidate>,
        view: PickerView,
        selected_display: Option<usize>,
        selected_window: Option<usize>,
        window_scroll_row: usize,
        window_scroll_remainder: f64,
        remember: bool,
        hovered: HitTarget,
        messages: &'static Messages,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    #[repr(usize)]
    enum PickerRenderVariant {
        Output = 0,
        Screencast = 1,
    }

    impl PickerRenderVariant {
        fn from_target(target: RenderTarget) -> Self {
            if target == RenderTarget::Screencast {
                Self::Screencast
            } else {
                Self::Output
            }
        }
    }

    struct PanelCache {
        scale: f64,
        output_size: Size<f64, Logical>,
        palette: PickerPalette,
        blocked_window_ids: Vec<u64>,
        buffer: TextureBuffer<GlesTexture>,
    }

    #[derive(Default)]
    struct DisplayPreview {
        buffer: OffscreenBuffer,
        element: Option<OffscreenRenderElement>,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct WindowPreviewCacheItem {
        id: u64,
        source_size: Size<i32, Logical>,
        bounds: Rectangle<i32, Physical>,
        blocked: bool,
    }

    #[derive(Clone, Debug, PartialEq)]
    struct WindowPreviewCacheKey {
        scale: f64,
        items: Vec<WindowPreviewCacheItem>,
    }

    #[derive(Default)]
    struct WindowPreviewCache {
        buffer: OffscreenBuffer,
        key: Option<WindowPreviewCacheKey>,
        element: Option<OffscreenRenderElement>,
        last_refresh: Option<Duration>,
    }

    impl WindowPreviewCache {
        fn update_key(&mut self, key: WindowPreviewCacheKey) -> bool {
            if self.key.as_ref() == Some(&key) {
                return false;
            }

            self.element = None;
            self.key = Some(key);
            true
        }

        fn should_refresh(&mut self, key: WindowPreviewCacheKey, now: Duration) -> bool {
            let key_changed = self.update_key(key);
            if !key_changed && !preview_refresh_due(self.last_refresh, now) {
                return false;
            }

            self.last_refresh = Some(now);
            true
        }

        fn clear(&mut self) {
            self.key = None;
            self.element = None;
            self.last_refresh = None;
            self.buffer.clear();
        }
    }

    struct FrozenBackdrop {
        output: WeakOutput,
        // Output and screencast variants.
        buffers: [TextureBuffer<GlesTexture>; 2],
    }

    pub struct DisplayPreviewRequest {
        pub name: String,
        pub size: Size<f64, Logical>,
    }

    enum PickerUiState {
        Closed,
        Open {
            picker: OpenPicker,
            animation: Option<Animation>,
        },
        Closing {
            picker: OpenPicker,
            animation: Animation,
        },
    }

    impl PickerUiState {
        fn open(&self) -> Option<&OpenPicker> {
            let Self::Open { picker, .. } = self else {
                return None;
            };
            Some(picker)
        }

        fn open_mut(&mut self) -> Option<&mut OpenPicker> {
            let Self::Open { picker, .. } = self else {
                return None;
            };
            Some(picker)
        }

        fn visible(&self) -> Option<&OpenPicker> {
            match self {
                Self::Open { picker, .. } | Self::Closing { picker, .. } => Some(picker),
                Self::Closed => None,
            }
        }

        fn progress(&self) -> f64 {
            match self {
                Self::Closed => 0.,
                Self::Open {
                    animation: Some(animation),
                    ..
                }
                | Self::Closing { animation, .. } => animation.clamped_value().clamp(0., 1.),
                Self::Open {
                    animation: None, ..
                } => 1.,
            }
        }
    }

    pub struct ScreenCastPickerUi {
        state: PickerUiState,
        panel_cache: RefCell<HashMap<(WeakOutput, PickerRenderVariant), PanelCache>>,
        backdrop_buffers: RefCell<HashMap<WeakOutput, SolidColorBuffer>>,
        preview_backgrounds: RefCell<HashMap<(WeakOutput, usize), SolidColorBuffer>>,
        display_previews: RefCell<HashMap<String, DisplayPreview>>,
        display_preview_last_refresh: Cell<Option<Duration>>,
        window_previews: RefCell<WindowPreviewCache>,
        frozen_backdrop: RefCell<Option<FrozenBackdrop>>,
        transition_buffers: [OffscreenBuffer; 2],
        clock: Clock,
        config: Rc<RefCell<Config>>,
    }

    niri_render_elements! {
        ScreenCastPickerRenderElement<R> => {
            Texture = PrimaryGpuTextureRenderElement,
            SolidColor = SolidColorRenderElement,
            Offscreen = OffscreenRenderElement,
            Placeholder = crate::layout::LayoutElementRenderElement<R>,
        }
    }

    impl ScreenCastPickerUi {
        pub fn new(clock: Clock, config: Rc<RefCell<Config>>) -> Self {
            Self {
                state: PickerUiState::Closed,
                panel_cache: RefCell::new(HashMap::new()),
                backdrop_buffers: RefCell::new(HashMap::new()),
                preview_backgrounds: RefCell::new(HashMap::new()),
                display_previews: RefCell::new(HashMap::new()),
                display_preview_last_refresh: Cell::new(None),
                window_previews: RefCell::new(WindowPreviewCache::default()),
                frozen_backdrop: RefCell::new(None),
                transition_buffers: [OffscreenBuffer::default(), OffscreenBuffer::default()],
                clock,
                config,
            }
        }

        pub fn is_open(&self) -> bool {
            self.is_visible()
        }

        pub fn is_visible(&self) -> bool {
            !matches!(self.state, PickerUiState::Closed)
        }

        pub fn output(&self) -> Option<Output> {
            self.state.open()?.output.upgrade()
        }

        pub fn is_host_output(&self, output: &Output) -> bool {
            self.state
                .visible()
                .and_then(|state| state.output.upgrade())
                .as_ref()
                == Some(output)
        }

        pub fn display_preview_requests(&self, output: &Output) -> Vec<DisplayPreviewRequest> {
            let Some(state) = self.state.visible() else {
                return Vec::new();
            };
            if state.view != PickerView::Display || state.output.upgrade().as_ref() != Some(output)
            {
                return Vec::new();
            }

            picker_geometry(output, state)
                .cards
                .into_iter()
                .filter_map(|(index, card)| {
                    let StreamTargetId::Output { name } = &state.displays.get(index)?.target else {
                        return None;
                    };
                    Some(DisplayPreviewRequest {
                        name: name.clone(),
                        size: card_preview_rect(card).size,
                    })
                })
                .collect()
        }

        pub fn display_previews_due(&self, output: &Output) -> bool {
            self.is_host_output(output)
                && self
                    .state
                    .visible()
                    .is_some_and(|state| state.view == PickerView::Display)
                && preview_refresh_due(
                    self.display_preview_last_refresh.get(),
                    self.clock.now_unadjusted(),
                )
        }

        pub fn mark_display_previews_refreshed(&self) {
            self.display_preview_last_refresh
                .set(Some(self.clock.now_unadjusted()));
        }

        pub fn is_display_previewing_host(&self, output: &Output) -> bool {
            let Some(state) = self.state.visible() else {
                return false;
            };
            if state.view != PickerView::Display || state.output.upgrade().as_ref() != Some(output)
            {
                return false;
            }

            let output_name = output.name();
            picker_geometry(output, state)
                .cards
                .iter()
                .any(|(index, _)| {
                    matches!(
                        state.displays.get(*index).map(|candidate| &candidate.target),
                        Some(StreamTargetId::Output { name }) if *name == output_name
                    )
                })
        }

        pub fn host_window_preview_ids(&self, output: &Output) -> Vec<u64> {
            let Some(state) = self.state.visible() else {
                return Vec::new();
            };
            if state.view != PickerView::Window || state.output.upgrade().as_ref() != Some(output) {
                return Vec::new();
            }

            picker_geometry(output, state)
                .cards
                .iter()
                .filter_map(|(index, _)| match &state.windows.get(*index)?.target {
                    StreamTargetId::Window { id } => Some(*id),
                    StreamTargetId::Output { .. } => None,
                })
                .collect()
        }

        pub fn begin_display_preview_frame(&self, requests: &[DisplayPreviewRequest]) {
            let mut previews = self.display_previews.borrow_mut();
            previews.retain(|name, _| requests.iter().any(|request| request.name == *name));
            for preview in previews.values_mut() {
                preview.element = None;
            }
        }

        pub fn update_display_preview<E>(
            &self,
            name: &str,
            renderer: &mut GlesRenderer,
            scale: Scale<f64>,
            bounds: Rectangle<i32, Physical>,
            elements: &[E],
        ) where
            E: RenderElement<GlesRenderer>,
        {
            let mut previews = self.display_previews.borrow_mut();
            let preview = previews.entry(name.to_owned()).or_default();
            preview.element = None;
            match preview
                .buffer
                .render_with_bounds(renderer, scale, bounds, elements)
            {
                Ok((element, _sync, _data)) => preview.element = Some(element),
                Err(err) => warn!("error rendering display preview for {name}: {err:?}"),
            }
        }

        pub fn preview_host_for_source(&self, source: &Output) -> Option<Output> {
            let state = self.state.visible()?;
            if state.view != PickerView::Display {
                return None;
            }
            let host = state.output.upgrade()?;
            if host == *source {
                return None;
            }
            let source_name = source.name();
            picker_geometry(&host, state)
                .cards
                .iter()
                .any(|(index, _)| {
                    matches!(
                        state.displays.get(*index).map(|candidate| &candidate.target),
                        Some(StreamTargetId::Output { name }) if *name == source_name
                    )
                })
                .then_some(host)
        }

        pub fn open(
            &mut self,
            request: PickSourcesRequest,
            output: Output,
            displays: Vec<PickerCandidate>,
            windows: Vec<PickerCandidate>,
            frozen_backdrop: Option<[TextureBuffer<GlesTexture>; 2]>,
        ) -> bool {
            if self.is_visible() {
                request.fail("screen cast picker is already open");
                return false;
            }
            if request.options.multiple {
                request.fail("multiple source selection is not implemented yet");
                return false;
            }

            let allow_display = request
                .options
                .source_types
                .contains(PickerSourceTypes::MONITOR);
            let allow_window = request
                .options
                .source_types
                .contains(PickerSourceTypes::WINDOW);
            let displays = if allow_display { displays } else { Vec::new() };
            let windows = if allow_window { windows } else { Vec::new() };

            let view = if !displays.is_empty() {
                PickerView::Display
            } else if !windows.is_empty() {
                PickerView::Window
            } else {
                request.fail("no matching screen cast sources are available");
                return false;
            };
            let remember = request.options.persist_mode != PickerPersistMode::None;
            let picker = OpenPicker {
                request,
                output: output.downgrade(),
                selected_display: (!displays.is_empty()).then_some(0),
                selected_window: (!windows.is_empty()).then_some(0),
                window_scroll_row: 0,
                window_scroll_remainder: 0.,
                displays,
                windows,
                view,
                remember,
                hovered: HitTarget::None,
                messages: messages_from_env(),
            };
            self.clear_caches();
            self.frozen_backdrop
                .replace(frozen_backdrop.map(|buffers| FrozenBackdrop {
                    output: output.downgrade(),
                    buffers,
                }));
            self.state = PickerUiState::Open {
                picker,
                animation: Some(self.animation(0., 1.)),
            };
            self.invalidate();
            true
        }

        pub fn cancel(&mut self) -> bool {
            self.finish(PickSourcesReply::Cancelled)
        }

        pub fn cancel_request(&mut self, id: &PickerRequestId) -> bool {
            if !self
                .state
                .open()
                .is_some_and(|state| &state.request.id == id)
            {
                return false;
            }
            self.cancel()
        }

        pub fn confirm(&mut self) -> bool {
            let Some(state) = self.state.open() else {
                return false;
            };
            let target = state
                .selected_candidate()
                .map(|candidate| candidate.target.clone());
            let Some(target) = target else {
                return false;
            };
            let persist_mode = if state.remember {
                state.request.options.persist_mode
            } else {
                PickerPersistMode::None
            };
            self.finish(PickSourcesReply::Selected(PickerSelection {
                sources: vec![target],
                persist_mode,
            }))
        }

        pub fn move_selection(&mut self, delta: isize) -> bool {
            let Some(state) = self.state.open_mut() else {
                return false;
            };
            let len = state.candidates().len();
            if len == 0 {
                return false;
            }
            let current = state.selected().unwrap_or(0);
            let next = (current as isize + delta).rem_euclid(len as isize) as usize;
            state.set_selected(next);
            state.hovered = HitTarget::None;
            self.invalidate();
            true
        }

        pub fn scroll_window(&mut self, delta: f64) -> bool {
            let Some(state) = self.state.open_mut() else {
                return false;
            };
            if state.view != PickerView::Window {
                return false;
            }
            let Some(output) = state.output.upgrade() else {
                return false;
            };

            state.window_scroll_remainder += delta;
            let steps = state.window_scroll_remainder.trunc() as isize;
            if steps == 0 {
                return false;
            }
            state.window_scroll_remainder -= steps as f64;
            let (columns, visible_rows) =
                card_grid_dimensions(picker_panel_size(&output), state.windows.len());
            let old_row = effective_scroll_row(
                state.windows.len(),
                state.selected_window.unwrap_or(0),
                columns,
                visible_rows,
                state.window_scroll_row,
            );
            let max_scroll_row = state
                .windows
                .len()
                .div_ceil(columns)
                .saturating_sub(visible_rows);
            let new_row = if steps > 0 {
                old_row.saturating_add(steps as usize).min(max_scroll_row)
            } else {
                old_row.saturating_sub(steps.unsigned_abs())
            };
            if new_row == old_row {
                return false;
            }
            state.window_scroll_row = new_row;
            let selected = state.selected_window.unwrap_or(0);
            let selected_row = selected / columns;
            let selected_column = selected % columns;
            if selected_row < new_row {
                state.selected_window =
                    Some((new_row * columns + selected_column).min(state.windows.len() - 1));
            } else if selected_row >= new_row + visible_rows {
                let last_visible_row = new_row + visible_rows - 1;
                state.selected_window = Some(
                    (last_visible_row * columns + selected_column).min(state.windows.len() - 1),
                );
            }
            state.hovered = HitTarget::None;
            self.invalidate();
            true
        }

        pub fn toggle_view(&mut self) -> bool {
            let Some(state) = self.state.open_mut() else {
                return false;
            };
            let next = match state.view {
                PickerView::Window if !state.displays.is_empty() => PickerView::Display,
                PickerView::Display if !state.windows.is_empty() => PickerView::Window,
                _ => return false,
            };
            state.view = next;
            state.hovered = HitTarget::None;
            self.invalidate_preview_refresh();
            self.invalidate();
            true
        }

        pub fn toggle_remember(&mut self) -> bool {
            let Some(state) = self.state.open_mut() else {
                return false;
            };
            if state.request.options.persist_mode == PickerPersistMode::None {
                return false;
            }
            state.remember = !state.remember;
            self.invalidate();
            true
        }

        pub fn pointer_motion(&mut self, output: &Output, position: Point<f64, Logical>) -> bool {
            let Some(state) = self.state.open_mut() else {
                return false;
            };
            if state.output.upgrade().as_ref() != Some(output) {
                if state.hovered == HitTarget::None {
                    return false;
                }
                state.hovered = HitTarget::None;
                self.invalidate();
                return true;
            }
            let hit = picker_geometry(output, state).hit_test(position);
            if state.hovered == hit {
                return false;
            }
            state.hovered = hit;
            self.invalidate();
            true
        }

        pub fn pointer_activate(&mut self, output: &Output, position: Point<f64, Logical>) -> bool {
            let Some(state) = self.state.open_mut() else {
                return false;
            };
            if state.output.upgrade().as_ref() != Some(output) {
                return self.cancel();
            }
            match picker_geometry(output, state).hit_test(position) {
                HitTarget::Cancel => self.cancel(),
                HitTarget::Share => self.confirm(),
                HitTarget::Remember => self.toggle_remember(),
                HitTarget::WindowTab if !state.windows.is_empty() => {
                    state.view = PickerView::Window;
                    state.hovered = HitTarget::WindowTab;
                    self.invalidate_preview_refresh();
                    self.invalidate();
                    true
                }
                HitTarget::DisplayTab if !state.displays.is_empty() => {
                    state.view = PickerView::Display;
                    state.hovered = HitTarget::DisplayTab;
                    self.invalidate_preview_refresh();
                    self.invalidate();
                    true
                }
                HitTarget::Candidate(index) if index < state.candidates().len() => {
                    state.set_selected(index);
                    state.hovered = HitTarget::Candidate(index);
                    self.invalidate();
                    true
                }
                _ => false,
            }
        }

        pub fn remove_window(&mut self, id: u64) -> bool {
            let Some(state) = self.state.open_mut() else {
                return false;
            };
            let old_len = state.windows.len();
            state
                .windows
                .retain(|candidate| candidate.target != StreamTargetId::Window { id });
            if state.windows.len() == old_len {
                return false;
            }
            state.hovered = HitTarget::None;
            state.selected_window = (!state.windows.is_empty()).then_some(0);
            state.window_scroll_row = 0;
            state.window_scroll_remainder = 0.;
            if state.view == PickerView::Window && state.windows.is_empty() {
                if state.displays.is_empty() {
                    return self.finish(PickSourcesReply::Failed(String::from(
                        "all matching screen cast sources disappeared",
                    )));
                }
                state.view = PickerView::Display;
                self.invalidate_preview_refresh();
            }
            self.invalidate();
            true
        }

        pub fn render<R: NiriRenderer>(
            &self,
            niri: &Niri,
            output: &Output,
            mut ctx: RenderCtx<R>,
            push: &mut dyn FnMut(ScreenCastPickerRenderElement<R>),
        ) -> bool {
            let Some(state) = self.state.visible() else {
                return false;
            };
            let Some(host_output) = state.output.upgrade() else {
                return false;
            };
            let target = ctx.target;
            let variant = PickerRenderVariant::from_target(ctx.target);
            let progress = self.state.progress() as f32;
            if progress <= 0. {
                return self.render_frozen_backdrop(output, target, push);
            }

            if host_output != *output || progress >= 1. {
                self.render_contents(niri, output, ctx, progress, push);
                return self.render_frozen_backdrop(output, target, push);
            }

            let scale = output.current_scale().fractional_scale();
            let mut ctx = ctx.as_gles();
            let mut elements = Vec::new();
            self.render_contents(niri, output, ctx.r(), 1., &mut |element| {
                elements.push(element)
            });
            match self.transition_buffers[variant as usize].render(
                ctx.renderer,
                Scale::from(scale),
                &elements,
            ) {
                Ok((element, _sync, _data)) => {
                    push(ScreenCastPickerRenderElement::<R>::Offscreen(
                        element.with_alpha(progress),
                    ));
                }
                Err(err) => {
                    warn!("error rendering screen cast picker transition: {err:?}");
                    self.render_backdrop(
                        output,
                        output_size(output),
                        output.downgrade(),
                        progress,
                        push,
                    );
                }
            }
            self.render_frozen_backdrop(output, target, push)
        }

        fn render_frozen_backdrop<R: NiriRenderer>(
            &self,
            output: &Output,
            target: RenderTarget,
            push: &mut dyn FnMut(ScreenCastPickerRenderElement<R>),
        ) -> bool {
            let backdrop = self.frozen_backdrop.borrow();
            let Some(backdrop) = backdrop.as_ref() else {
                return false;
            };
            if backdrop.output.upgrade().as_ref() != Some(output) {
                return false;
            }

            let index = usize::from(target == RenderTarget::Screencast);
            let element = TextureRenderElement::from_texture_buffer(
                backdrop.buffers[index].clone(),
                Point::new(0., 0.),
                1.,
                None,
                Some(output_size(output)),
                Kind::Unspecified,
            );
            push(ScreenCastPickerRenderElement::<R>::Texture(
                PrimaryGpuTextureRenderElement(element),
            ));
            true
        }

        fn render_contents<R: NiriRenderer>(
            &self,
            niri: &Niri,
            output: &Output,
            ctx: RenderCtx<R>,
            alpha: f32,
            push: &mut dyn FnMut(ScreenCastPickerRenderElement<R>),
        ) {
            let Some(state) = self.state.visible() else {
                return;
            };
            let Some(host_output) = state.output.upgrade() else {
                return;
            };

            let size = output_size(output);
            let weak = output.downgrade();

            if host_output != *output {
                self.render_backdrop(output, size, weak, alpha, push);
                return;
            }

            let scale = output.current_scale().fractional_scale();
            let palette = PickerPalette::from(self.config.borrow().screen_cast_picker);
            let geometry = picker_geometry(output, state);
            let variant = PickerRenderVariant::from_target(ctx.target);
            let blocked_window_ids = if variant == PickerRenderVariant::Screencast {
                geometry
                    .cards
                    .iter()
                    .filter_map(|(index, _)| {
                        let StreamTargetId::Window { id } = &state.candidates().get(*index)?.target
                        else {
                            return None;
                        };
                        let id = *id;
                        let (_, mapped) = niri
                            .layout
                            .windows()
                            .find(|(_, mapped)| mapped.id().get() == id)?;
                        RenderTarget::Screencast
                            .should_block_out(mapped.rules().block_out_from)
                            .then_some(id)
                    })
                    .collect()
            } else {
                Vec::new()
            };
            let mut cache = self.panel_cache.borrow_mut();
            cache.retain(|(output, _), _| output.is_alive());
            let cache_key = (weak.clone(), variant);
            let stale = cache.get(&cache_key).is_none_or(|entry| {
                entry.scale != scale
                    || entry.output_size != size
                    || entry.palette != palette
                    || entry.blocked_window_ids != blocked_window_ids
            });
            if stale {
                cache.remove(&cache_key);
                match render_panel(
                    ctx.renderer.as_gles_renderer(),
                    output,
                    state,
                    &palette,
                    &blocked_window_ids,
                ) {
                    Ok(buffer) => {
                        cache.insert(
                            cache_key.clone(),
                            PanelCache {
                                scale,
                                output_size: size,
                                palette,
                                blocked_window_ids,
                                buffer,
                            },
                        );
                    }
                    Err(err) => {
                        warn!("error rendering screen cast picker: {err:?}");
                        return;
                    }
                }
            }
            let Some(entry) = cache.get(&cache_key) else {
                return;
            };
            let element = TextureRenderElement::from_texture_buffer(
                entry.buffer.clone(),
                geometry.panel.loc,
                alpha,
                None,
                None,
                Kind::Unspecified,
            );
            push(ScreenCastPickerRenderElement::<R>::Texture(
                PrimaryGpuTextureRenderElement(element),
            ));

            if state.view == PickerView::Window {
                let output_scale = Scale::from(scale);
                let mut snapshots = Vec::new();
                let mut items = Vec::new();
                let mut bounds = None;
                for (index, card) in &geometry.cards {
                    let Some(StreamTargetId::Window { id }) = state
                        .candidates()
                        .get(*index)
                        .map(|candidate| &candidate.target)
                    else {
                        continue;
                    };
                    let Some((_, mapped)) = niri
                        .layout
                        .windows()
                        .find(|(_, mapped)| mapped.id().get() == *id)
                    else {
                        continue;
                    };
                    let preview = card_preview_rect(*card);
                    let source_size = mapped.size();
                    if source_size.w <= 0 || source_size.h <= 0 {
                        continue;
                    }
                    let source_size_f64 = source_size.to_f64();
                    let factor = f64::min(
                        preview.size.w / source_size_f64.w,
                        preview.size.h / source_size_f64.h,
                    )
                    .max(0.001);
                    let rendered_size = source_size_f64.upscale(factor);
                    let location = preview.loc
                        + (preview.size.to_point() - rendered_size.to_point()).downscale(2.);
                    let preview_bounds = preview.to_physical_precise_round(scale);
                    let blocked =
                        RenderTarget::Screencast.should_block_out(mapped.rules().block_out_from);
                    bounds = Some(bounds.map_or(preview_bounds, |bounds: Rectangle<_, _>| {
                        bounds.merge(preview_bounds)
                    }));
                    items.push(WindowPreviewCacheItem {
                        id: *id,
                        source_size,
                        bounds: preview_bounds,
                        blocked,
                    });
                    snapshots.push((mapped, factor, location));
                }

                let key = WindowPreviewCacheKey { scale, items };
                let mut cache = self.window_previews.borrow_mut();
                if cache.should_refresh(key, self.clock.now_unadjusted()) {
                    let mut preview_ctx = RenderCtx {
                        renderer: ctx.renderer.as_gles_renderer(),
                        target: RenderTarget::Screencast,
                        intent: RenderIntent::PickerPreview,
                        xray: ctx.xray,
                    };
                    let mut elements = Vec::new();
                    for (mapped, factor, location) in snapshots {
                        mapped.render_normal(
                            preview_ctx.r(),
                            Point::new(0., 0.),
                            output_scale,
                            1.,
                            &mut |element| {
                                let element = RescaleRenderElement::from_element(
                                    element,
                                    Point::new(0, 0),
                                    factor,
                                );
                                let element = RelocateRenderElement::from_element(
                                    element,
                                    location.to_physical_precise_round(scale),
                                    Relocate::Relative,
                                );
                                elements.push(element);
                            },
                        );
                    }

                    if let Some(bounds) = bounds {
                        match cache.buffer.render_with_bounds(
                            preview_ctx.renderer,
                            output_scale,
                            bounds,
                            &elements,
                        ) {
                            Ok((element, _sync, _data)) => cache.element = Some(element),
                            Err(err) => {
                                warn!("error rendering window picker previews: {err:?}")
                            }
                        }
                    }
                }

                if let Some(element) = cache.element.clone() {
                    push(ScreenCastPickerRenderElement::<R>::Offscreen(
                        element.with_alpha(alpha),
                    ));
                }
            } else {
                let previews = self.display_previews.borrow();
                for (index, card) in &geometry.cards {
                    let Some(StreamTargetId::Output { name }) = state
                        .candidates()
                        .get(*index)
                        .map(|candidate| &candidate.target)
                    else {
                        continue;
                    };
                    let Some(element) = previews
                        .get(name)
                        .and_then(|preview| preview.element.as_ref())
                    else {
                        continue;
                    };
                    let preview = card_preview_rect(*card);
                    let rendered_size = element.logical_size();
                    let location = preview.loc
                        + (preview.size.to_point() - rendered_size.to_point()).downscale(2.);
                    let element = element.clone().with_alpha(alpha).with_offset(location);
                    push(ScreenCastPickerRenderElement::<R>::Offscreen(element));
                }
            }

            self.render_preview_backgrounds(&geometry, weak.clone(), alpha, push);
            self.render_backdrop(output, size, weak, alpha, push);
        }

        fn render_preview_backgrounds<R: NiriRenderer>(
            &self,
            geometry: &PickerGeometry,
            weak: WeakOutput,
            alpha: f32,
            push: &mut dyn FnMut(ScreenCastPickerRenderElement<R>),
        ) {
            let mut backgrounds = self.preview_backgrounds.borrow_mut();
            backgrounds.retain(|(output, _), _| output.is_alive());
            let color =
                PickerPalette::from(self.config.borrow().screen_cast_picker).preview_background;
            for (index, card) in &geometry.cards {
                let preview = card_preview_rect(*card);
                let buffer = backgrounds
                    .entry((weak.clone(), *index))
                    .or_insert_with(|| SolidColorBuffer::new(preview.size, color));
                buffer.resize(preview.size);
                buffer.set_color(color);
                push(ScreenCastPickerRenderElement::<R>::SolidColor(
                    SolidColorRenderElement::from_buffer(
                        buffer,
                        preview.loc,
                        alpha,
                        Kind::Unspecified,
                    ),
                ));
            }
        }

        fn render_backdrop<R: NiriRenderer>(
            &self,
            _output: &Output,
            size: Size<f64, Logical>,
            weak: WeakOutput,
            alpha: f32,
            push: &mut dyn FnMut(ScreenCastPickerRenderElement<R>),
        ) {
            let mut backdrops = self.backdrop_buffers.borrow_mut();
            backdrops.retain(|output, _| output.is_alive());
            let backdrop = backdrops
                .entry(weak)
                .or_insert_with(|| SolidColorBuffer::new(size, BACKDROP_COLOR));
            backdrop.resize(size);
            backdrop.set_color(BACKDROP_COLOR);
            push(ScreenCastPickerRenderElement::<R>::SolidColor(
                SolidColorRenderElement::from_buffer(
                    backdrop,
                    Point::new(0., 0.),
                    alpha,
                    Kind::Unspecified,
                ),
            ));
        }

        fn finish(&mut self, reply: PickSourcesReply) -> bool {
            let state = mem::replace(&mut self.state, PickerUiState::Closed);
            let PickerUiState::Open { picker, animation } = state else {
                self.state = state;
                return false;
            };

            let from = animation
                .as_ref()
                .map_or(1., |animation| animation.clamped_value().clamp(0., 1.));
            let _ = picker.request.reply.try_send(reply);
            let animation = self.animation(from, 0.);
            if animation.is_done() {
                self.clear_caches();
            } else {
                self.state = PickerUiState::Closing { picker, animation };
            }
            true
        }

        pub fn cancel_immediately(&mut self) -> bool {
            let state = mem::replace(&mut self.state, PickerUiState::Closed);
            match state {
                PickerUiState::Closed => false,
                PickerUiState::Open { picker, .. } => {
                    let _ = picker.request.reply.try_send(PickSourcesReply::Cancelled);
                    self.clear_caches();
                    true
                }
                PickerUiState::Closing { .. } => {
                    self.clear_caches();
                    true
                }
            }
        }

        pub fn advance_animations(&mut self) -> bool {
            let mut closed = false;
            match &mut self.state {
                PickerUiState::Open { animation, .. } => {
                    animation.take_if(|animation| animation.is_done());
                }
                PickerUiState::Closing { animation, .. } => {
                    if animation.is_done() {
                        closed = true;
                    }
                }
                PickerUiState::Closed => {}
            }
            if closed {
                self.state = PickerUiState::Closed;
                self.clear_caches();
            }
            closed
        }

        pub fn are_animations_ongoing(&self, output: &Output) -> bool {
            match &self.state {
                PickerUiState::Open {
                    animation: Some(animation),
                    ..
                }
                | PickerUiState::Closing { animation, .. } => !animation.is_done(),
                PickerUiState::Open {
                    picker,
                    animation: None,
                } => picker.output.upgrade().as_ref() == Some(output),
                PickerUiState::Closed => false,
            }
        }

        fn animation(&self, from: f64, to: f64) -> Animation {
            let config = self
                .config
                .borrow()
                .animations
                .screen_cast_picker_open_close
                .0;
            Animation::new(self.clock.clone(), from, to, 0., config)
        }

        fn invalidate(&self) {
            self.panel_cache.borrow_mut().clear();
        }

        fn invalidate_preview_refresh(&self) {
            self.display_preview_last_refresh.set(None);
            self.window_previews.borrow_mut().last_refresh = None;
        }

        fn clear_caches(&self) {
            self.invalidate();
            self.backdrop_buffers.borrow_mut().clear();
            self.preview_backgrounds.borrow_mut().clear();
            self.display_previews.borrow_mut().clear();
            self.display_preview_last_refresh.set(None);
            self.window_previews.borrow_mut().clear();
            self.frozen_backdrop.borrow_mut().take();
            for buffer in &self.transition_buffers {
                buffer.clear();
            }
        }
    }

    fn preview_refresh_due(last_refresh: Option<Duration>, now: Duration) -> bool {
        last_refresh.is_none_or(|last| now.saturating_sub(last) >= PREVIEW_FRAME_INTERVAL)
    }

    impl OpenPicker {
        fn candidates(&self) -> &[PickerCandidate] {
            match self.view {
                PickerView::Window => &self.windows,
                PickerView::Display => &self.displays,
            }
        }

        fn selected(&self) -> Option<usize> {
            match self.view {
                PickerView::Window => self.selected_window,
                PickerView::Display => self.selected_display,
            }
        }

        fn set_selected(&mut self, selected: usize) {
            match self.view {
                PickerView::Window => self.selected_window = Some(selected),
                PickerView::Display => self.selected_display = Some(selected),
            }
        }

        fn selected_candidate(&self) -> Option<&PickerCandidate> {
            self.candidates().get(self.selected()?)
        }
    }

    struct PickerGeometry {
        panel: Rectangle<f64, Logical>,
        cancel: Rectangle<f64, Logical>,
        share: Rectangle<f64, Logical>,
        title: Option<Rectangle<f64, Logical>>,
        description: Option<Rectangle<f64, Logical>>,
        remember: Option<Rectangle<f64, Logical>>,
        window_tab: Rectangle<f64, Logical>,
        display_tab: Rectangle<f64, Logical>,
        cards: Vec<(usize, Rectangle<f64, Logical>)>,
    }

    #[derive(Clone, Copy)]
    struct PickerMetrics {
        compact: bool,
        padding: f64,
        cards_top: f64,
        bottom_padding: f64,
    }

    fn picker_metrics(panel_size: Size<f64, Logical>) -> PickerMetrics {
        let compact = panel_size.h < COMPACT_BREAKPOINT_HEIGHT;
        if compact {
            PickerMetrics {
                compact,
                padding: 12.,
                cards_top: COMPACT_CARDS_TOP,
                bottom_padding: 8.,
            }
        } else {
            PickerMetrics {
                compact,
                padding: PANEL_PADDING,
                cards_top: CARDS_TOP,
                bottom_padding: PANEL_PADDING,
            }
        }
    }

    impl PickerGeometry {
        fn hit_test(&self, position: Point<f64, Logical>) -> HitTarget {
            if self.cancel.contains(position) {
                return HitTarget::Cancel;
            }
            if self.share.contains(position) {
                return HitTarget::Share;
            }
            if self
                .remember
                .is_some_and(|remember| remember.contains(position))
            {
                return HitTarget::Remember;
            }
            if self.window_tab.contains(position) {
                return HitTarget::WindowTab;
            }
            if self.display_tab.contains(position) {
                return HitTarget::DisplayTab;
            }
            self.cards
                .iter()
                .find_map(|(index, rect)| rect.contains(position).then_some(*index))
                .map(HitTarget::Candidate)
                .unwrap_or(HitTarget::None)
        }
    }

    fn picker_geometry(output: &Output, state: &OpenPicker) -> PickerGeometry {
        let output_size = output_size(output);
        let panel_size = picker_panel_size(output);
        let width = panel_size.w;
        let height = panel_size.h;
        let panel = Rectangle::new(
            Point::from(((output_size.w - width) / 2., (output_size.h - height) / 2.)),
            Size::from((width, height)),
        );
        let local =
            |x, y, w, h| Rectangle::new(panel.loc + Point::from((x, y)), Size::from((w, h)));
        let metrics = picker_metrics(panel.size);
        let header_y = if metrics.compact { 8. } else { 16. };
        let header_height = if metrics.compact { 36. } else { HEADER_HEIGHT };
        let header_padding = metrics.padding.min((width / 6.).max(0.));
        let header_gap = 16_f64.min(((width - header_padding * 2.) / 3.).max(0.));
        let preferred_button_width: f64 = if metrics.compact { 80. } else { 100. };
        let max_button_width = ((width - header_padding * 2. - header_gap) / 2.).max(0.1);
        let button_width = preferred_button_width.min(max_button_width);
        let cancel = local(header_padding, header_y, button_width, header_height);
        let share = local(
            width - header_padding - button_width,
            header_y,
            button_width,
            header_height,
        );
        let title_x = header_padding + button_width + header_gap / 2.;
        let title_width = (width - title_x * 2.).max(0.);
        let title = (title_width >= 80.).then(|| {
            local(
                title_x,
                header_y + if metrics.compact { 6. } else { 9. },
                title_width,
                24.,
            )
        });
        let description = (!metrics.compact).then(|| {
            local(
                metrics.padding,
                68.,
                (width - metrics.padding * 2.).max(1.),
                20.,
            )
        });
        let remember_y = if metrics.compact { 48. } else { 92. };
        let remember_height = if metrics.compact { 28. } else { 34. };
        let remember = (state.request.options.persist_mode != PickerPersistMode::None).then(|| {
            local(
                metrics.padding,
                remember_y,
                (width - metrics.padding * 2.).max(1.),
                remember_height,
            )
        });
        let tabs_width = 300_f64.min((width - metrics.padding * 2.).max(1.));
        let tab_x = (width - tabs_width) / 2.;
        let tabs_y = if metrics.compact { 80. } else { 136. };
        let tabs_height = if metrics.compact { 36. } else { CONTROL_HEIGHT };
        let window_tab = local(tab_x, tabs_y, tabs_width / 2., tabs_height);
        let display_tab = local(
            tab_x + tabs_width / 2.,
            tabs_y,
            tabs_width / 2.,
            tabs_height,
        );
        let selected = state.selected().unwrap_or(0);
        let card_rects = card_layout(
            panel.size,
            state.candidates().len(),
            selected,
            state.window_scroll_row,
        );
        let cards = card_rects
            .into_iter()
            .map(|(index, rect)| (index, Rectangle::new(panel.loc + rect.loc, rect.size)))
            .collect();
        PickerGeometry {
            panel,
            cancel,
            share,
            title,
            description,
            remember,
            window_tab,
            display_tab,
            cards,
        }
    }

    fn picker_panel_size(output: &Output) -> Size<f64, Logical> {
        let output_size = output_size(output);
        Size::from((
            PANEL_MAX_WIDTH.min((output_size.w - PANEL_MARGIN * 2.).max(1.)),
            PANEL_MAX_HEIGHT.min((output_size.h - PANEL_MARGIN * 2.).max(1.)),
        ))
    }

    fn card_layout(
        panel_size: Size<f64, Logical>,
        candidate_count: usize,
        selected: usize,
        scroll_row: usize,
    ) -> Vec<(usize, Rectangle<f64, Logical>)> {
        if candidate_count == 0 {
            return Vec::new();
        }

        let metrics = picker_metrics(panel_size);
        let available_width = (panel_size.w - metrics.padding * 2.).max(1.);
        let (columns, max_rows_by_height) = card_grid_dimensions(panel_size, candidate_count);
        let total_rows = candidate_count.div_ceil(columns);
        let visible_rows = total_rows.min(max_rows_by_height);
        let selected = selected.min(candidate_count - 1);
        let scroll_row =
            effective_scroll_row(candidate_count, selected, columns, visible_rows, scroll_row);
        let first_index = scroll_row * columns;
        let visible_count = (candidate_count - first_index).min(visible_rows * columns);

        let available_height = (panel_size.h - metrics.cards_top - metrics.bottom_padding).max(1.);
        let sizing_rows = visible_rows;
        let card_width = card_width_for(panel_size, columns, candidate_count);
        let max_card_height =
            (available_height - CARD_GAP * (sizing_rows - 1) as f64) / sizing_rows as f64;
        let desired_card_height = card_width * 9. / 16. + CARD_FOOTER_HEIGHT + 16.;
        let card_min_height = CARD_MIN_PREVIEW_HEIGHT + CARD_FOOTER_HEIGHT + 12.;
        let card_height = desired_card_height
            .max(card_min_height)
            .min(max_card_height)
            .max(1.);
        let actual_rows = visible_count.div_ceil(columns);
        let grid_height = card_height * actual_rows as f64 + CARD_GAP * (actual_rows - 1) as f64;
        let grid_y = metrics.cards_top + (available_height - grid_height).max(0.) / 2.;

        let cards = (0..visible_count)
            .map(|local_index| {
                let index = first_index + local_index;
                let row = local_index / columns;
                let col = local_index % columns;
                let row_start = row * columns;
                let row_count = (visible_count - row_start).min(columns);
                let row_width = card_width * row_count as f64 + CARD_GAP * (row_count - 1) as f64;
                let row_x = metrics.padding + (available_width - row_width) / 2.;
                let x = row_x + (card_width + CARD_GAP) * col as f64;
                let y = grid_y + (card_height + CARD_GAP) * row as f64;
                (
                    index,
                    Rectangle::new(Point::from((x, y)), Size::from((card_width, card_height))),
                )
            })
            .collect();
        cards
    }

    fn effective_scroll_row(
        candidate_count: usize,
        selected: usize,
        columns: usize,
        visible_rows: usize,
        scroll_row: usize,
    ) -> usize {
        let total_rows = candidate_count.div_ceil(columns);
        let max_scroll_row = total_rows.saturating_sub(visible_rows);
        let mut scroll_row = scroll_row.min(max_scroll_row);
        if candidate_count == 0 {
            return scroll_row;
        }

        let selected_row = selected.min(candidate_count - 1) / columns;
        if selected_row < scroll_row {
            scroll_row = selected_row;
        } else if selected_row >= scroll_row + visible_rows {
            scroll_row = (selected_row + 1 - visible_rows).min(max_scroll_row);
        }
        scroll_row
    }

    fn card_grid_dimensions(
        panel_size: Size<f64, Logical>,
        candidate_count: usize,
    ) -> (usize, usize) {
        let metrics = picker_metrics(panel_size);
        let available_width = (panel_size.w - metrics.padding * 2.).max(1.);
        let max_columns = (((available_width + CARD_GAP) / (CARD_MIN_WIDTH + CARD_GAP)).floor()
            as usize)
            .clamp(1, CARD_MAX_COLUMNS);
        let desired_columns = match candidate_count {
            1 => 1,
            2..=4 => 2,
            _ => 3,
        };
        let columns = desired_columns.min(max_columns);
        let available_height = (panel_size.h - metrics.cards_top - metrics.bottom_padding).max(1.);
        let card_min_height = CARD_MIN_PREVIEW_HEIGHT + CARD_FOOTER_HEIGHT + 12.;
        let visible_rows = (1..=CARD_MAX_ROWS)
            .rev()
            .find(|rows| {
                available_height >= card_min_height * *rows as f64 + CARD_GAP * (rows - 1) as f64
            })
            .unwrap_or(1);
        (columns, visible_rows)
    }

    fn card_width_for(
        panel_size: Size<f64, Logical>,
        columns: usize,
        candidate_count: usize,
    ) -> f64 {
        let metrics = picker_metrics(panel_size);
        let available_width = (panel_size.w - metrics.padding * 2.).max(1.);
        let mut card_width = (available_width - CARD_GAP * (columns - 1) as f64) / columns as f64;
        if candidate_count == 1 {
            card_width = card_width.min(CARD_SINGLE_MAX_WIDTH);
        }
        card_width
    }

    fn render_panel(
        renderer: &mut GlesRenderer,
        output: &Output,
        state: &OpenPicker,
        palette: &PickerPalette,
        blocked_window_ids: &[u64],
    ) -> anyhow::Result<TextureBuffer<GlesTexture>> {
        let scale = output.current_scale().fractional_scale();
        let geometry = picker_geometry(output, state);
        let logical_size = geometry.panel.size;
        let width = to_physical_precise_round(scale, logical_size.w);
        let height = to_physical_precise_round(scale, logical_size.h);
        let surface = ImageSurface::create(cairo::Format::ARgb32, width, height)?;
        let cr = cairo::Context::new(&surface)?;
        cr.scale(scale, scale);

        let radius = palette.corner_radius;
        rounded_rectangle(&cr, 0., 0., logical_size.w, logical_size.h, radius);
        palette.panel.set_source(&cr);
        cr.fill()?;
        rounded_rectangle(
            &cr,
            1.,
            1.,
            logical_size.w - 2.,
            logical_size.h - 2.,
            (radius - 1.).max(0.),
        );
        palette.panel_border.set_source(&cr);
        cr.set_line_width(2.);
        cr.stroke()?;

        draw_button(
            &cr,
            local_rect(geometry.cancel, geometry.panel.loc),
            state.messages.cancel,
            state.hovered == HitTarget::Cancel,
            false,
            palette,
        )?;
        draw_button(
            &cr,
            local_rect(geometry.share, geometry.panel.loc),
            state.messages.share,
            state.hovered == HitTarget::Share,
            true,
            palette,
        )?;
        if let Some(title) = geometry.title {
            let title = local_rect(title, geometry.panel.loc);
            draw_text(
                &cr,
                state.messages.share_screen,
                title.loc.x,
                title.loc.y,
                title.size.w,
                20.,
                Weight::Bold,
                palette.text,
                pango::Alignment::Center,
            );
        }
        if let Some(description_rect) = geometry.description {
            let description_rect = local_rect(description_rect, geometry.panel.loc);
            let app_id = (!state.request.options.app_id.is_empty())
                .then_some(state.request.options.app_id.as_str());
            let description = (state.messages.share_description)(app_id);
            draw_text(
                &cr,
                &description,
                description_rect.loc.x,
                description_rect.loc.y,
                description_rect.size.w,
                15.,
                Weight::Normal,
                palette.text_secondary,
                pango::Alignment::Center,
            );
        }

        if let Some(remember_rect) = geometry.remember {
            let remember_rect = local_rect(remember_rect, geometry.panel.loc);
            let checkbox = Rectangle::new(
                remember_rect.loc + Point::from((4., 5.)),
                Size::from((22., 22.)),
            );
            rounded_rectangle(
                &cr,
                checkbox.loc.x,
                checkbox.loc.y,
                checkbox.size.w,
                checkbox.size.h,
                5.,
            );
            if state.remember {
                palette.accent.set_source(&cr);
                cr.fill()?;
                cr.move_to(checkbox.loc.x + 5., checkbox.loc.y + 11.);
                cr.line_to(checkbox.loc.x + 9., checkbox.loc.y + 16.);
                cr.line_to(checkbox.loc.x + 18., checkbox.loc.y + 6.);
                palette.on_accent.set_source(&cr);
                cr.set_line_width(2.5);
                cr.stroke()?;
            } else {
                palette.border.set_source(&cr);
                cr.set_line_width(1.5);
                cr.stroke()?;
            }
            draw_text(
                &cr,
                state.messages.remember_selection,
                remember_rect.loc.x + 34.,
                remember_rect.loc.y + 5.,
                remember_rect.size.w - 38.,
                15.,
                Weight::Normal,
                palette.text,
                pango::Alignment::Left,
            );
        }

        draw_tab(
            &cr,
            local_rect(geometry.window_tab, geometry.panel.loc),
            state.messages.window,
            state.view == PickerView::Window,
            state.hovered == HitTarget::WindowTab,
            !state.windows.is_empty(),
            palette,
        )?;
        draw_tab(
            &cr,
            local_rect(geometry.display_tab, geometry.panel.loc),
            state.messages.display,
            state.view == PickerView::Display,
            state.hovered == HitTarget::DisplayTab,
            !state.displays.is_empty(),
            palette,
        )?;

        for (index, rect) in &geometry.cards {
            let candidate = &state.candidates()[*index];
            let blocked = matches!(
                &candidate.target,
                StreamTargetId::Window { id } if blocked_window_ids.contains(id)
            );
            let rect = local_rect(*rect, geometry.panel.loc);
            let selected = state.selected() == Some(*index);
            let hovered = state.hovered == HitTarget::Candidate(*index);
            rounded_rectangle(&cr, rect.loc.x, rect.loc.y, rect.size.w, rect.size.h, 9.);
            if selected {
                palette.surface_active.set_source(&cr);
            } else if hovered {
                palette.surface_hover.set_source(&cr);
            } else {
                palette.surface.set_source(&cr);
            }
            cr.fill_preserve()?;
            if selected {
                palette.accent.set_source(&cr);
                cr.set_line_width(3.);
            } else {
                palette.border.set_source(&cr);
                cr.set_line_width(1.);
            }
            cr.stroke()?;

            let preview = card_preview_rect(rect);
            cr.save()?;
            rounded_rectangle(
                &cr,
                preview.loc.x,
                preview.loc.y,
                preview.size.w,
                preview.size.h,
                6.,
            );
            cr.set_operator(cairo::Operator::Clear);
            cr.fill()?;
            cr.restore()?;

            let footer_y = rect.loc.y + rect.size.h - CARD_FOOTER_HEIGHT;
            let (title, subtitle) = candidate_labels(candidate, blocked, state.messages);
            draw_text(
                &cr,
                title,
                rect.loc.x + 12.,
                footer_y + 6.,
                rect.size.w - 24.,
                14.,
                Weight::Bold,
                palette.text,
                pango::Alignment::Left,
            );
            draw_text(
                &cr,
                subtitle,
                rect.loc.x + 12.,
                footer_y + 27.,
                rect.size.w - 24.,
                11.,
                Weight::Normal,
                palette.text_secondary,
                pango::Alignment::Left,
            );
        }

        drop(cr);
        let data = surface.take_data().unwrap();
        let memory = MemoryBuffer::new(
            data.to_vec(),
            Fourcc::Argb8888,
            (width, height),
            scale,
            Transform::Normal,
        );
        Ok(TextureBuffer::from_memory_buffer(renderer, &memory)?)
    }

    fn candidate_labels<'a>(
        candidate: &'a PickerCandidate,
        blocked: bool,
        messages: &'a Messages,
    ) -> (&'a str, &'a str) {
        if blocked {
            return (messages.protected_window, messages.hidden_from_screen_share);
        }

        let fallback_subtitle = match &candidate.target {
            StreamTargetId::Output { .. } => messages.display,
            StreamTargetId::Window { .. } => messages.window,
        };
        (
            &candidate.title,
            candidate.subtitle.as_deref().unwrap_or(fallback_subtitle),
        )
    }

    fn local_rect(
        rect: Rectangle<f64, Logical>,
        panel_location: Point<f64, Logical>,
    ) -> Rectangle<f64, Logical> {
        Rectangle::new(rect.loc - panel_location, rect.size)
    }

    fn card_preview_rect(card: Rectangle<f64, Logical>) -> Rectangle<f64, Logical> {
        Rectangle::new(
            card.loc + Point::from((8., 8.)),
            Size::from((
                (card.size.w - 16.).max(1.),
                (card.size.h - CARD_FOOTER_HEIGHT - 12.).max(1.),
            )),
        )
    }

    fn rounded_rectangle(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
        let r = r.min(w / 2.).min(h / 2.);
        cr.new_sub_path();
        cr.arc(x + w - r, y + r, r, -std::f64::consts::FRAC_PI_2, 0.);
        cr.arc(x + w - r, y + h - r, r, 0., std::f64::consts::FRAC_PI_2);
        cr.arc(
            x + r,
            y + h - r,
            r,
            std::f64::consts::FRAC_PI_2,
            std::f64::consts::PI,
        );
        cr.arc(
            x + r,
            y + r,
            r,
            std::f64::consts::PI,
            std::f64::consts::PI * 1.5,
        );
        cr.close_path();
    }

    fn draw_button(
        cr: &cairo::Context,
        rect: Rectangle<f64, Logical>,
        label: &str,
        hovered: bool,
        primary: bool,
        palette: &PickerPalette,
    ) -> anyhow::Result<()> {
        rounded_rectangle(cr, rect.loc.x, rect.loc.y, rect.size.w, rect.size.h, 10.);
        if primary {
            if hovered {
                palette.accent_hover.set_source(cr);
            } else {
                palette.accent.set_source(cr);
            }
        } else {
            if hovered {
                palette.surface_hover.set_source(cr);
            } else {
                palette.surface.set_source(cr);
            }
        }
        cr.fill()?;
        draw_text(
            cr,
            label,
            rect.loc.x,
            rect.loc.y + 12.,
            rect.size.w,
            16.,
            Weight::Bold,
            if primary {
                palette.on_accent
            } else {
                palette.text
            },
            pango::Alignment::Center,
        );
        Ok(())
    }

    fn draw_tab(
        cr: &cairo::Context,
        rect: Rectangle<f64, Logical>,
        label: &str,
        active: bool,
        hovered: bool,
        enabled: bool,
        palette: &PickerPalette,
    ) -> anyhow::Result<()> {
        rounded_rectangle(cr, rect.loc.x, rect.loc.y, rect.size.w, rect.size.h, 8.);
        let color = if active {
            palette.surface_active
        } else if hovered && enabled {
            palette.surface_hover
        } else {
            palette.surface
        };
        color.set_source(cr);
        cr.fill()?;
        draw_text(
            cr,
            label,
            rect.loc.x,
            rect.loc.y + 10.,
            rect.size.w,
            15.,
            Weight::Bold,
            if enabled {
                palette.text
            } else {
                palette.text_disabled
            },
            pango::Alignment::Center,
        );
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_text(
        cr: &cairo::Context,
        text: &str,
        x: f64,
        y: f64,
        width: f64,
        size: f64,
        weight: Weight,
        color: Color,
        alignment: pango::Alignment,
    ) {
        let layout = pangocairo::functions::create_layout(cr);
        let mut font = FontDescription::from_string(FONT);
        font.set_absolute_size(size * f64::from(pango::SCALE));
        font.set_weight(weight);
        layout.set_font_description(Some(&font));
        layout.set_text(text);
        layout.set_width((width * f64::from(pango::SCALE)) as i32);
        layout.set_wrap(WrapMode::WordChar);
        layout.set_ellipsize(EllipsizeMode::End);
        layout.set_single_paragraph_mode(true);
        layout.set_alignment(alignment);
        cr.move_to(x, y);
        color.set_source(cr);
        pangocairo::functions::show_layout(cr, &layout);
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn default_palette_matches_screenshot_ui_neutrals() {
            let palette = PickerPalette::from(ScreenCastPicker::default());
            let border = composite_color(palette.panel, palette.panel_border);

            assert_eq!(palette.panel, Color::new_unpremul(0.1, 0.1, 0.1, 1.));
            assert_eq!(palette.accent, Color::new_unpremul(1., 1., 1., 1.));
            assert_eq!(palette.text, Color::new_unpremul(1., 1., 1., 1.));
            assert!((palette.preview_background.r - 0.145).abs() < 0.0001);
            assert!((border.r - 0.298).abs() < 0.0001);
        }

        #[test]
        fn single_candidate_is_centered() {
            let size = Size::from((1040., 760.));
            let layout = card_layout(size, 1, 0, 0);
            let rect = layout[0].1;

            assert!((rect.loc.x + rect.size.w / 2. - size.w / 2.).abs() < 0.001);
        }

        #[test]
        fn incomplete_rows_are_centered() {
            let layout = card_layout(Size::from((1040., 760.)), 5, 0, 0);

            assert_eq!(layout.len(), 5);
            assert!(layout[3].1.loc.x > layout[0].1.loc.x);
            assert_eq!(layout[3].1.loc.y, layout[4].1.loc.y);
        }

        #[test]
        fn nine_candidates_fit_and_larger_lists_scroll() {
            let layout = card_layout(Size::from((1040., 760.)), 9, 0, 0);
            assert_eq!(layout.len(), 9);
            assert_eq!(layout[8].0, 8);

            let layout = card_layout(Size::from((1040., 760.)), 10, 9, 0);
            assert_eq!(layout.len(), 7);
            assert_eq!(layout[0].0, 3);
            assert!(layout.iter().any(|(index, _)| *index == 9));
        }

        #[test]
        fn narrow_panels_reduce_the_column_count() {
            let layout = card_layout(Size::from((600., 760.)), 6, 0, 0);

            assert_eq!(layout.len(), 6);
        }

        #[test]
        fn short_panels_use_one_visible_row() {
            let layout = card_layout(Size::from((600., 312.)), 6, 0, 0);

            assert_eq!(layout.len(), 2);
            assert!(card_preview_rect(layout[0].1).size.h >= CARD_MIN_PREVIEW_HEIGHT);
        }

        #[test]
        fn medium_panels_only_use_rows_that_fit_the_minimum_preview_height() {
            let layout = card_layout(Size::from((1040., 672.)), 9, 0, 0);

            assert_eq!(layout.len(), 6);
            assert!(card_preview_rect(layout[0].1).size.h >= CARD_MIN_PREVIEW_HEIGHT);
        }

        #[test]
        fn narrow_panels_preserve_the_minimum_preview_height() {
            let layout = card_layout(Size::from((192., 752.)), 9, 0, 0);

            assert_eq!(layout.len(), 3);
            assert!(card_preview_rect(layout[0].1).size.h >= CARD_MIN_PREVIEW_HEIGHT);
        }

        #[test]
        fn manual_scroll_does_not_snap_back_to_the_selection() {
            let layout = card_layout(Size::from((1040., 760.)), 12, 3, 1);

            assert_eq!(layout[0].0, 3);
            assert!(!layout.iter().any(|(index, _)| *index == 0));
        }

        #[test]
        fn unchanged_window_preview_layout_reuses_snapshot() {
            let item = WindowPreviewCacheItem {
                id: 1,
                source_size: Size::from((800, 600)),
                bounds: Rectangle::new(Point::from((10, 20)), Size::from((320, 240))),
                blocked: false,
            };
            let key = WindowPreviewCacheKey {
                scale: 1.5,
                items: vec![item],
            };
            let mut cache = WindowPreviewCache::default();

            assert!(cache.update_key(key.clone()));
            assert!(!cache.update_key(key.clone()));

            let mut resized = key;
            resized.items[0].source_size = Size::from((801, 600));
            assert!(cache.update_key(resized));
        }

        #[test]
        fn preview_refresh_is_capped_at_thirty_fps() {
            let start = Duration::from_secs(1);

            assert!(preview_refresh_due(None, start));
            assert!(!preview_refresh_due(
                Some(start),
                start + PREVIEW_FRAME_INTERVAL - Duration::from_nanos(1)
            ));
            assert!(preview_refresh_due(
                Some(start),
                start + PREVIEW_FRAME_INTERVAL
            ));
        }

        #[test]
        fn structural_preview_changes_refresh_immediately() {
            let item = WindowPreviewCacheItem {
                id: 1,
                source_size: Size::from((800, 600)),
                bounds: Rectangle::new(Point::from((10, 20)), Size::from((320, 240))),
                blocked: false,
            };
            let key = WindowPreviewCacheKey {
                scale: 1.,
                items: vec![item],
            };
            let mut cache = WindowPreviewCache::default();

            assert!(cache.should_refresh(key.clone(), Duration::ZERO));
            assert!(!cache.should_refresh(key.clone(), Duration::from_millis(1)));

            let mut changed = key;
            changed.items[0].blocked = true;
            assert!(cache.should_refresh(changed, Duration::from_millis(1)));
        }

        #[test]
        fn screencast_uses_a_separate_picker_render_variant() {
            assert_eq!(
                PickerRenderVariant::from_target(RenderTarget::Output),
                PickerRenderVariant::Output
            );
            assert_eq!(
                PickerRenderVariant::from_target(RenderTarget::ScreenCapture),
                PickerRenderVariant::Output
            );
            assert_eq!(
                PickerRenderVariant::from_target(RenderTarget::Screencast),
                PickerRenderVariant::Screencast
            );
        }

        #[test]
        fn blocked_window_labels_do_not_expose_metadata() {
            let candidate = PickerCandidate::window(
                1,
                Some(String::from("Secret title")),
                Some(String::from("secret.app")),
            );
            let messages = messages_from_env();

            let (title, subtitle) = candidate_labels(&candidate, true, messages);

            assert_eq!(title, messages.protected_window);
            assert_eq!(subtitle, messages.hidden_from_screen_share);
            assert_ne!(title, candidate.title);
            assert_ne!(subtitle, candidate.subtitle.as_deref().unwrap());
        }
    }
}

#[cfg(feature = "xdp-gnome-screencast")]
pub use enabled::*;
