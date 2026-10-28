use std::cell::{Cell, OnceCell, RefCell};

use niri_config::utils::Flag;
use niri_config::workspace::WorkspaceName;
use niri_config::{
    CenterFocusedColumn, FloatOrInt, OutputName, Struts, TabIndicatorLength, TabIndicatorPosition,
    WorkspaceReference,
};
use proptest::prelude::*;
use proptest_derive::Arbitrary;
use smithay::output::{Mode, PhysicalProperties, Subpixel};
use smithay::utils::Rectangle;

use super::*;

mod animations;
mod fullscreen;

impl<W: LayoutElement> Default for Layout<W> {
    fn default() -> Self {
        Self::with_options(Clock::with_time(Duration::ZERO), Default::default())
    }
}

#[derive(Debug)]
struct TestWindowInner {
    id: usize,
    parent_id: Cell<Option<usize>>,
    bbox: Cell<Rectangle<i32, Logical>>,
    initial_bbox: Rectangle<i32, Logical>,
    requested_size: Cell<Option<Size<i32, Logical>>>,
    // Emulates a configure that the window hasn't responded to yet.
    pending_configure: Cell<bool>,
    // Emulates the window ignoring the compositor-provided size.
    forced_size: Cell<Option<Size<i32, Logical>>>,
    min_size: Size<i32, Logical>,
    max_size: Size<i32, Logical>,
    pending_sizing_mode: Cell<SizingMode>,
    pending_activated: Cell<bool>,
    sizing_mode: Cell<SizingMode>,
    is_windowed_fullscreen: Cell<bool>,
    is_pending_windowed_fullscreen: Cell<bool>,
    is_minimized: Cell<bool>,
    animate_next_configure: Cell<bool>,
    animation_snapshot: RefCell<Option<LayoutElementRenderSnapshot>>,
    rules: ResolvedWindowRules,
}

#[derive(Debug, Clone)]
struct TestWindow(Rc<TestWindowInner>);

#[derive(Debug, Clone, Arbitrary)]
struct TestWindowParams {
    #[proptest(strategy = "1..=5usize")]
    id: usize,
    #[proptest(strategy = "arbitrary_parent_id()")]
    parent_id: Option<usize>,
    is_floating: bool,
    #[proptest(strategy = "arbitrary_bbox()")]
    bbox: Rectangle<i32, Logical>,
    #[proptest(strategy = "arbitrary_min_max_size()")]
    min_max_size: (Size<i32, Logical>, Size<i32, Logical>),
    #[proptest(strategy = "prop::option::of(arbitrary_rules())")]
    rules: Option<ResolvedWindowRules>,
}

impl TestWindowParams {
    pub fn new(id: usize) -> Self {
        Self {
            id,
            parent_id: None,
            is_floating: false,
            bbox: Rectangle::from_size(Size::from((100, 200))),
            min_max_size: Default::default(),
            rules: None,
        }
    }
}

impl TestWindow {
    fn new(params: TestWindowParams) -> Self {
        Self(Rc::new(TestWindowInner {
            id: params.id,
            parent_id: Cell::new(params.parent_id),
            bbox: Cell::new(params.bbox),
            initial_bbox: params.bbox,
            requested_size: Cell::new(None),
            pending_configure: Cell::new(false),
            forced_size: Cell::new(None),
            min_size: params.min_max_size.0,
            max_size: params.min_max_size.1,
            pending_sizing_mode: Cell::new(SizingMode::Normal),
            pending_activated: Cell::new(false),
            sizing_mode: Cell::new(SizingMode::Normal),
            is_windowed_fullscreen: Cell::new(false),
            is_pending_windowed_fullscreen: Cell::new(false),
            is_minimized: Cell::new(false),
            animate_next_configure: Cell::new(false),
            animation_snapshot: RefCell::new(None),
            rules: params.rules.unwrap_or_default(),
        }))
    }

    fn communicate(&self) -> bool {
        let mut changed = false;

        let size = self.0.forced_size.get().or(self.0.requested_size.get());
        if let Some(size) = size {
            assert!(size.w >= 0);
            assert!(size.h >= 0);

            let mut new_bbox = self.0.initial_bbox;
            if size.w != 0 {
                new_bbox.size.w = size.w;
            }
            if size.h != 0 {
                new_bbox.size.h = size.h;
            }

            if self.0.bbox.get() != new_bbox {
                if self.0.animate_next_configure.get() {
                    self.0.animation_snapshot.replace(Some(RenderSnapshot {
                        contents: Vec::new(),
                        contents_with_blocked_out_bg: None,
                        blocked_out_contents: Vec::new(),
                        block_out_from: None,
                        size: self.0.bbox.get().size.to_f64(),
                        texture: OnceCell::new(),
                        texture_with_blocked_out_bg: Default::default(),
                        blocked_out_texture: OnceCell::new(),
                    }));
                }

                self.0.bbox.set(new_bbox);
                changed = true;
            }
        }

        self.0.animate_next_configure.set(false);
        self.0.pending_configure.set(false);

        if self.0.sizing_mode.get() != self.0.pending_sizing_mode.get() {
            self.0.sizing_mode.set(self.0.pending_sizing_mode.get());
            changed = true;
        }

        if self.0.is_windowed_fullscreen.get() != self.0.is_pending_windowed_fullscreen.get() {
            self.0
                .is_windowed_fullscreen
                .set(self.0.is_pending_windowed_fullscreen.get());
            changed = true;
        }

        changed
    }
}

impl LayoutElement for TestWindow {
    type Id = usize;

    fn id(&self) -> &Self::Id {
        &self.0.id
    }

    fn size(&self) -> Size<i32, Logical> {
        self.0.bbox.get().size
    }

    fn buf_loc(&self) -> Point<i32, Logical> {
        (0, 0).into()
    }

    fn is_in_input_region(&self, _point: Point<f64, Logical>) -> bool {
        false
    }

    fn request_size(
        &mut self,
        size: Size<i32, Logical>,
        mode: SizingMode,
        _animate: bool,
        _transaction: Option<Transaction>,
    ) {
        if self.0.requested_size.get() != Some(size) {
            self.0.requested_size.set(Some(size));
            self.0.pending_configure.set(true);
            self.0.animate_next_configure.set(true);
        }

        self.0.pending_sizing_mode.set(mode);

        if mode.is_fullscreen() {
            self.0.is_pending_windowed_fullscreen.set(false);
        }
    }

    fn min_size(&self) -> Size<i32, Logical> {
        self.0.min_size
    }

    fn max_size(&self) -> Size<i32, Logical> {
        self.0.max_size
    }

    fn is_wl_surface(&self, _wl_surface: &WlSurface) -> bool {
        false
    }

    fn set_preferred_scale_transform(&self, _scale: output::Scale, _transform: Transform) {}

    fn has_ssd(&self) -> bool {
        false
    }

    fn output_enter(&self, _output: &Output) {}

    fn output_leave(&self, _output: &Output) {}

    fn set_offscreen_data(&self, _data: Option<OffscreenData>) {}

    fn set_activated(&mut self, active: bool) {
        self.0.pending_activated.set(active);
    }

    fn set_bounds(&self, _bounds: Size<i32, Logical>) {}

    fn is_ignoring_opacity_window_rule(&self) -> bool {
        false
    }

    fn configure_intent(&self) -> ConfigureIntent {
        ConfigureIntent::CanSend
    }

    fn send_pending_configure(&mut self) {}

    fn set_active_in_column(&mut self, _active: bool) {}

    fn set_floating(&mut self, _floating: bool) {}

    fn sizing_mode(&self) -> SizingMode {
        self.0.sizing_mode.get()
    }

    fn pending_sizing_mode(&self) -> SizingMode {
        self.0.pending_sizing_mode.get()
    }

    fn requested_size(&self) -> Option<Size<i32, Logical>> {
        self.0.requested_size.get()
    }

    fn expected_size(&self) -> Option<Size<i32, Logical>> {
        if self.sizing_mode().is_fullscreen() {
            return None;
        }

        let current = self.size();

        // Mimic Mapped: a requested size only counts while the configure is in flight; once the
        // window responds (communicates), its own committed size wins.
        if self.0.pending_configure.get() {
            let mut requested = self.0.requested_size.get().unwrap_or_default();
            if requested.w == 0 {
                requested.w = current.w;
            }
            if requested.h == 0 {
                requested.h = current.h;
            }
            return Some(requested);
        }

        Some(current)
    }

    fn is_windowed_fullscreen(&self) -> bool {
        self.0.is_windowed_fullscreen.get()
    }

    fn is_pending_windowed_fullscreen(&self) -> bool {
        self.0.is_pending_windowed_fullscreen.get()
    }

    fn request_windowed_fullscreen(&mut self, value: bool) {
        self.0.is_pending_windowed_fullscreen.set(value);
    }

    fn is_child_of(&self, parent: &Self) -> bool {
        self.0.parent_id.get() == Some(parent.0.id)
    }

    fn refresh(&self) {}

    fn rules(&self) -> &ResolvedWindowRules {
        &self.0.rules
    }

    fn take_animation_snapshot(&mut self) -> Option<LayoutElementRenderSnapshot> {
        self.0.animation_snapshot.take()
    }

    fn set_interactive_resize(&mut self, _data: Option<InteractiveResizeData>) {}

    fn cancel_interactive_resize(&mut self) {}

    fn on_commit(&mut self, _serial: Serial) {}

    fn interactive_resize_data(&self) -> Option<InteractiveResizeData> {
        None
    }

    fn is_urgent(&self) -> bool {
        false
    }

    fn is_minimized(&self) -> bool {
        self.0.is_minimized.get()
    }

    fn set_minimized(&mut self, minimized: bool) {
        self.0.is_minimized.set(minimized);
    }
}

fn arbitrary_size() -> impl Strategy<Value = Size<i32, Logical>> {
    any::<(u16, u16)>().prop_map(|(w, h)| Size::from((w.max(1).into(), h.max(1).into())))
}

fn arbitrary_bbox() -> impl Strategy<Value = Rectangle<i32, Logical>> {
    any::<(i16, i16, u16, u16)>().prop_map(|(x, y, w, h)| {
        let loc: Point<i32, _> = Point::from((x.into(), y.into()));
        let size: Size<i32, _> = Size::from((w.max(1).into(), h.max(1).into()));
        Rectangle::new(loc, size)
    })
}

fn arbitrary_size_change() -> impl Strategy<Value = SizeChange> {
    prop_oneof![
        (0..).prop_map(SizeChange::SetFixed),
        (0f64..).prop_map(SizeChange::SetProportion),
        any::<i32>().prop_map(SizeChange::AdjustFixed),
        any::<f64>().prop_map(SizeChange::AdjustProportion),
        // Interactive resize can have negative values here.
        Just(SizeChange::SetFixed(-100)),
    ]
}

fn arbitrary_position_change() -> impl Strategy<Value = PositionChange> {
    prop_oneof![
        (-1000f64..1000f64).prop_map(PositionChange::SetFixed),
        any::<f64>().prop_map(PositionChange::SetProportion),
        (-1000f64..1000f64).prop_map(PositionChange::AdjustFixed),
        any::<f64>().prop_map(PositionChange::AdjustProportion),
        any::<f64>().prop_map(PositionChange::SetFixed),
        any::<f64>().prop_map(PositionChange::AdjustFixed),
    ]
}

fn arbitrary_min_max() -> impl Strategy<Value = (i32, i32)> {
    prop_oneof![
        Just((0, 0)),
        (1..65536).prop_map(|n| (n, n)),
        (1..65536).prop_map(|min| (min, 0)),
        (1..).prop_map(|max| (0, max)),
        (1..65536, 1..).prop_map(|(min, max): (i32, i32)| (min, max.max(min))),
    ]
}

fn arbitrary_min_max_size() -> impl Strategy<Value = (Size<i32, Logical>, Size<i32, Logical>)> {
    prop_oneof![
        5 => (arbitrary_min_max(), arbitrary_min_max()).prop_map(
            |((min_w, max_w), (min_h, max_h))| {
                let min_size = Size::from((min_w, min_h));
                let max_size = Size::from((max_w, max_h));
                (min_size, max_size)
            },
        ),
        1 => arbitrary_min_max().prop_map(|(w, h)| {
            let size = Size::from((w, h));
            (size, size)
        }),
    ]
}

prop_compose! {
    fn arbitrary_rules()(
        focus_ring in arbitrary_focus_ring(),
        border in arbitrary_border(),
    ) -> ResolvedWindowRules {
        ResolvedWindowRules {
            focus_ring,
            border,
            ..ResolvedWindowRules::default()
        }
    }
}

fn arbitrary_view_offset_gesture_delta() -> impl Strategy<Value = f64> {
    prop_oneof![(-10f64..10f64), (-50000f64..50000f64),]
}

fn arbitrary_resize_edge() -> impl Strategy<Value = ResizeEdge> {
    prop_oneof![
        Just(ResizeEdge::RIGHT),
        Just(ResizeEdge::BOTTOM),
        Just(ResizeEdge::LEFT),
        Just(ResizeEdge::TOP),
        Just(ResizeEdge::BOTTOM_RIGHT),
        Just(ResizeEdge::BOTTOM_LEFT),
        Just(ResizeEdge::TOP_RIGHT),
        Just(ResizeEdge::TOP_LEFT),
        Just(ResizeEdge::empty()),
    ]
}

fn arbitrary_scale() -> impl Strategy<Value = f64> {
    prop_oneof![Just(1.), Just(1.5), Just(2.),]
}

fn arbitrary_msec_delta() -> impl Strategy<Value = i32> {
    prop_oneof![
        1 => Just(-1000),
        2 => Just(-10),
        1 => Just(0),
        2 => Just(10),
        6 => Just(1000),
    ]
}

fn arbitrary_parent_id() -> impl Strategy<Value = Option<usize>> {
    prop_oneof![
        5 => Just(None),
        1 => prop::option::of(1..=5usize),
    ]
}

fn arbitrary_scroll_direction() -> impl Strategy<Value = ScrollDirection> {
    prop_oneof![Just(ScrollDirection::Left), Just(ScrollDirection::Right)]
}

fn arbitrary_column_display() -> impl Strategy<Value = ColumnDisplay> {
    prop_oneof![Just(ColumnDisplay::Normal), Just(ColumnDisplay::Tabbed)]
}

#[derive(Debug, Clone, Arbitrary)]
enum Op {
    AddOutput(#[proptest(strategy = "1..=5usize")] usize),
    AddScaledOutput {
        #[proptest(strategy = "1..=5usize")]
        id: usize,
        #[proptest(strategy = "arbitrary_scale()")]
        scale: f64,
        #[proptest(strategy = "prop::option::of(arbitrary_layout_part().prop_map(Box::new))")]
        layout_config: Option<Box<niri_config::LayoutPart>>,
    },
    RemoveOutput(#[proptest(strategy = "1..=5usize")] usize),
    FocusOutput(#[proptest(strategy = "1..=5usize")] usize),
    UpdateOutputLayoutConfig {
        #[proptest(strategy = "1..=5usize")]
        id: usize,
        #[proptest(strategy = "prop::option::of(arbitrary_layout_part().prop_map(Box::new))")]
        layout_config: Option<Box<niri_config::LayoutPart>>,
    },
    AddNamedWorkspace {
        #[proptest(strategy = "1..=5usize")]
        ws_name: usize,
        #[proptest(strategy = "proptest::collection::vec(1..=5usize, 0..=3)")]
        output_names: Vec<usize>,
        #[proptest(strategy = "prop::option::of(arbitrary_layout_part().prop_map(Box::new))")]
        layout_config: Option<Box<niri_config::LayoutPart>>,
    },
    UnnameWorkspace {
        #[proptest(strategy = "1..=5usize")]
        ws_name: usize,
    },
    UpdateWorkspaceLayoutConfig {
        #[proptest(strategy = "1..=5usize")]
        ws_name: usize,
        #[proptest(strategy = "prop::option::of(arbitrary_layout_part().prop_map(Box::new))")]
        layout_config: Option<Box<niri_config::LayoutPart>>,
    },
    AddWindow {
        params: TestWindowParams,
    },
    AddWindowNextTo {
        params: TestWindowParams,
        #[proptest(strategy = "1..=5usize")]
        next_to_id: usize,
    },
    AddWindowToNamedWorkspace {
        params: TestWindowParams,
        #[proptest(strategy = "1..=5usize")]
        ws_name: usize,
    },
    CloseWindow(#[proptest(strategy = "1..=5usize")] usize),
    FullscreenWindow(#[proptest(strategy = "1..=5usize")] usize),
    SetFullscreenWindow {
        #[proptest(strategy = "1..=5usize")]
        window: usize,
        is_fullscreen: bool,
    },
    ToggleWindowedFullscreen(#[proptest(strategy = "1..=5usize")] usize),
    MinimizeWindow(#[proptest(strategy = "1..=5usize")] usize),
    UnminimizeWindow(#[proptest(strategy = "1..=5usize")] usize),
    SetWindowMinimized {
        #[proptest(strategy = "1..=5usize")]
        window: usize,
        minimize: bool,
    },
    FocusColumnLeft,
    FocusColumnRight,
    FocusColumnFirst,
    FocusColumnLast,
    FocusColumnRightOrFirst,
    FocusColumnLeftOrLast,
    FocusColumn(#[proptest(strategy = "1..=5usize")] usize),
    FocusWindowOrMonitorUp(#[proptest(strategy = "1..=2u8")] u8),
    FocusWindowOrMonitorDown(#[proptest(strategy = "1..=2u8")] u8),
    FocusColumnOrMonitorLeft(#[proptest(strategy = "1..=2u8")] u8),
    FocusColumnOrMonitorRight(#[proptest(strategy = "1..=2u8")] u8),
    FocusWindowDown,
    FocusWindowUp,
    FocusWindowDownOrColumnLeft,
    FocusWindowDownOrColumnRight,
    FocusWindowUpOrColumnLeft,
    FocusWindowUpOrColumnRight,
    FocusWindowOrWorkspaceDown,
    FocusWindowOrWorkspaceUp,
    FocusWindow(#[proptest(strategy = "1..=5usize")] usize),
    FocusWindowInColumn(#[proptest(strategy = "1..=5u8")] u8),
    FocusWindowTop,
    FocusWindowBottom,
    FocusWindowDownOrTop,
    FocusWindowUpOrBottom,
    MoveColumnLeft,
    MoveColumnRight,
    MoveColumnToFirst,
    MoveColumnToLast,
    MoveColumnLeftOrToMonitorLeft(#[proptest(strategy = "1..=2u8")] u8),
    MoveColumnRightOrToMonitorRight(#[proptest(strategy = "1..=2u8")] u8),
    MoveColumnToIndex(#[proptest(strategy = "1..=5usize")] usize),
    MoveWindowDown,
    MoveWindowUp,
    MoveWindowDownOrToWorkspaceDown,
    MoveWindowUpOrToWorkspaceUp,
    ConsumeOrExpelWindowLeft {
        #[proptest(strategy = "proptest::option::of(1..=5usize)")]
        id: Option<usize>,
    },
    ConsumeOrExpelWindowRight {
        #[proptest(strategy = "proptest::option::of(1..=5usize)")]
        id: Option<usize>,
    },
    ConsumeWindowIntoColumn,
    ExpelWindowFromColumn,
    SwapWindowInDirection(#[proptest(strategy = "arbitrary_scroll_direction()")] ScrollDirection),
    ToggleColumnTabbedDisplay,
    SetColumnDisplay(#[proptest(strategy = "arbitrary_column_display()")] ColumnDisplay),
    CenterColumn,
    CenterWindow {
        #[proptest(strategy = "proptest::option::of(1..=5usize)")]
        id: Option<usize>,
    },
    CenterVisibleColumns,
    FocusWorkspaceDown,
    FocusWorkspaceUp,
    FocusWorkspace(#[proptest(strategy = "0..=4usize")] usize),
    FocusWorkspaceAutoBackAndForth(#[proptest(strategy = "0..=4usize")] usize),
    FocusWorkspacePrevious,
    MoveWindowToWorkspaceDown(bool),
    MoveWindowToWorkspaceUp(bool),
    MoveWindowToWorkspace {
        #[proptest(strategy = "proptest::option::of(1..=5usize)")]
        window_id: Option<usize>,
        #[proptest(strategy = "0..=4usize")]
        workspace_idx: usize,
    },
    MoveColumnToWorkspaceDown(bool),
    MoveColumnToWorkspaceUp(bool),
    MoveColumnToWorkspace(#[proptest(strategy = "0..=4usize")] usize, bool),
    MoveWorkspaceDown,
    MoveWorkspaceUp,
    MoveWorkspaceToIndex {
        #[proptest(strategy = "proptest::option::of(1..=5usize)")]
        ws_name: Option<usize>,
        #[proptest(strategy = "0..=4usize")]
        target_idx: usize,
    },
    MoveWorkspaceToMonitor {
        #[proptest(strategy = "proptest::option::of(1..=5usize)")]
        ws_name: Option<usize>,
        #[proptest(strategy = "0..=5usize")]
        output_id: usize,
    },
    SetWorkspaceName {
        #[proptest(strategy = "1..=5usize")]
        new_ws_name: usize,
        #[proptest(strategy = "proptest::option::of(1..=5usize)")]
        ws_name: Option<usize>,
    },
    UnsetWorkspaceName {
        #[proptest(strategy = "proptest::option::of(1..=5usize)")]
        ws_name: Option<usize>,
    },
    MoveWindowToOutput {
        #[proptest(strategy = "proptest::option::of(1..=5usize)")]
        window_id: Option<usize>,
        #[proptest(strategy = "1..=5usize")]
        output_id: usize,
        #[proptest(strategy = "proptest::option::of(0..=4usize)")]
        target_ws_idx: Option<usize>,
    },
    MoveColumnToOutput {
        #[proptest(strategy = "1..=5usize")]
        output_id: usize,
        #[proptest(strategy = "proptest::option::of(0..=4usize)")]
        target_ws_idx: Option<usize>,
        activate: bool,
    },
    SwitchPresetColumnWidth,
    SwitchPresetColumnWidthBack,
    SwitchPresetWindowWidth {
        #[proptest(strategy = "proptest::option::of(1..=5usize)")]
        id: Option<usize>,
    },
    SwitchPresetWindowWidthBack {
        #[proptest(strategy = "proptest::option::of(1..=5usize)")]
        id: Option<usize>,
    },
    SwitchPresetWindowHeight {
        #[proptest(strategy = "proptest::option::of(1..=5usize)")]
        id: Option<usize>,
    },
    SwitchPresetWindowHeightBack {
        #[proptest(strategy = "proptest::option::of(1..=5usize)")]
        id: Option<usize>,
    },
    MaximizeColumn,
    MaximizeWindowToEdges {
        #[proptest(strategy = "proptest::option::of(1..=5usize)")]
        id: Option<usize>,
    },
    SetColumnWidth(#[proptest(strategy = "arbitrary_size_change()")] SizeChange),
    SetWindowWidth {
        #[proptest(strategy = "proptest::option::of(1..=5usize)")]
        id: Option<usize>,
        #[proptest(strategy = "arbitrary_size_change()")]
        change: SizeChange,
    },
    SetWindowHeight {
        #[proptest(strategy = "proptest::option::of(1..=5usize)")]
        id: Option<usize>,
        #[proptest(strategy = "arbitrary_size_change()")]
        change: SizeChange,
    },
    ResetWindowHeight {
        #[proptest(strategy = "proptest::option::of(1..=5usize)")]
        id: Option<usize>,
    },
    ExpandColumnToAvailableWidth,
    ToggleWindowFloating {
        #[proptest(strategy = "proptest::option::of(1..=5usize)")]
        id: Option<usize>,
    },
    SetWindowFloating {
        #[proptest(strategy = "proptest::option::of(1..=5usize)")]
        id: Option<usize>,
        floating: bool,
    },
    FocusFloating,
    FocusTiling,
    SwitchFocusFloatingTiling,
    MoveFloatingWindow {
        #[proptest(strategy = "proptest::option::of(1..=5usize)")]
        id: Option<usize>,
        #[proptest(strategy = "arbitrary_position_change()")]
        x: PositionChange,
        #[proptest(strategy = "arbitrary_position_change()")]
        y: PositionChange,
        animate: bool,
    },
    SetParent {
        #[proptest(strategy = "1..=5usize")]
        id: usize,
        #[proptest(strategy = "prop::option::of(1..=5usize)")]
        new_parent_id: Option<usize>,
    },
    SetForcedSize {
        #[proptest(strategy = "1..=5usize")]
        id: usize,
        #[proptest(strategy = "proptest::option::of(arbitrary_size())")]
        size: Option<Size<i32, Logical>>,
    },
    Communicate(#[proptest(strategy = "1..=5usize")] usize),
    Refresh {
        is_active: bool,
    },
    AdvanceAnimations {
        #[proptest(strategy = "arbitrary_msec_delta()")]
        msec_delta: i32,
    },
    CompleteAnimations,
    MoveWorkspaceToOutput(#[proptest(strategy = "1..=5usize")] usize),
    ViewOffsetGestureBegin {
        #[proptest(strategy = "1..=5usize")]
        output_idx: usize,
        #[proptest(strategy = "proptest::option::of(0..=4usize)")]
        workspace_idx: Option<usize>,
        is_touchpad: bool,
    },
    ViewOffsetGestureUpdate {
        #[proptest(strategy = "arbitrary_view_offset_gesture_delta()")]
        delta: f64,
        timestamp: Duration,
        is_touchpad: bool,
    },
    ViewOffsetGestureEnd {
        is_touchpad: Option<bool>,
    },
    WorkspaceSwitchGestureBegin {
        #[proptest(strategy = "1..=5usize")]
        output_idx: usize,
        is_touchpad: bool,
    },
    WorkspaceSwitchGestureUpdate {
        #[proptest(strategy = "-400f64..400f64")]
        delta: f64,
        timestamp: Duration,
        is_touchpad: bool,
    },
    WorkspaceSwitchGestureEnd {
        is_touchpad: Option<bool>,
    },
    OverviewGestureBegin,
    OverviewGestureUpdate {
        #[proptest(strategy = "-400f64..400f64")]
        delta: f64,
        timestamp: Duration,
    },
    OverviewGestureEnd,
    GridGestureBegin,
    GridGestureUpdate {
        #[proptest(strategy = "-400f64..400f64")]
        delta: f64,
        timestamp: Duration,
    },
    GridGestureEnd,
    InteractiveMoveBegin {
        #[proptest(strategy = "1..=5usize")]
        window: usize,
        #[proptest(strategy = "1..=5usize")]
        output_idx: usize,
        #[proptest(strategy = "-20000f64..20000f64")]
        px: f64,
        #[proptest(strategy = "-20000f64..20000f64")]
        py: f64,
    },
    InteractiveMoveUpdate {
        #[proptest(strategy = "1..=5usize")]
        window: usize,
        #[proptest(strategy = "-20000f64..20000f64")]
        dx: f64,
        #[proptest(strategy = "-20000f64..20000f64")]
        dy: f64,
        #[proptest(strategy = "1..=5usize")]
        output_idx: usize,
        #[proptest(strategy = "-20000f64..20000f64")]
        px: f64,
        #[proptest(strategy = "-20000f64..20000f64")]
        py: f64,
    },
    InteractiveMoveEnd {
        #[proptest(strategy = "1..=5usize")]
        window: usize,
    },
    DndUpdate {
        #[proptest(strategy = "1..=5usize")]
        output_idx: usize,
        #[proptest(strategy = "-20000f64..20000f64")]
        px: f64,
        #[proptest(strategy = "-20000f64..20000f64")]
        py: f64,
    },
    DndEnd,
    InteractiveResizeBegin {
        #[proptest(strategy = "1..=5usize")]
        window: usize,
        #[proptest(strategy = "arbitrary_resize_edge()")]
        edges: ResizeEdge,
    },
    InteractiveResizeUpdate {
        #[proptest(strategy = "1..=5usize")]
        window: usize,
        #[proptest(strategy = "-20000f64..20000f64")]
        dx: f64,
        #[proptest(strategy = "-20000f64..20000f64")]
        dy: f64,
    },
    InteractiveResizeEnd {
        #[proptest(strategy = "1..=5usize")]
        window: usize,
    },
    ToggleOverview,
    ToggleGridOverview,
    OpenGridOverview,
    CloseGridOverview,
    ConfirmGridSelection,
    ConfirmGridSelectionForWindow(#[proptest(strategy = "1..=5usize")] usize),
    ActivateWindowFromActivation(#[proptest(strategy = "1..=5usize")] usize),
    SetGridAllMonitors(bool),
    UpdateConfig {
        #[proptest(strategy = "arbitrary_layout_part().prop_map(Box::new)")]
        layout_config: Box<niri_config::LayoutPart>,
    },
}

impl Op {
    fn apply(self, layout: &mut Layout<TestWindow>) {
        match self {
            Op::AddOutput(id) => {
                let name = format!("output{id}");
                if layout.outputs().any(|o| o.name() == name) {
                    return;
                }

                let output = Output::new(
                    name.clone(),
                    PhysicalProperties {
                        size: Size::from((1280, 720)),
                        subpixel: Subpixel::Unknown,
                        make: String::new(),
                        model: String::new(),
                        serial_number: String::new(),
                    },
                );
                output.change_current_state(
                    Some(Mode {
                        size: Size::from((1280, 720)),
                        refresh: 60000,
                    }),
                    None,
                    None,
                    None,
                );
                output.user_data().insert_if_missing(|| OutputName {
                    connector: name,
                    make: None,
                    model: None,
                    serial: None,
                });
                layout.add_output(output.clone(), None);
            }
            Op::AddScaledOutput {
                id,
                scale,
                layout_config,
            } => {
                let name = format!("output{id}");
                if layout.outputs().any(|o| o.name() == name) {
                    return;
                }

                let output = Output::new(
                    name.clone(),
                    PhysicalProperties {
                        size: Size::from((1280, 720)),
                        subpixel: Subpixel::Unknown,
                        make: String::new(),
                        model: String::new(),
                        serial_number: String::new(),
                    },
                );
                output.change_current_state(
                    Some(Mode {
                        size: Size::from((1280, 720)),
                        refresh: 60000,
                    }),
                    None,
                    Some(smithay::output::Scale::Fractional(scale)),
                    None,
                );
                output.user_data().insert_if_missing(|| OutputName {
                    connector: name,
                    make: None,
                    model: None,
                    serial: None,
                });
                layout.add_output(output.clone(), layout_config.map(|x| *x));
            }
            Op::RemoveOutput(id) => {
                let name = format!("output{id}");
                let Some(output) = layout.outputs().find(|o| o.name() == name).cloned() else {
                    return;
                };

                layout.remove_output(&output);
            }
            Op::FocusOutput(id) => {
                let name = format!("output{id}");
                let Some(output) = layout.outputs().find(|o| o.name() == name).cloned() else {
                    return;
                };

                layout.focus_output(&output);
            }
            Op::UpdateOutputLayoutConfig { id, layout_config } => {
                let name = format!("output{id}");
                let Some(mon) = layout.monitors_mut().find(|m| m.output_name() == &name) else {
                    return;
                };

                mon.update_layout_config(layout_config.map(|x| *x));
            }
            Op::AddNamedWorkspace {
                ws_name,
                output_names,
                layout_config,
            } => {
                layout.ensure_named_workspace(&WorkspaceConfig {
                    name: WorkspaceName(format!("ws{ws_name}")),
                    open_on_output: output_names
                        .into_iter()
                        .map(|name| format!("output{name}"))
                        .collect(),
                    layout: layout_config.map(|x| niri_config::WorkspaceLayoutPart(*x)),
                });
            }
            Op::UnnameWorkspace { ws_name } => {
                layout.unname_workspace(&format!("ws{ws_name}"));
            }
            Op::UpdateWorkspaceLayoutConfig {
                ws_name,
                layout_config,
            } => {
                let ws_name = format!("ws{ws_name}");
                let Some(ws) = layout
                    .workspaces_mut()
                    .find(|ws| ws.name() == Some(&ws_name))
                else {
                    return;
                };

                ws.update_layout_config(layout_config.map(|x| *x));
            }
            Op::SetWorkspaceName {
                new_ws_name,
                ws_name,
            } => {
                let ws_ref =
                    ws_name.map(|ws_name| WorkspaceReference::Name(format!("ws{ws_name}")));
                layout.set_workspace_name(format!("ws{new_ws_name}"), ws_ref);
            }
            Op::UnsetWorkspaceName { ws_name } => {
                let ws_ref =
                    ws_name.map(|ws_name| WorkspaceReference::Name(format!("ws{ws_name}")));
                layout.unset_workspace_name(ws_ref);
            }
            Op::AddWindow { mut params } => {
                if layout.has_window(&params.id) {
                    return;
                }
                if let Some(parent_id) = params.parent_id {
                    if parent_id_causes_loop(layout, params.id, parent_id) {
                        params.parent_id = None;
                    }
                }

                let is_floating = params.is_floating;
                let win = TestWindow::new(params);
                layout.add_window(
                    win,
                    AddWindowTarget::Auto,
                    None,
                    None,
                    false,
                    is_floating,
                    ActivateWindow::default(),
                );
            }
            Op::AddWindowNextTo {
                mut params,
                next_to_id,
            } => {
                let mut found_next_to = false;

                if let Some(InteractiveMoveState::Moving(move_)) = &layout.interactive_move {
                    let win_id = move_.tile.window().0.id;
                    if win_id == params.id {
                        return;
                    }
                    if win_id == next_to_id {
                        found_next_to = true;
                    }
                }

                match &mut layout.monitor_set {
                    MonitorSet::Normal { monitors, .. } => {
                        for mon in monitors {
                            for ws in &mut mon.workspaces {
                                for win in ws.windows() {
                                    if win.0.id == params.id {
                                        return;
                                    }

                                    if win.0.id == next_to_id {
                                        found_next_to = true;
                                    }
                                }
                            }
                        }
                    }
                    MonitorSet::NoOutputs { workspaces, .. } => {
                        for ws in workspaces {
                            for win in ws.windows() {
                                if win.0.id == params.id {
                                    return;
                                }

                                if win.0.id == next_to_id {
                                    found_next_to = true;
                                }
                            }
                        }
                    }
                }

                if !found_next_to {
                    return;
                }

                if let Some(parent_id) = params.parent_id {
                    if parent_id_causes_loop(layout, params.id, parent_id) {
                        params.parent_id = None;
                    }
                }

                let is_floating = params.is_floating;
                let win = TestWindow::new(params);
                layout.add_window(
                    win,
                    AddWindowTarget::NextTo(&next_to_id),
                    None,
                    None,
                    false,
                    is_floating,
                    ActivateWindow::default(),
                );
            }
            Op::AddWindowToNamedWorkspace {
                mut params,
                ws_name,
            } => {
                let ws_name = format!("ws{ws_name}");
                let mut ws_id = None;

                if let Some(InteractiveMoveState::Moving(move_)) = &layout.interactive_move {
                    if move_.tile.window().0.id == params.id {
                        return;
                    }
                }

                match &mut layout.monitor_set {
                    MonitorSet::Normal { monitors, .. } => {
                        for mon in monitors {
                            for ws in &mut mon.workspaces {
                                for win in ws.windows() {
                                    if win.0.id == params.id {
                                        return;
                                    }
                                }

                                if ws
                                    .name
                                    .as_ref()
                                    .is_some_and(|name| name.eq_ignore_ascii_case(&ws_name))
                                {
                                    ws_id = Some(ws.id());
                                }
                            }
                        }
                    }
                    MonitorSet::NoOutputs { workspaces, .. } => {
                        for ws in workspaces {
                            for win in ws.windows() {
                                if win.0.id == params.id {
                                    return;
                                }
                            }

                            if ws
                                .name
                                .as_ref()
                                .is_some_and(|name| name.eq_ignore_ascii_case(&ws_name))
                            {
                                ws_id = Some(ws.id());
                            }
                        }
                    }
                }

                let Some(ws_id) = ws_id else {
                    return;
                };

                if let Some(parent_id) = params.parent_id {
                    if parent_id_causes_loop(layout, params.id, parent_id) {
                        params.parent_id = None;
                    }
                }

                let is_floating = params.is_floating;
                let win = TestWindow::new(params);
                layout.add_window(
                    win,
                    AddWindowTarget::Workspace(ws_id),
                    None,
                    None,
                    false,
                    is_floating,
                    ActivateWindow::default(),
                );
            }
            Op::CloseWindow(id) => {
                layout.remove_window(&id, Transaction::new());
            }
            Op::FullscreenWindow(id) => {
                if !layout.has_window(&id) {
                    return;
                }
                layout.toggle_fullscreen(&id);
            }
            Op::SetFullscreenWindow {
                window,
                is_fullscreen,
            } => {
                if !layout.has_window(&window) {
                    return;
                }
                layout.set_fullscreen(&window, is_fullscreen);
            }
            Op::MinimizeWindow(id) => {
                if !layout.has_window(&id) {
                    return;
                }
                layout.toggle_window_minimized(&id);
            }
            Op::UnminimizeWindow(id) => {
                if !layout.is_window_minimized(&id) {
                    return;
                }
                layout.unminimize_window(&id, true);
            }
            Op::SetWindowMinimized { window, minimize } => {
                if !layout.has_window(&window) {
                    return;
                }
                layout.set_window_minimized(&window, minimize);
            }
            Op::ToggleWindowedFullscreen(id) => {
                if !layout.has_window(&id) {
                    return;
                }
                layout.toggle_windowed_fullscreen(&id);
            }
            Op::FocusColumnLeft => layout.focus_left(),
            Op::FocusColumnRight => layout.focus_right(),
            Op::FocusColumnFirst => layout.focus_column_first(),
            Op::FocusColumnLast => layout.focus_column_last(),
            Op::FocusColumnRightOrFirst => layout.focus_column_right_or_first(),
            Op::FocusColumnLeftOrLast => layout.focus_column_left_or_last(),
            Op::FocusColumn(index) => layout.focus_column(index),
            Op::FocusWindowOrMonitorUp(id) => {
                let name = format!("output{id}");
                let Some(output) = layout.outputs().find(|o| o.name() == name).cloned() else {
                    return;
                };

                layout.focus_window_up_or_output(&output);
            }
            Op::FocusWindowOrMonitorDown(id) => {
                let name = format!("output{id}");
                let Some(output) = layout.outputs().find(|o| o.name() == name).cloned() else {
                    return;
                };

                layout.focus_window_down_or_output(&output);
            }
            Op::FocusColumnOrMonitorLeft(id) => {
                let name = format!("output{id}");
                let Some(output) = layout.outputs().find(|o| o.name() == name).cloned() else {
                    return;
                };

                layout.focus_column_left_or_output(&output);
            }
            Op::FocusColumnOrMonitorRight(id) => {
                let name = format!("output{id}");
                let Some(output) = layout.outputs().find(|o| o.name() == name).cloned() else {
                    return;
                };

                layout.focus_column_right_or_output(&output);
            }
            Op::FocusWindowDown => layout.focus_down(),
            Op::FocusWindowUp => layout.focus_up(),
            Op::FocusWindowDownOrColumnLeft => layout.focus_down_or_left(),
            Op::FocusWindowDownOrColumnRight => layout.focus_down_or_right(),
            Op::FocusWindowUpOrColumnLeft => layout.focus_up_or_left(),
            Op::FocusWindowUpOrColumnRight => layout.focus_up_or_right(),
            Op::FocusWindowOrWorkspaceDown => layout.focus_window_or_workspace_down(),
            Op::FocusWindowOrWorkspaceUp => layout.focus_window_or_workspace_up(),
            Op::FocusWindow(id) => layout.activate_window(&id),
            Op::FocusWindowInColumn(index) => layout.focus_window_in_column(index),
            Op::FocusWindowTop => layout.focus_window_top(),
            Op::FocusWindowBottom => layout.focus_window_bottom(),
            Op::FocusWindowDownOrTop => layout.focus_window_down_or_top(),
            Op::FocusWindowUpOrBottom => layout.focus_window_up_or_bottom(),
            Op::MoveColumnLeft => layout.move_left(),
            Op::MoveColumnRight => layout.move_right(),
            Op::MoveColumnToFirst => layout.move_column_to_first(),
            Op::MoveColumnToLast => layout.move_column_to_last(),
            Op::MoveColumnLeftOrToMonitorLeft(id) => {
                let name = format!("output{id}");
                let Some(output) = layout.outputs().find(|o| o.name() == name).cloned() else {
                    return;
                };

                layout.move_column_left_or_to_output(&output);
            }
            Op::MoveColumnRightOrToMonitorRight(id) => {
                let name = format!("output{id}");
                let Some(output) = layout.outputs().find(|o| o.name() == name).cloned() else {
                    return;
                };

                layout.move_column_right_or_to_output(&output);
            }
            Op::MoveColumnToIndex(index) => layout.move_column_to_index(index),
            Op::MoveWindowDown => layout.move_down(),
            Op::MoveWindowUp => layout.move_up(),
            Op::MoveWindowDownOrToWorkspaceDown => layout.move_down_or_to_workspace_down(),
            Op::MoveWindowUpOrToWorkspaceUp => layout.move_up_or_to_workspace_up(),
            Op::ConsumeOrExpelWindowLeft { id } => {
                let id = id.filter(|id| layout.has_window(id));
                layout.consume_or_expel_window_left(id.as_ref());
            }
            Op::ConsumeOrExpelWindowRight { id } => {
                let id = id.filter(|id| layout.has_window(id));
                layout.consume_or_expel_window_right(id.as_ref());
            }
            Op::ConsumeWindowIntoColumn => layout.consume_into_column(),
            Op::ExpelWindowFromColumn => layout.expel_from_column(),
            Op::SwapWindowInDirection(direction) => layout.swap_window_in_direction(direction),
            Op::ToggleColumnTabbedDisplay => layout.toggle_column_tabbed_display(),
            Op::SetColumnDisplay(display) => layout.set_column_display(display),
            Op::CenterColumn => layout.center_column(),
            Op::CenterWindow { id } => {
                let id = id.filter(|id| layout.has_window(id));
                layout.center_window(id.as_ref());
            }
            Op::CenterVisibleColumns => layout.center_visible_columns(),
            Op::FocusWorkspaceDown => layout.switch_workspace_down(),
            Op::FocusWorkspaceUp => layout.switch_workspace_up(),
            Op::FocusWorkspace(idx) => layout.switch_workspace(idx),
            Op::FocusWorkspaceAutoBackAndForth(idx) => {
                layout.switch_workspace_auto_back_and_forth(idx)
            }
            Op::FocusWorkspacePrevious => layout.switch_workspace_previous(),
            Op::MoveWindowToWorkspaceDown(focus) => layout.move_to_workspace_down(focus),
            Op::MoveWindowToWorkspaceUp(focus) => layout.move_to_workspace_up(focus),
            Op::MoveWindowToWorkspace {
                window_id,
                workspace_idx,
            } => {
                let window_id = window_id.filter(|id| layout.has_window(id));
                layout.move_to_workspace(window_id.as_ref(), workspace_idx, ActivateWindow::Smart);
            }
            Op::MoveColumnToWorkspaceDown(focus) => layout.move_column_to_workspace_down(focus),
            Op::MoveColumnToWorkspaceUp(focus) => layout.move_column_to_workspace_up(focus),
            Op::MoveColumnToWorkspace(idx, focus) => layout.move_column_to_workspace(idx, focus),
            Op::MoveWindowToOutput {
                window_id,
                output_id: id,
                target_ws_idx,
            } => {
                let name = format!("output{id}");
                let Some(output) = layout.outputs().find(|o| o.name() == name).cloned() else {
                    return;
                };
                let mon = layout.monitor_for_output(&output).unwrap();

                let window_id = window_id.filter(|id| layout.has_window(id));
                let target_ws_idx = target_ws_idx.filter(|idx| mon.workspaces.len() > *idx);
                layout.move_to_output(
                    window_id.as_ref(),
                    &output,
                    target_ws_idx,
                    ActivateWindow::Smart,
                );
            }
            Op::MoveColumnToOutput {
                output_id: id,
                target_ws_idx,
                activate,
            } => {
                let name = format!("output{id}");
                let Some(output) = layout.outputs().find(|o| o.name() == name).cloned() else {
                    return;
                };

                layout.move_column_to_output(&output, target_ws_idx, activate);
            }
            Op::MoveWorkspaceDown => layout.move_workspace_down(),
            Op::MoveWorkspaceUp => layout.move_workspace_up(),
            Op::MoveWorkspaceToIndex {
                ws_name: Some(ws_name),
                target_idx,
            } => {
                let MonitorSet::Normal { monitors, .. } = &mut layout.monitor_set else {
                    return;
                };

                let Some((old_idx, old_output)) = monitors.iter().find_map(|monitor| {
                    monitor
                        .workspaces
                        .iter()
                        .enumerate()
                        .find_map(|(i, ws)| {
                            if ws.name == Some(format!("ws{ws_name}")) {
                                Some(i)
                            } else {
                                None
                            }
                        })
                        .map(|i| (i, monitor.output.clone()))
                }) else {
                    return;
                };

                layout.move_workspace_to_idx(Some((Some(old_output), old_idx)), target_idx)
            }
            Op::MoveWorkspaceToIndex {
                ws_name: None,
                target_idx,
            } => layout.move_workspace_to_idx(None, target_idx),
            Op::MoveWorkspaceToMonitor {
                ws_name: None,
                output_id: id,
            } => {
                let name = format!("output{id}");
                let Some(output) = layout.outputs().find(|o| o.name() == name).cloned() else {
                    return;
                };
                layout.move_workspace_to_output(&output);
            }
            Op::MoveWorkspaceToMonitor {
                ws_name: Some(ws_name),
                output_id: id,
            } => {
                let name = format!("output{id}");
                let Some(output) = layout.outputs().find(|o| o.name() == name).cloned() else {
                    return;
                };
                let MonitorSet::Normal { monitors, .. } = &mut layout.monitor_set else {
                    return;
                };

                let Some((old_idx, old_output)) = monitors.iter().find_map(|monitor| {
                    monitor
                        .workspaces
                        .iter()
                        .enumerate()
                        .find_map(|(i, ws)| {
                            if ws.name == Some(format!("ws{ws_name}")) {
                                Some(i)
                            } else {
                                None
                            }
                        })
                        .map(|i| (i, monitor.output.clone()))
                }) else {
                    return;
                };

                layout.move_workspace_to_output_by_id(old_idx, Some(old_output), &output);
            }
            Op::SwitchPresetColumnWidth => layout.toggle_width(true),
            Op::SwitchPresetColumnWidthBack => layout.toggle_width(false),
            Op::SwitchPresetWindowWidth { id } => {
                let id = id.filter(|id| layout.has_window(id));
                layout.toggle_window_width(id.as_ref(), true);
            }
            Op::SwitchPresetWindowWidthBack { id } => {
                let id = id.filter(|id| layout.has_window(id));
                layout.toggle_window_width(id.as_ref(), false);
            }
            Op::SwitchPresetWindowHeight { id } => {
                let id = id.filter(|id| layout.has_window(id));
                layout.toggle_window_height(id.as_ref(), true);
            }
            Op::SwitchPresetWindowHeightBack { id } => {
                let id = id.filter(|id| layout.has_window(id));
                layout.toggle_window_height(id.as_ref(), false);
            }
            Op::MaximizeColumn => layout.toggle_full_width(),
            Op::MaximizeWindowToEdges { id } => {
                let id = id.or_else(|| layout.focus().map(|win| *win.id()));
                let Some(id) = id else {
                    return;
                };
                if !layout.has_window(&id) {
                    return;
                }
                layout.toggle_maximized(&id);
            }
            Op::SetColumnWidth(change) => layout.set_column_width(change),
            Op::SetWindowWidth { id, change } => {
                let id = id.filter(|id| layout.has_window(id));
                layout.set_window_width(id.as_ref(), change);
            }
            Op::SetWindowHeight { id, change } => {
                let id = id.filter(|id| layout.has_window(id));
                layout.set_window_height(id.as_ref(), change);
            }
            Op::ResetWindowHeight { id } => {
                let id = id.filter(|id| layout.has_window(id));
                layout.reset_window_height(id.as_ref());
            }
            Op::ExpandColumnToAvailableWidth => layout.expand_column_to_available_width(),
            Op::ToggleWindowFloating { id } => {
                let id = id.filter(|id| layout.has_window(id));
                layout.toggle_window_floating(id.as_ref());
            }
            Op::SetWindowFloating { id, floating } => {
                let id = id.filter(|id| layout.has_window(id));
                layout.set_window_floating(id.as_ref(), floating);
            }
            Op::FocusFloating => {
                layout.focus_floating();
            }
            Op::FocusTiling => {
                layout.focus_tiling();
            }
            Op::SwitchFocusFloatingTiling => {
                layout.switch_focus_floating_tiling();
            }
            Op::MoveFloatingWindow { id, x, y, animate } => {
                let id = id.filter(|id| layout.has_window(id));
                layout.move_floating_window(id.as_ref(), x, y, animate);
            }
            Op::SetParent {
                id,
                mut new_parent_id,
            } => {
                if !layout.has_window(&id) {
                    return;
                }

                if let Some(parent_id) = new_parent_id {
                    if parent_id_causes_loop(layout, id, parent_id) {
                        new_parent_id = None;
                    }
                }

                let mut update = false;

                if let Some(InteractiveMoveState::Moving(move_)) = &layout.interactive_move {
                    if move_.tile.window().0.id == id {
                        move_.tile.window().0.parent_id.set(new_parent_id);
                        update = true;
                    }
                }

                match &mut layout.monitor_set {
                    MonitorSet::Normal { monitors, .. } => {
                        'outer: for mon in monitors {
                            for ws in &mut mon.workspaces {
                                for win in ws.windows() {
                                    if win.0.id == id {
                                        win.0.parent_id.set(new_parent_id);
                                        update = true;
                                        break 'outer;
                                    }
                                }
                            }
                        }
                    }
                    MonitorSet::NoOutputs { workspaces, .. } => {
                        'outer: for ws in workspaces {
                            for win in ws.windows() {
                                if win.0.id == id {
                                    win.0.parent_id.set(new_parent_id);
                                    update = true;
                                    break 'outer;
                                }
                            }
                        }
                    }
                }

                if update {
                    if let Some(new_parent_id) = new_parent_id {
                        layout.descendants_added(&new_parent_id);
                    }
                }
            }
            Op::SetForcedSize { id, size } => {
                for (_mon, win) in layout.windows() {
                    if win.0.id == id {
                        win.0.forced_size.set(size);
                        return;
                    }
                }
            }
            Op::Communicate(id) => {
                let mut update = false;

                if let Some(InteractiveMoveState::Moving(move_)) = &layout.interactive_move {
                    if move_.tile.window().0.id == id {
                        if move_.tile.window().communicate() {
                            update = true;
                        }

                        if update {
                            // FIXME: serial.
                            layout.update_window(&id, None);
                        }
                        return;
                    }
                }

                match &mut layout.monitor_set {
                    MonitorSet::Normal { monitors, .. } => {
                        'outer: for mon in monitors {
                            for ws in &mut mon.workspaces {
                                for win in ws.windows() {
                                    if win.0.id == id {
                                        if win.communicate() {
                                            update = true;
                                        }
                                        break 'outer;
                                    }
                                }
                            }
                        }
                    }
                    MonitorSet::NoOutputs { workspaces, .. } => {
                        'outer: for ws in workspaces {
                            for win in ws.windows() {
                                if win.0.id == id {
                                    if win.communicate() {
                                        update = true;
                                    }
                                    break 'outer;
                                }
                            }
                        }
                    }
                }

                if update {
                    // FIXME: serial.
                    layout.update_window(&id, None);
                }
            }
            Op::Refresh { is_active } => {
                layout.refresh(is_active);
            }
            Op::AdvanceAnimations { msec_delta } => {
                let mut now = layout.clock.now_unadjusted();
                if msec_delta >= 0 {
                    now = now.saturating_add(Duration::from_millis(msec_delta as u64));
                } else {
                    now = now.saturating_sub(Duration::from_millis(-msec_delta as u64));
                }
                layout.clock.set_unadjusted(now);
                layout.advance_animations();
            }
            Op::CompleteAnimations => {
                layout.clock.set_complete_instantly(true);
                layout.advance_animations();
                layout.clock.set_complete_instantly(false);
            }
            Op::MoveWorkspaceToOutput(id) => {
                let name = format!("output{id}");
                let Some(output) = layout.outputs().find(|o| o.name() == name).cloned() else {
                    return;
                };

                layout.move_workspace_to_output(&output);
            }
            Op::ViewOffsetGestureBegin {
                output_idx: id,
                workspace_idx,
                is_touchpad: normalize,
            } => {
                let name = format!("output{id}");
                let Some(output) = layout.outputs().find(|o| o.name() == name).cloned() else {
                    return;
                };

                layout.view_offset_gesture_begin(&output, workspace_idx, normalize);
            }
            Op::ViewOffsetGestureUpdate {
                delta,
                timestamp,
                is_touchpad,
            } => {
                layout.view_offset_gesture_update(delta, timestamp, is_touchpad);
            }
            Op::ViewOffsetGestureEnd { is_touchpad } => {
                layout.view_offset_gesture_end(is_touchpad);
            }
            Op::WorkspaceSwitchGestureBegin {
                output_idx: id,
                is_touchpad,
            } => {
                let name = format!("output{id}");
                let Some(output) = layout.outputs().find(|o| o.name() == name).cloned() else {
                    return;
                };

                layout.workspace_switch_gesture_begin(&output, is_touchpad);
            }
            Op::WorkspaceSwitchGestureUpdate {
                delta,
                timestamp,
                is_touchpad,
            } => {
                layout.workspace_switch_gesture_update(delta, timestamp, is_touchpad);
            }
            Op::WorkspaceSwitchGestureEnd { is_touchpad } => {
                layout.workspace_switch_gesture_end(is_touchpad);
            }
            Op::OverviewGestureBegin => {
                layout.overview_gesture_begin();
            }
            Op::OverviewGestureUpdate { delta, timestamp } => {
                layout.overview_gesture_update(delta, timestamp);
            }
            Op::OverviewGestureEnd => {
                layout.overview_gesture_end();
            }
            Op::GridGestureBegin => {
                layout.grid_gesture_begin();
            }
            Op::GridGestureUpdate { delta, timestamp } => {
                layout.grid_gesture_update(delta, timestamp);
            }
            Op::GridGestureEnd => {
                layout.grid_gesture_end();
            }
            Op::InteractiveMoveBegin {
                window,
                output_idx,
                px,
                py,
            } => {
                let name = format!("output{output_idx}");
                let Some(output) = layout.outputs().find(|o| o.name() == name).cloned() else {
                    return;
                };
                layout.interactive_move_begin(window, &output, Point::from((px, py)));
            }
            Op::InteractiveMoveUpdate {
                window,
                dx,
                dy,
                output_idx,
                px,
                py,
            } => {
                let name = format!("output{output_idx}");
                let Some(output) = layout.outputs().find(|o| o.name() == name).cloned() else {
                    return;
                };
                layout.interactive_move_update(
                    &window,
                    Point::from((dx, dy)),
                    output,
                    Point::from((px, py)),
                );
            }
            Op::InteractiveMoveEnd { window } => {
                layout.interactive_move_end(&window);
            }
            Op::DndUpdate { output_idx, px, py } => {
                let name = format!("output{output_idx}");
                let Some(output) = layout.outputs().find(|o| o.name() == name).cloned() else {
                    return;
                };
                layout.dnd_update(output, Point::from((px, py)));
            }
            Op::DndEnd => {
                layout.dnd_end();
            }
            Op::InteractiveResizeBegin { window, edges } => {
                layout.interactive_resize_begin(window, edges);
            }
            Op::InteractiveResizeUpdate { window, dx, dy } => {
                layout.interactive_resize_update(&window, Point::from((dx, dy)));
            }
            Op::InteractiveResizeEnd { window } => {
                layout.interactive_resize_end(&window);
            }
            Op::ToggleOverview => {
                layout.toggle_overview();
            }
            Op::ToggleGridOverview => {
                layout.toggle_grid_overview();
            }
            Op::OpenGridOverview => {
                layout.open_grid_overview();
            }
            Op::CloseGridOverview => {
                layout.close_grid_overview();
            }
            Op::ConfirmGridSelection => {
                layout.confirm_grid_selection();
            }
            Op::ConfirmGridSelectionForWindow(id) => {
                layout.confirm_grid_selection_for_window(&id);
            }
            Op::ActivateWindowFromActivation(id) => {
                if layout.has_window(&id) {
                    layout.activate_window_from_activation(&id);
                }
            }
            Op::SetGridAllMonitors(all) => {
                let mut options = Options::clone(&layout.options);
                options.grid_overview.grid_all_monitors = all;
                layout.update_options(options);
            }
            Op::UpdateConfig { layout_config } => {
                let options = Options {
                    layout: niri_config::Layout::from_part(&layout_config),
                    ..Default::default()
                };

                layout.update_options(options);
            }
        }
    }
}

#[track_caller]
fn check_ops_on_layout(layout: &mut Layout<TestWindow>, ops: impl IntoIterator<Item = Op>) {
    for op in ops {
        if std::env::var_os("NIRI_TEST_TRACE_OPS").is_some() {
            eprintln!("OP {op:?}");
        }
        op.apply(layout);
        layout.verify_invariants();
    }
}

#[track_caller]
fn check_ops(ops: impl IntoIterator<Item = Op>) -> Layout<TestWindow> {
    let mut layout = Layout::default();
    check_ops_on_layout(&mut layout, ops);
    layout
}

fn workspace_output_name(layout: &Layout<TestWindow>, ws_name: usize) -> &str {
    layout
        .workspaces()
        .find(|(_, _, workspace)| workspace.name() == Some(&format!("ws{ws_name}")))
        .and_then(|(monitor, _, _)| monitor)
        .unwrap()
        .output_name()
}

#[test]
fn workspace_output_fallback_promotes_preferred_in_reverse_connection_order() {
    // output3 is not a candidate, so it stays the primary monitor throughout; every assertion
    // below can only hold because of the candidate list.
    let mut layout = Layout::default();
    check_ops_on_layout(
        &mut layout,
        [
            Op::AddOutput(3),
            Op::AddNamedWorkspace {
                ws_name: 1,
                output_names: vec![1, 2],
                layout_config: None,
            },
        ],
    );
    assert_eq!(workspace_output_name(&layout, 1), "output3");

    // The second candidate connects first; the workspace must leave the primary monitor.
    check_ops_on_layout(&mut layout, [Op::AddOutput(2)]);
    assert_eq!(workspace_output_name(&layout, 1), "output2");

    // The first candidate connects last; it still wins.
    check_ops_on_layout(&mut layout, [Op::AddOutput(1)]);
    assert_eq!(workspace_output_name(&layout, 1), "output1");
}

#[test]
fn workspace_output_preferred_removal_uses_fallback() {
    let mut layout = Layout::default();
    check_ops_on_layout(
        &mut layout,
        [
            Op::AddOutput(3),
            Op::AddNamedWorkspace {
                ws_name: 1,
                output_names: vec![1, 2],
                layout_config: None,
            },
            Op::AddOutput(1),
            Op::AddOutput(2),
        ],
    );
    assert_eq!(workspace_output_name(&layout, 1), "output1");

    // The preferred candidate disconnects: the next candidate wins over the primary monitor.
    check_ops_on_layout(&mut layout, [Op::RemoveOutput(1)]);
    assert_eq!(workspace_output_name(&layout, 1), "output2");

    // No candidate left: fall back to the primary monitor.
    check_ops_on_layout(&mut layout, [Op::RemoveOutput(2)]);
    assert_eq!(workspace_output_name(&layout, 1), "output3");
}

#[test]
fn workspace_output_without_match_stays_on_default_output() {
    let mut layout = Layout::default();
    check_ops_on_layout(
        &mut layout,
        [
            Op::AddOutput(3),
            Op::AddNamedWorkspace {
                ws_name: 1,
                output_names: vec![1, 2],
                layout_config: None,
            },
        ],
    );
    assert_eq!(workspace_output_name(&layout, 1), "output3");

    // An output that is not in the candidate list must not take the workspace.
    check_ops_on_layout(&mut layout, [Op::AddOutput(4)]);
    assert_eq!(workspace_output_name(&layout, 1), "output3");
}

#[test]
fn explicit_workspace_move_overrides_output_candidates() {
    let mut layout = Layout::default();
    check_ops_on_layout(
        &mut layout,
        [
            Op::AddOutput(3),
            Op::AddNamedWorkspace {
                ws_name: 1,
                output_names: vec![1, 2],
                layout_config: None,
            },
            Op::AddOutput(2),
        ],
    );
    assert_eq!(workspace_output_name(&layout, 1), "output2");

    // After an explicit move, the candidate list no longer applies, even when the preferred
    // candidate connects afterwards.
    check_ops_on_layout(
        &mut layout,
        [
            Op::MoveWorkspaceToMonitor {
                ws_name: Some(1),
                output_id: 3,
            },
            Op::AddOutput(1),
        ],
    );
    assert_eq!(workspace_output_name(&layout, 1), "output3");
}

#[track_caller]
fn check_ops_with_options(
    options: Options,
    ops: impl IntoIterator<Item = Op>,
) -> Layout<TestWindow> {
    let mut layout = Layout::with_options(Clock::with_time(Duration::ZERO), options);
    check_ops_on_layout(&mut layout, ops);
    layout
}

#[test]
fn operations_dont_panic() {
    if std::env::var_os("RUN_SLOW_TESTS").is_none() {
        eprintln!("ignoring slow test");
        return;
    }

    let every_op = [
        Op::AddOutput(0),
        Op::AddOutput(1),
        Op::AddOutput(2),
        Op::RemoveOutput(0),
        Op::RemoveOutput(1),
        Op::RemoveOutput(2),
        Op::FocusOutput(0),
        Op::FocusOutput(1),
        Op::FocusOutput(2),
        Op::AddNamedWorkspace {
            ws_name: 1,
            output_names: vec![1],
            layout_config: None,
        },
        Op::UnnameWorkspace { ws_name: 1 },
        Op::AddWindow {
            params: TestWindowParams::new(0),
        },
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindowNextTo {
            params: TestWindowParams::new(2),
            next_to_id: 1,
        },
        Op::AddWindowToNamedWorkspace {
            params: TestWindowParams::new(3),
            ws_name: 1,
        },
        Op::CloseWindow(0),
        Op::CloseWindow(1),
        Op::CloseWindow(2),
        Op::FullscreenWindow(1),
        Op::FullscreenWindow(2),
        Op::FullscreenWindow(3),
        Op::MaximizeWindowToEdges { id: Some(1) },
        Op::MaximizeWindowToEdges { id: Some(2) },
        Op::MaximizeWindowToEdges { id: Some(3) },
        Op::MinimizeWindow(1),
        Op::UnminimizeWindow(1),
        Op::FocusColumnLeft,
        Op::FocusColumnRight,
        Op::FocusColumnRightOrFirst,
        Op::FocusColumnLeftOrLast,
        Op::FocusWindowOrMonitorUp(0),
        Op::FocusWindowOrMonitorDown(1),
        Op::FocusColumnOrMonitorLeft(0),
        Op::FocusColumnOrMonitorRight(1),
        Op::FocusWindowUp,
        Op::FocusWindowUpOrColumnLeft,
        Op::FocusWindowUpOrColumnRight,
        Op::FocusWindowOrWorkspaceUp,
        Op::FocusWindowDown,
        Op::FocusWindowDownOrColumnLeft,
        Op::FocusWindowDownOrColumnRight,
        Op::FocusWindowOrWorkspaceDown,
        Op::MoveColumnLeft,
        Op::MoveColumnRight,
        Op::MoveColumnLeftOrToMonitorLeft(0),
        Op::MoveColumnRightOrToMonitorRight(1),
        Op::ConsumeWindowIntoColumn,
        Op::ExpelWindowFromColumn,
        Op::CenterColumn,
        Op::FocusWorkspaceDown,
        Op::FocusWorkspaceUp,
        Op::FocusWorkspace(1),
        Op::FocusWorkspace(2),
        Op::MoveWindowToWorkspaceDown(true),
        Op::MoveWindowToWorkspaceUp(true),
        Op::MoveWindowToWorkspace {
            window_id: None,
            workspace_idx: 1,
        },
        Op::MoveWindowToWorkspace {
            window_id: None,
            workspace_idx: 2,
        },
        Op::MoveColumnToWorkspaceDown(true),
        Op::MoveColumnToWorkspaceUp(true),
        Op::MoveColumnToWorkspace(1, true),
        Op::MoveColumnToWorkspace(2, true),
        Op::MoveWindowDown,
        Op::MoveWindowDownOrToWorkspaceDown,
        Op::MoveWindowUp,
        Op::MoveWindowUpOrToWorkspaceUp,
        Op::ConsumeOrExpelWindowLeft { id: None },
        Op::ConsumeOrExpelWindowRight { id: None },
        Op::MoveWorkspaceToOutput(1),
        Op::ToggleColumnTabbedDisplay,
    ];

    for third in &every_op {
        for second in &every_op {
            for first in &every_op {
                // eprintln!("{first:?}, {second:?}, {third:?}");

                let mut layout = Layout::default();
                first.clone().apply(&mut layout);
                layout.verify_invariants();
                second.clone().apply(&mut layout);
                layout.verify_invariants();
                third.clone().apply(&mut layout);
                layout.verify_invariants();
            }
        }
    }
}

#[test]
fn operations_from_starting_state_dont_panic() {
    if std::env::var_os("RUN_SLOW_TESTS").is_none() {
        eprintln!("ignoring slow test");
        return;
    }

    // Running every op from an empty state doesn't get us to all the interesting states. So,
    // also run it from a manually-created starting state with more things going on to exercise
    // more code paths.
    let setup_ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::MoveWindowToWorkspaceDown(true),
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::FocusColumnLeft,
        Op::ConsumeWindowIntoColumn,
        Op::AddWindow {
            params: TestWindowParams::new(4),
        },
        Op::AddOutput(2),
        Op::AddWindow {
            params: TestWindowParams::new(5),
        },
        Op::MoveWindowToOutput {
            window_id: None,
            output_id: 2,
            target_ws_idx: None,
        },
        Op::FocusOutput(1),
        Op::Communicate(1),
        Op::Communicate(2),
        Op::Communicate(3),
        Op::Communicate(4),
        Op::Communicate(5),
    ];

    let every_op = [
        Op::AddOutput(0),
        Op::AddOutput(1),
        Op::AddOutput(2),
        Op::RemoveOutput(0),
        Op::RemoveOutput(1),
        Op::RemoveOutput(2),
        Op::FocusOutput(0),
        Op::FocusOutput(1),
        Op::FocusOutput(2),
        Op::AddNamedWorkspace {
            ws_name: 1,
            output_names: vec![1],
            layout_config: None,
        },
        Op::UnnameWorkspace { ws_name: 1 },
        Op::AddWindow {
            params: TestWindowParams::new(0),
        },
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindowNextTo {
            params: TestWindowParams::new(6),
            next_to_id: 0,
        },
        Op::AddWindowNextTo {
            params: TestWindowParams::new(7),
            next_to_id: 1,
        },
        Op::AddWindowToNamedWorkspace {
            params: TestWindowParams::new(5),
            ws_name: 1,
        },
        Op::CloseWindow(0),
        Op::CloseWindow(1),
        Op::CloseWindow(2),
        Op::FullscreenWindow(1),
        Op::FullscreenWindow(2),
        Op::FullscreenWindow(3),
        Op::MaximizeWindowToEdges { id: Some(1) },
        Op::MaximizeWindowToEdges { id: Some(2) },
        Op::MaximizeWindowToEdges { id: Some(3) },
        Op::SetFullscreenWindow {
            window: 1,
            is_fullscreen: false,
        },
        Op::SetFullscreenWindow {
            window: 1,
            is_fullscreen: true,
        },
        Op::SetFullscreenWindow {
            window: 2,
            is_fullscreen: false,
        },
        Op::SetFullscreenWindow {
            window: 2,
            is_fullscreen: true,
        },
        Op::MinimizeWindow(1),
        Op::UnminimizeWindow(1),
        Op::SetWindowMinimized {
            window: 2,
            minimize: true,
        },
        Op::FocusColumnLeft,
        Op::FocusColumnRight,
        Op::FocusColumnRightOrFirst,
        Op::FocusColumnLeftOrLast,
        Op::FocusWindowOrMonitorUp(0),
        Op::FocusWindowOrMonitorDown(1),
        Op::FocusColumnOrMonitorLeft(0),
        Op::FocusColumnOrMonitorRight(1),
        Op::FocusWindowUp,
        Op::FocusWindowUpOrColumnLeft,
        Op::FocusWindowUpOrColumnRight,
        Op::FocusWindowOrWorkspaceUp,
        Op::FocusWindowDown,
        Op::FocusWindowDownOrColumnLeft,
        Op::FocusWindowDownOrColumnRight,
        Op::FocusWindowOrWorkspaceDown,
        Op::MoveColumnLeft,
        Op::MoveColumnRight,
        Op::MoveColumnLeftOrToMonitorLeft(0),
        Op::MoveColumnRightOrToMonitorRight(1),
        Op::ConsumeWindowIntoColumn,
        Op::ExpelWindowFromColumn,
        Op::CenterColumn,
        Op::FocusWorkspaceDown,
        Op::FocusWorkspaceUp,
        Op::FocusWorkspace(1),
        Op::FocusWorkspace(2),
        Op::FocusWorkspace(3),
        Op::MoveWindowToWorkspaceDown(true),
        Op::MoveWindowToWorkspaceUp(true),
        Op::MoveWindowToWorkspace {
            window_id: None,
            workspace_idx: 1,
        },
        Op::MoveWindowToWorkspace {
            window_id: None,
            workspace_idx: 2,
        },
        Op::MoveWindowToWorkspace {
            window_id: None,
            workspace_idx: 3,
        },
        Op::MoveColumnToWorkspaceDown(true),
        Op::MoveColumnToWorkspaceUp(true),
        Op::MoveColumnToWorkspace(1, true),
        Op::MoveColumnToWorkspace(2, true),
        Op::MoveColumnToWorkspace(3, true),
        Op::MoveWindowDown,
        Op::MoveWindowDownOrToWorkspaceDown,
        Op::MoveWindowUp,
        Op::MoveWindowUpOrToWorkspaceUp,
        Op::ConsumeOrExpelWindowLeft { id: None },
        Op::ConsumeOrExpelWindowRight { id: None },
        Op::ToggleColumnTabbedDisplay,
    ];

    for third in &every_op {
        for second in &every_op {
            for first in &every_op {
                // eprintln!("{first:?}, {second:?}, {third:?}");

                let mut layout = Layout::default();
                for op in &setup_ops {
                    op.clone().apply(&mut layout);
                }

                let mut layout = Layout::default();
                first.clone().apply(&mut layout);
                layout.verify_invariants();
                second.clone().apply(&mut layout);
                layout.verify_invariants();
                third.clone().apply(&mut layout);
                layout.verify_invariants();
            }
        }
    }
}

#[test]
fn primary_active_workspace_idx_not_updated_on_output_add() {
    let ops = [
        Op::AddOutput(1),
        Op::AddOutput(2),
        Op::FocusOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(0),
        },
        Op::FocusOutput(2),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::RemoveOutput(2),
        Op::FocusWorkspace(3),
        Op::AddOutput(2),
    ];

    check_ops(ops);
}

#[test]
fn window_closed_on_previous_workspace() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(0),
        },
        Op::FocusWorkspaceDown,
        Op::CloseWindow(0),
    ];

    check_ops(ops);
}

#[test]
fn removing_output_must_keep_empty_focus_on_primary() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(0),
        },
        Op::AddOutput(2),
        Op::RemoveOutput(1),
    ];

    let layout = check_ops(ops);

    let MonitorSet::Normal { monitors, .. } = layout.monitor_set else {
        unreachable!()
    };

    // The workspace from the removed output was inserted at position 0, so the active workspace
    // must change to 1 to keep the focus on the empty workspace.
    assert_eq!(monitors[0].active_workspace_idx, 1);
}

#[test]
fn move_to_workspace_by_idx_does_not_leave_empty_workspaces() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(0),
        },
        Op::AddOutput(2),
        Op::FocusOutput(2),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::RemoveOutput(1),
        Op::MoveWindowToWorkspace {
            window_id: Some(0),
            workspace_idx: 2,
        },
    ];

    let layout = check_ops(ops);

    let MonitorSet::Normal { monitors, .. } = layout.monitor_set else {
        unreachable!()
    };

    assert!(monitors[0].workspaces[1].has_windows());
}

#[test]
fn empty_workspaces_dont_move_back_to_original_output() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::FocusWorkspaceDown,
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddOutput(2),
        Op::RemoveOutput(1),
        Op::FocusWorkspace(1),
        Op::CloseWindow(1),
        Op::AddOutput(1),
    ];

    check_ops(ops);
}

#[test]
fn named_workspaces_dont_update_original_output_on_adding_window() {
    let ops = [
        Op::AddOutput(1),
        Op::SetWorkspaceName {
            new_ws_name: 1,
            ws_name: None,
        },
        Op::AddOutput(2),
        Op::RemoveOutput(1),
        Op::FocusWorkspaceUp,
        // Adding a window updates the original output for unnamed workspaces.
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        // Connecting the previous output should move the named workspace back since its
        // original output wasn't updated.
        Op::AddOutput(1),
    ];

    let layout = check_ops(ops);
    let (mon, _, ws) = layout
        .workspaces()
        .find(|(_, _, ws)| ws.name().is_some())
        .unwrap();
    assert!(ws.name().is_some()); // Sanity check.
    let mon = mon.unwrap();
    assert_eq!(mon.output_name(), "output1");
}

#[test]
fn workspaces_update_original_output_on_moving_to_same_output() {
    let ops = [
        Op::AddOutput(1),
        Op::SetWorkspaceName {
            new_ws_name: 1,
            ws_name: None,
        },
        Op::AddOutput(2),
        Op::RemoveOutput(1),
        Op::FocusWorkspaceUp,
        Op::MoveWorkspaceToOutput(2),
        Op::AddOutput(1),
    ];

    let layout = check_ops(ops);
    let (mon, _, ws) = layout
        .workspaces()
        .find(|(_, _, ws)| ws.name().is_some())
        .unwrap();
    assert!(ws.name().is_some()); // Sanity check.
    let mon = mon.unwrap();
    assert_eq!(mon.output_name(), "output2");
}

#[test]
fn workspaces_update_original_output_on_moving_to_same_monitor() {
    let ops = [
        Op::AddOutput(1),
        Op::SetWorkspaceName {
            new_ws_name: 1,
            ws_name: None,
        },
        Op::AddOutput(2),
        Op::RemoveOutput(1),
        Op::FocusWorkspaceUp,
        Op::MoveWorkspaceToMonitor {
            ws_name: Some(1),
            output_id: 2,
        },
        Op::AddOutput(1),
    ];

    let layout = check_ops(ops);
    let (mon, _, ws) = layout
        .workspaces()
        .find(|(_, _, ws)| ws.name().is_some())
        .unwrap();
    assert!(ws.name().is_some()); // Sanity check.
    let mon = mon.unwrap();
    assert_eq!(mon.output_name(), "output2");
}

#[test]
fn large_negative_height_change() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::SetWindowHeight {
            id: None,
            change: SizeChange::AdjustProportion(-1e129),
        },
    ];

    let mut options = Options::default();
    options.layout.border.off = false;
    options.layout.border.width = 1.;

    check_ops_with_options(options, ops);
}

#[test]
fn large_max_size() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams {
                min_max_size: (Size::from((0, 0)), Size::from((i32::MAX, i32::MAX))),
                ..TestWindowParams::new(1)
            },
        },
    ];

    let mut options = Options::default();
    options.layout.border.off = false;
    options.layout.border.width = 1.;

    check_ops_with_options(options, ops);
}

#[test]
fn workspace_cleanup_during_switch() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::FocusWorkspaceDown,
        Op::CloseWindow(1),
    ];

    check_ops(ops);
}

#[test]
fn workspace_transfer_during_switch() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddOutput(2),
        Op::FocusOutput(2),
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::RemoveOutput(1),
        Op::FocusWorkspaceDown,
        Op::FocusWorkspaceDown,
        Op::AddOutput(1),
    ];

    check_ops(ops);
}

#[test]
fn workspace_transfer_during_switch_from_last() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddOutput(2),
        Op::RemoveOutput(1),
        Op::FocusWorkspaceUp,
        Op::AddOutput(1),
    ];

    check_ops(ops);
}

#[test]
fn workspace_transfer_during_switch_gets_cleaned_up() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::RemoveOutput(1),
        Op::AddOutput(2),
        Op::MoveColumnToWorkspaceDown(true),
        Op::MoveColumnToWorkspaceDown(true),
        Op::AddOutput(1),
    ];

    check_ops(ops);
}

#[test]
fn move_workspace_to_output() {
    let ops = [
        Op::AddOutput(1),
        Op::AddOutput(2),
        Op::FocusOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(0),
        },
        Op::MoveWorkspaceToOutput(2),
    ];

    let layout = check_ops(ops);

    let MonitorSet::Normal {
        monitors,
        active_monitor_idx,
        ..
    } = layout.monitor_set
    else {
        unreachable!()
    };

    assert_eq!(active_monitor_idx, 1);
    assert_eq!(monitors[0].workspaces.len(), 1);
    assert!(!monitors[0].workspaces[0].has_windows());
    assert_eq!(monitors[1].active_workspace_idx, 0);
    assert_eq!(monitors[1].workspaces.len(), 2);
    assert!(monitors[1].workspaces[0].has_windows());
}

#[test]
fn open_right_of_on_different_workspace() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::FocusWorkspaceDown,
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindowNextTo {
            params: TestWindowParams::new(3),
            next_to_id: 1,
        },
    ];

    let layout = check_ops(ops);

    let MonitorSet::Normal { monitors, .. } = layout.monitor_set else {
        unreachable!()
    };

    let mon = monitors.into_iter().next().unwrap();
    assert_eq!(
        mon.active_workspace_idx, 1,
        "the second workspace must remain active"
    );
    assert_eq!(
        mon.workspaces[0].scrolling().active_column_idx(),
        1,
        "the new window must become active"
    );
}

#[test]
// empty_workspace_above_first = true
fn open_right_of_on_different_workspace_ewaf() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::FocusWorkspaceDown,
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindowNextTo {
            params: TestWindowParams::new(3),
            next_to_id: 1,
        },
    ];

    let options = Options {
        layout: niri_config::Layout {
            empty_workspace_above_first: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let layout = check_ops_with_options(options, ops);

    let MonitorSet::Normal { monitors, .. } = layout.monitor_set else {
        unreachable!()
    };

    let mon = monitors.into_iter().next().unwrap();
    assert_eq!(
        mon.active_workspace_idx, 2,
        "the second workspace must remain active"
    );
    assert_eq!(
        mon.workspaces[1].scrolling().active_column_idx(),
        1,
        "the new window must become active"
    );
}

#[test]
fn removing_all_outputs_preserves_empty_named_workspaces() {
    let ops = [
        Op::AddOutput(1),
        Op::AddNamedWorkspace {
            ws_name: 1,
            output_names: vec![],
            layout_config: None,
        },
        Op::AddNamedWorkspace {
            ws_name: 2,
            output_names: vec![],
            layout_config: None,
        },
        Op::RemoveOutput(1),
    ];

    let layout = check_ops(ops);

    let MonitorSet::NoOutputs { workspaces } = layout.monitor_set else {
        unreachable!()
    };

    assert_eq!(workspaces.len(), 2);
}

#[test]
fn config_change_updates_cached_sizes() {
    let mut config = Config::default();
    let border = &mut config.layout.border;
    border.off = false;
    border.width = 2.;

    let mut layout = Layout::new(Clock::default(), &config);

    Op::AddWindow {
        params: TestWindowParams {
            bbox: Rectangle::from_size(Size::from((1280, 200))),
            ..TestWindowParams::new(1)
        },
    }
    .apply(&mut layout);

    config.layout.border.width = 4.;
    layout.update_config(&config);

    layout.verify_invariants();
}

#[test]
fn preset_height_change_removes_preset() {
    let mut config = Config::default();
    config.layout.preset_window_heights = vec![PresetSize::Fixed(1), PresetSize::Fixed(2)];

    let mut layout = Layout::new(Clock::default(), &config);

    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::ConsumeOrExpelWindowLeft { id: None },
        Op::SwitchPresetWindowHeight { id: None },
        Op::SwitchPresetWindowHeight { id: None },
    ];
    for op in ops {
        op.apply(&mut layout);
    }

    // Leave only one.
    config.layout.preset_window_heights = vec![PresetSize::Fixed(1)];

    layout.update_config(&config);

    layout.verify_invariants();
}

#[test]
fn set_window_height_recomputes_to_auto() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(0),
        },
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::ConsumeOrExpelWindowLeft { id: None },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::ConsumeOrExpelWindowLeft { id: None },
        Op::SetWindowHeight {
            id: None,
            change: SizeChange::SetFixed(100),
        },
        Op::FocusWindowUp,
        Op::SetWindowHeight {
            id: None,
            change: SizeChange::SetFixed(200),
        },
    ];

    check_ops(ops);
}

#[test]
fn one_window_in_column_becomes_weight_1() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(0),
        },
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::ConsumeOrExpelWindowLeft { id: None },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::ConsumeOrExpelWindowLeft { id: None },
        Op::SetWindowHeight {
            id: None,
            change: SizeChange::SetFixed(100),
        },
        Op::Communicate(2),
        Op::FocusWindowUp,
        Op::SetWindowHeight {
            id: None,
            change: SizeChange::SetFixed(200),
        },
        Op::Communicate(1),
        Op::CloseWindow(0),
        Op::CloseWindow(1),
    ];

    check_ops(ops);
}

#[test]
fn fixed_height_takes_max_non_auto_into_account() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(0),
        },
        Op::SetWindowHeight {
            id: Some(0),
            change: SizeChange::SetFixed(704),
        },
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::ConsumeOrExpelWindowLeft { id: None },
    ];

    let options = Options {
        layout: niri_config::Layout {
            border: niri_config::Border {
                off: false,
                width: 4.,
                ..Default::default()
            },
            gaps: 0.,
            ..Default::default()
        },
        ..Default::default()
    };
    check_ops_with_options(options, ops);
}

#[test]
fn start_interactive_move_then_remove_window() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(0),
        },
        Op::InteractiveMoveBegin {
            window: 0,
            output_idx: 1,
            px: 0.,
            py: 0.,
        },
        Op::CloseWindow(0),
    ];

    check_ops(ops);
}

#[test]
fn interactive_move_onto_empty_output() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(0),
        },
        Op::InteractiveMoveBegin {
            window: 0,
            output_idx: 1,
            px: 0.,
            py: 0.,
        },
        Op::AddOutput(2),
        Op::InteractiveMoveUpdate {
            window: 0,
            dx: 1000.,
            dy: 0.,
            output_idx: 2,
            px: 0.,
            py: 0.,
        },
        Op::InteractiveMoveEnd { window: 0 },
    ];

    check_ops(ops);
}

#[test]
fn interactive_move_onto_empty_output_ewaf() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(0),
        },
        Op::InteractiveMoveBegin {
            window: 0,
            output_idx: 1,
            px: 0.,
            py: 0.,
        },
        Op::AddOutput(2),
        Op::InteractiveMoveUpdate {
            window: 0,
            dx: 1000.,
            dy: 0.,
            output_idx: 2,
            px: 0.,
            py: 0.,
        },
        Op::InteractiveMoveEnd { window: 0 },
    ];

    let options = Options {
        layout: niri_config::Layout {
            empty_workspace_above_first: true,
            ..Default::default()
        },
        ..Default::default()
    };
    check_ops_with_options(options, ops);
}

#[test]
fn interactive_move_onto_last_workspace() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(0),
        },
        Op::InteractiveMoveBegin {
            window: 0,
            output_idx: 1,
            px: 0.,
            py: 0.,
        },
        Op::InteractiveMoveUpdate {
            window: 0,
            dx: 1000.,
            dy: 0.,
            output_idx: 1,
            px: 0.,
            py: 0.,
        },
        Op::FocusWorkspaceDown,
        Op::AdvanceAnimations { msec_delta: 1000 },
        Op::InteractiveMoveEnd { window: 0 },
    ];

    check_ops(ops);
}

#[test]
fn interactive_move_onto_first_empty_workspace() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::InteractiveMoveBegin {
            window: 1,
            output_idx: 1,
            px: 0.,
            py: 0.,
        },
        Op::InteractiveMoveUpdate {
            window: 1,
            dx: 1000.,
            dy: 0.,
            output_idx: 1,
            px: 0.,
            py: 0.,
        },
        Op::FocusWorkspaceUp,
        Op::AdvanceAnimations { msec_delta: 1000 },
        Op::InteractiveMoveEnd { window: 1 },
    ];
    let options = Options {
        layout: niri_config::Layout {
            empty_workspace_above_first: true,
            ..Default::default()
        },
        ..Default::default()
    };
    check_ops_with_options(options, ops);
}

#[test]
fn output_active_workspace_is_preserved() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::FocusWorkspaceDown,
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::RemoveOutput(1),
        Op::AddOutput(1),
    ];

    let layout = check_ops(ops);

    let MonitorSet::Normal { monitors, .. } = layout.monitor_set else {
        unreachable!()
    };

    assert_eq!(monitors[0].active_workspace_idx, 1);
}

#[test]
fn output_active_workspace_is_preserved_with_other_outputs() {
    let ops = [
        Op::AddOutput(1),
        Op::AddOutput(2),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::FocusWorkspaceDown,
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::RemoveOutput(1),
        Op::AddOutput(1),
    ];

    let layout = check_ops(ops);

    let MonitorSet::Normal { monitors, .. } = layout.monitor_set else {
        unreachable!()
    };

    assert_eq!(monitors[1].active_workspace_idx, 1);
}

#[test]
fn named_workspace_to_output() {
    let ops = [
        Op::AddNamedWorkspace {
            ws_name: 1,
            output_names: vec![],
            layout_config: None,
        },
        Op::AddOutput(1),
        Op::MoveWorkspaceToOutput(1),
        Op::FocusWorkspaceUp,
    ];
    check_ops(ops);
}

#[test]
// empty_workspace_above_first = true
fn named_workspace_to_output_ewaf() {
    let ops = [
        Op::AddNamedWorkspace {
            ws_name: 1,
            output_names: vec![2],
            layout_config: None,
        },
        Op::AddOutput(1),
        Op::AddOutput(2),
    ];
    let options = Options {
        layout: niri_config::Layout {
            empty_workspace_above_first: true,
            ..Default::default()
        },
        ..Default::default()
    };
    check_ops_with_options(options, ops);
}

#[test]
fn move_window_to_empty_workspace_above_first() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::MoveWorkspaceUp,
        Op::MoveWorkspaceDown,
        Op::FocusWorkspaceUp,
        Op::MoveWorkspaceDown,
    ];
    let options = Options {
        layout: niri_config::Layout {
            empty_workspace_above_first: true,
            ..Default::default()
        },
        ..Default::default()
    };
    check_ops_with_options(options, ops);
}

#[test]
fn move_window_to_different_output() {
    let ops = [
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddOutput(1),
        Op::AddOutput(2),
        Op::MoveWorkspaceToOutput(2),
    ];
    let options = Options {
        layout: niri_config::Layout {
            empty_workspace_above_first: true,
            ..Default::default()
        },
        ..Default::default()
    };
    check_ops_with_options(options, ops);
}

#[test]
fn close_window_empty_ws_above_first() {
    let ops = [
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddOutput(1),
        Op::CloseWindow(1),
    ];
    let options = Options {
        layout: niri_config::Layout {
            empty_workspace_above_first: true,
            ..Default::default()
        },
        ..Default::default()
    };
    check_ops_with_options(options, ops);
}

#[test]
fn add_and_remove_output() {
    let ops = [
        Op::AddOutput(2),
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::RemoveOutput(2),
    ];
    let options = Options {
        layout: niri_config::Layout {
            empty_workspace_above_first: true,
            ..Default::default()
        },
        ..Default::default()
    };
    check_ops_with_options(options, ops);
}

#[test]
fn switch_ewaf_on() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
    ];

    let mut layout = check_ops(ops);
    layout.update_options(Options {
        layout: niri_config::Layout {
            empty_workspace_above_first: true,
            ..Default::default()
        },
        ..Default::default()
    });
    layout.verify_invariants();
}

#[test]
fn switch_ewaf_off() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
    ];

    let options = Options {
        layout: niri_config::Layout {
            empty_workspace_above_first: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut layout = check_ops_with_options(options, ops);
    layout.update_options(Options::default());
    layout.verify_invariants();
}

#[test]
fn interactive_move_drop_on_other_output_during_animation() {
    let ops = [
        Op::AddOutput(3),
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::InteractiveMoveBegin {
            window: 3,
            output_idx: 3,
            px: 0.0,
            py: 0.0,
        },
        Op::FocusWorkspaceDown,
        Op::AddOutput(4),
        Op::InteractiveMoveUpdate {
            window: 3,
            dx: 0.0,
            dy: 8300.68619826683,
            output_idx: 4,
            px: 0.0,
            py: 0.0,
        },
        Op::RemoveOutput(4),
        Op::InteractiveMoveEnd { window: 3 },
    ];
    check_ops(ops);
}

#[test]
fn add_window_next_to_only_interactively_moved_without_outputs() {
    let ops = [
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddOutput(1),
        Op::InteractiveMoveBegin {
            window: 2,
            output_idx: 1,
            px: 0.0,
            py: 0.0,
        },
        Op::InteractiveMoveUpdate {
            window: 2,
            dx: 0.0,
            dy: 3586.692842955048,
            output_idx: 1,
            px: 0.0,
            py: 0.0,
        },
        Op::RemoveOutput(1),
        // We have no outputs, and the only existing window is interactively moved, meaning there
        // are no workspaces either.
        Op::AddWindowNextTo {
            params: TestWindowParams::new(3),
            next_to_id: 2,
        },
    ];

    check_ops(ops);
}

#[test]
fn interactive_move_toggle_floating_ends_dnd_gesture() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::InteractiveMoveBegin {
            window: 2,
            output_idx: 1,
            px: 0.0,
            py: 0.0,
        },
        Op::InteractiveMoveUpdate {
            window: 2,
            dx: 0.0,
            dy: 3586.692842955048,
            output_idx: 1,
            px: 0.0,
            py: 0.0,
        },
        Op::Refresh { is_active: false },
        Op::ToggleWindowFloating { id: None },
        Op::InteractiveMoveEnd { window: 2 },
    ];

    check_ops(ops);
}

#[test]
fn interactive_move_from_workspace_with_layout_config() {
    let ops = [
        Op::AddNamedWorkspace {
            ws_name: 1,
            output_names: vec![2],
            layout_config: Some(Box::new(niri_config::LayoutPart {
                border: Some(niri_config::BorderRule {
                    on: true,
                    ..Default::default()
                }),
                ..Default::default()
            })),
        },
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::InteractiveMoveBegin {
            window: 2,
            output_idx: 1,
            px: 0.0,
            py: 0.0,
        },
        Op::InteractiveMoveUpdate {
            window: 2,
            dx: 0.0,
            dy: 3586.692842955048,
            output_idx: 1,
            px: 0.0,
            py: 0.0,
        },
        // Now remove and add the output. It will have the same workspace.
        Op::RemoveOutput(1),
        Op::AddOutput(1),
        Op::InteractiveMoveUpdate {
            window: 2,
            dx: 0.0,
            dy: 0.0,
            output_idx: 1,
            px: 0.0,
            py: 0.0,
        },
        // Now move onto a different workspace.
        Op::FocusWorkspaceDown,
        Op::CompleteAnimations,
        Op::InteractiveMoveUpdate {
            window: 2,
            dx: 0.0,
            dy: 0.0,
            output_idx: 1,
            px: 0.0,
            py: 0.0,
        },
    ];

    check_ops(ops);
}

#[test]
fn set_width_fixed_negative() {
    let ops = [
        Op::AddOutput(3),
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::ToggleWindowFloating { id: Some(3) },
        Op::SetColumnWidth(SizeChange::SetFixed(-100)),
    ];
    check_ops(ops);
}

#[test]
fn set_height_fixed_negative() {
    let ops = [
        Op::AddOutput(3),
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::ToggleWindowFloating { id: Some(3) },
        Op::SetWindowHeight {
            id: None,
            change: SizeChange::SetFixed(-100),
        },
    ];
    check_ops(ops);
}

#[test]
fn interactive_resize_to_negative() {
    let ops = [
        Op::AddOutput(3),
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::ToggleWindowFloating { id: Some(3) },
        Op::InteractiveResizeBegin {
            window: 3,
            edges: ResizeEdge::BOTTOM_RIGHT,
        },
        Op::InteractiveResizeUpdate {
            window: 3,
            dx: -10000.,
            dy: -10000.,
        },
    ];
    check_ops(ops);
}

#[test]
fn windows_on_other_workspaces_remain_activated() {
    let ops = [
        Op::AddOutput(3),
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::FocusWorkspaceDown,
        Op::Refresh { is_active: true },
    ];

    let layout = check_ops(ops);
    let (_, win) = layout.windows().next().unwrap();
    assert!(win.0.pending_activated.get());
}

#[test]
fn stacking_add_parent_brings_up_child() {
    let ops = [
        Op::AddOutput(0),
        Op::AddWindow {
            params: TestWindowParams {
                is_floating: true,
                parent_id: Some(1),
                ..TestWindowParams::new(0)
            },
        },
        Op::AddWindow {
            params: TestWindowParams {
                is_floating: true,
                ..TestWindowParams::new(1)
            },
        },
    ];

    check_ops(ops);
}

#[test]
fn stacking_add_parent_brings_up_descendants() {
    let ops = [
        Op::AddOutput(0),
        Op::AddWindow {
            params: TestWindowParams {
                is_floating: true,
                parent_id: Some(2),
                ..TestWindowParams::new(0)
            },
        },
        Op::AddWindow {
            params: TestWindowParams {
                is_floating: true,
                parent_id: Some(0),
                ..TestWindowParams::new(1)
            },
        },
        Op::AddWindow {
            params: TestWindowParams {
                is_floating: true,
                ..TestWindowParams::new(2)
            },
        },
    ];

    check_ops(ops);
}

#[test]
fn stacking_activate_brings_up_descendants() {
    let ops = [
        Op::AddOutput(0),
        Op::AddWindow {
            params: TestWindowParams {
                is_floating: true,
                ..TestWindowParams::new(0)
            },
        },
        Op::AddWindow {
            params: TestWindowParams {
                is_floating: true,
                parent_id: Some(0),
                ..TestWindowParams::new(1)
            },
        },
        Op::AddWindow {
            params: TestWindowParams {
                is_floating: true,
                parent_id: Some(1),
                ..TestWindowParams::new(2)
            },
        },
        Op::AddWindow {
            params: TestWindowParams {
                is_floating: true,
                ..TestWindowParams::new(3)
            },
        },
        Op::FocusWindow(0),
    ];

    check_ops(ops);
}

#[test]
fn stacking_set_parent_brings_up_child() {
    let ops = [
        Op::AddOutput(0),
        Op::AddWindow {
            params: TestWindowParams {
                is_floating: true,
                ..TestWindowParams::new(0)
            },
        },
        Op::AddWindow {
            params: TestWindowParams {
                is_floating: true,
                ..TestWindowParams::new(1)
            },
        },
        Op::SetParent {
            id: 0,
            new_parent_id: Some(1),
        },
    ];

    check_ops(ops);
}

#[test]
fn move_window_to_workspace_with_different_active_output() {
    let ops = [
        Op::AddOutput(0),
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(0),
        },
        Op::FocusOutput(1),
        Op::MoveWindowToWorkspace {
            window_id: Some(0),
            workspace_idx: 2,
        },
    ];

    check_ops(ops);
}

#[test]
fn set_first_workspace_name() {
    let ops = [
        Op::AddOutput(0),
        Op::SetWorkspaceName {
            new_ws_name: 0,
            ws_name: None,
        },
    ];

    check_ops(ops);
}

#[test]
fn set_first_workspace_name_ewaf() {
    let ops = [
        Op::AddOutput(0),
        Op::SetWorkspaceName {
            new_ws_name: 0,
            ws_name: None,
        },
    ];

    let options = Options {
        layout: niri_config::Layout {
            empty_workspace_above_first: true,
            ..Default::default()
        },
        ..Default::default()
    };
    check_ops_with_options(options, ops);
}

#[test]
fn set_last_workspace_name() {
    let ops = [
        Op::AddOutput(0),
        Op::AddWindow {
            params: TestWindowParams::new(0),
        },
        Op::FocusWorkspaceDown,
        Op::SetWorkspaceName {
            new_ws_name: 0,
            ws_name: None,
        },
    ];

    check_ops(ops);
}

#[test]
fn move_workspace_to_same_monitor_doesnt_reorder() {
    let ops = [
        Op::AddOutput(0),
        Op::SetWorkspaceName {
            new_ws_name: 0,
            ws_name: None,
        },
        Op::AddWindow {
            params: TestWindowParams::new(0),
        },
        Op::FocusWorkspaceDown,
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::MoveWorkspaceToMonitor {
            ws_name: Some(0),
            output_id: 0,
        },
    ];

    let layout = check_ops(ops);
    let counts: Vec<_> = layout
        .workspaces()
        .map(|(_, _, ws)| ws.windows().count())
        .collect();
    assert_eq!(counts, &[1, 2, 0]);
}

#[test]
fn removing_window_above_preserves_focused_window() {
    let ops = [
        Op::AddOutput(0),
        Op::AddWindow {
            params: TestWindowParams::new(0),
        },
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::FocusColumnFirst,
        Op::ConsumeWindowIntoColumn,
        Op::ConsumeWindowIntoColumn,
        Op::FocusWindowDown,
        Op::CloseWindow(0),
    ];

    let layout = check_ops(ops);
    let win = layout.focus().unwrap();
    assert_eq!(win.0.id, 1);
}

#[test]
fn preset_column_width_fixed_correct_with_border() {
    let ops = [
        Op::AddOutput(0),
        Op::AddWindow {
            params: TestWindowParams::new(0),
        },
        Op::SwitchPresetColumnWidth,
    ];

    let options = Options {
        layout: niri_config::Layout {
            preset_column_widths: vec![PresetSize::Fixed(500)],
            ..Default::default()
        },
        ..Default::default()
    };
    let mut layout = check_ops_with_options(options, ops);

    let win = layout.windows().next().unwrap().1;
    assert_eq!(win.requested_size().unwrap().w, 500);

    // Add border.
    let options = Options {
        layout: niri_config::Layout {
            preset_column_widths: vec![PresetSize::Fixed(500)],
            border: niri_config::Border {
                off: false,
                width: 5.,
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };
    layout.update_options(options);

    // With border, the window gets less size.
    let win = layout.windows().next().unwrap().1;
    assert_eq!(win.requested_size().unwrap().w, 490);

    // However, preset fixed width will still work correctly.
    layout.toggle_width(true);
    let win = layout.windows().next().unwrap().1;
    assert_eq!(win.requested_size().unwrap().w, 500);
}

#[test]
fn preset_column_width_reset_after_set_width() {
    let ops = [
        Op::AddOutput(0),
        Op::AddWindow {
            params: TestWindowParams::new(0),
        },
        Op::SwitchPresetColumnWidth,
        Op::SetWindowWidth {
            id: None,
            change: SizeChange::AdjustFixed(-10),
        },
        Op::SwitchPresetColumnWidth,
    ];

    let options = Options {
        layout: niri_config::Layout {
            preset_column_widths: vec![PresetSize::Fixed(500), PresetSize::Fixed(1000)],
            ..Default::default()
        },
        ..Default::default()
    };
    let layout = check_ops_with_options(options, ops);
    let win = layout.windows().next().unwrap().1;
    assert_eq!(win.requested_size().unwrap().w, 500);
}

#[test]
fn move_column_to_workspace_unfocused_with_multiple_monitors() {
    let ops = [
        Op::AddOutput(1),
        Op::SetWorkspaceName {
            new_ws_name: 101,
            ws_name: None,
        },
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::FocusWorkspaceDown,
        Op::SetWorkspaceName {
            new_ws_name: 102,
            ws_name: None,
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddOutput(2),
        Op::FocusOutput(2),
        Op::SetWorkspaceName {
            new_ws_name: 201,
            ws_name: None,
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::AddWindow {
            params: TestWindowParams::new(4),
        },
        Op::MoveColumnToOutput {
            output_id: 1,
            target_ws_idx: Some(0),
            activate: false,
        },
        Op::FocusOutput(1),
    ];

    let layout = check_ops(ops);

    assert_eq!(layout.active_workspace().unwrap().name().unwrap(), "ws102");

    for (mon, win) in layout.windows() {
        let mon = mon.unwrap();
        let ws = mon
            .workspaces
            .iter()
            .find(|w| w.has_window(win.id()))
            .unwrap();

        assert_eq!(
            ws.name().unwrap(),
            match win.id() {
                1 | 4 => "ws101",
                2 => "ws102",
                3 => "ws201",
                _ => unreachable!(),
            }
        );
    }
}

#[test]
fn move_column_to_workspace_down_focus_false_on_floating_window() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::ToggleWindowFloating { id: None },
        Op::MoveColumnToWorkspaceDown(false),
    ];

    let layout = check_ops(ops);

    let MonitorSet::Normal { monitors, .. } = layout.monitor_set else {
        unreachable!()
    };

    assert_eq!(monitors[0].active_workspace_idx, 0);
}

#[test]
fn move_column_to_workspace_focus_false_on_floating_window() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::ToggleWindowFloating { id: None },
        Op::MoveColumnToWorkspace(1, false),
    ];

    let layout = check_ops(ops);

    let MonitorSet::Normal { monitors, .. } = layout.monitor_set else {
        unreachable!()
    };

    assert_eq!(monitors[0].active_workspace_idx, 0);
}

#[test]
fn restore_to_floating_persists_across_fullscreen_maximize() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::ToggleWindowFloating { id: None },
        // Maximize then fullscreen.
        Op::MaximizeWindowToEdges { id: None },
        Op::FullscreenWindow(1),
        // Unfullscreen.
        Op::FullscreenWindow(1),
    ];

    let mut layout = check_ops(ops);

    // Unfullscreening should return the window to the maximized state.
    let scrolling = layout.active_workspace().unwrap().scrolling();
    assert!(scrolling.tiles().next().is_some());

    let ops = [
        // Unmaximize.
        Op::MaximizeWindowToEdges { id: None },
    ];
    check_ops_on_layout(&mut layout, ops);

    // Unmaximize should return the window back to floating.
    let scrolling = layout.active_workspace().unwrap().scrolling();
    assert!(scrolling.tiles().next().is_none());
}

#[test]
fn unmaximize_during_fullscreen_does_not_float() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::ToggleWindowFloating { id: None },
        // Maximize then fullscreen.
        Op::MaximizeWindowToEdges { id: None },
        Op::FullscreenWindow(1),
        // Unmaximize.
        Op::MaximizeWindowToEdges { id: None },
    ];

    let mut layout = check_ops(ops);

    // Unmaximize shouldn't have changed the window state since it's fullscreen.
    let scrolling = layout.active_workspace().unwrap().scrolling();
    assert!(scrolling.tiles().next().is_some());

    let ops = [
        // Unfullscreen.
        Op::FullscreenWindow(1),
    ];
    check_ops_on_layout(&mut layout, ops);

    // Unfullscreen should return the window back to floating.
    let scrolling = layout.active_workspace().unwrap().scrolling();
    assert!(scrolling.tiles().next().is_none());
}

#[test]
fn grid_overview_preserves_fullscreen() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::FullscreenWindow(1),
        Op::ToggleGridOverview,
    ];

    let mut layout = check_ops(ops);

    assert!(layout.is_grid_overview_open());
    let (_, win) = layout.windows().find(|(_, win)| *win.id() == 1).unwrap();
    assert_eq!(win.pending_sizing_mode(), SizingMode::Fullscreen);

    check_ops_on_layout(&mut layout, [Op::ToggleGridOverview]);

    assert!(!layout.is_grid_overview_open());
    let (_, win) = layout.windows().find(|(_, win)| *win.id() == 1).unwrap();
    assert_eq!(win.pending_sizing_mode(), SizingMode::Fullscreen);
}

#[test]
fn grid_overview_fullscreen_preview_is_larger_than_normal_column() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::Communicate(1),
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::Communicate(2),
        Op::FocusWindow(1),
        Op::FullscreenWindow(1),
        Op::Communicate(1),
        Op::ToggleGridOverview,
    ];

    let layout = check_ops(ops);
    let go = layout
        .active_workspace()
        .and_then(|ws| ws.grid_overview())
        .unwrap();

    let size_for = |id| {
        go.layout
            .entries
            .iter()
            .find_map(|(item, info)| (item.window_id() == &id).then_some(info.target_size))
            .unwrap()
    };

    let fullscreen = size_for(1);
    let normal = size_for(2);

    assert!(
        fullscreen.w * fullscreen.h > normal.w * normal.h,
        "fullscreen grid preview should be larger than normal column: fullscreen={fullscreen:?}, normal={normal:?}"
    );
}

#[test]
fn grid_overview_packs_visual_gap_with_fullscreen() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::Communicate(1),
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::Communicate(2),
        Op::FocusWindow(1),
        Op::FullscreenWindow(1),
        Op::Communicate(1),
        Op::ToggleGridOverview,
    ];

    let layout = check_ops(ops);
    let go = layout
        .active_workspace()
        .and_then(|ws| ws.grid_overview())
        .unwrap();

    let mut entries: Vec<_> = go
        .layout
        .entries
        .iter()
        .filter_map(|(item, info)| {
            matches!(*item.window_id(), 1 | 2).then_some((info.target_pos, info.target_size))
        })
        .collect();
    entries.sort_by(|(a, _), (b, _)| a.x.partial_cmp(&b.x).unwrap());

    let visual_gap = entries[1].0.x - (entries[0].0.x + entries[0].1.w);

    assert!(
        (visual_gap - go.layout.gap).abs() < 0.0001,
        "grid visual gap should match configured gap: visual_gap={visual_gap}, configured={}",
        go.layout.gap
    );
}

#[test]
fn grid_overview_fill_scale_makes_padding_visual() {
    let ops = vec![
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::Communicate(1),
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::Communicate(2),
        Op::FocusWindow(1),
        Op::FullscreenWindow(1),
        Op::Communicate(1),
        Op::ToggleGridOverview,
    ];

    let bounds_for_padding = |padding| {
        let options = Options {
            grid_overview: niri_config::GridOverview {
                padding: niri_config::GridOverviewPadding::uniform(padding),
                ..Default::default()
            },
            ..Default::default()
        };
        let layout = check_ops_with_options(options, ops.clone());
        let ws = layout.active_workspace().unwrap();
        let area = ws.working_area();
        let go = ws.grid_overview().unwrap();

        let mut min_x = f64::MAX;
        let mut min_y = f64::MAX;
        let mut max_x = f64::MIN;
        let mut max_y = f64::MIN;
        for (_, info) in &go.layout.entries {
            min_x = min_x.min(info.target_pos.x);
            min_y = min_y.min(info.target_pos.y);
            max_x = max_x.max(info.target_pos.x + info.target_size.w);
            max_y = max_y.max(info.target_pos.y + info.target_size.h);
        }

        (area, min_x, min_y, max_x, max_y)
    };

    let (area, min_x, min_y, max_x, max_y) = bounds_for_padding(0.);
    let right = area.loc.x + area.size.w;
    let bottom = area.loc.y + area.size.h;
    let closest_edge = (min_x - area.loc.x)
        .min(min_y - area.loc.y)
        .min(right - max_x)
        .min(bottom - max_y);
    assert!(
        closest_edge.abs() < 0.0001,
        "grid should fill to at least one content edge: closest_edge={closest_edge}"
    );

    let (_, padded_min_x, padded_min_y, padded_max_x, padded_max_y) = bounds_for_padding(100.);
    let area_without_padding = (max_x - min_x) * (max_y - min_y);
    let area_with_padding = (padded_max_x - padded_min_x) * (padded_max_y - padded_min_y);
    assert!(
        area_without_padding > area_with_padding,
        "larger padding should reduce the packed grid size: without={area_without_padding}, with={area_with_padding}"
    );
}

#[test]
fn grid_overview_keeps_window_sizes_in_proportion() {
    let mut floating = TestWindowParams::new(2);
    floating.is_floating = true;

    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::Communicate(1),
        Op::FullscreenWindow(1),
        Op::Communicate(1),
        Op::AddWindow { params: floating },
        Op::Communicate(2),
        Op::ToggleGridOverview,
    ];

    let layout = check_ops(ops);
    let go = layout
        .active_workspace()
        .and_then(|ws| ws.grid_overview())
        .unwrap();

    let info_for = |id| {
        go.layout
            .entries
            .iter()
            .find_map(|(item, info)| (item.window_id() == &id).then_some(info))
            .unwrap()
    };

    let fullscreen = info_for(1);
    let floating = info_for(2);

    // Sizes compare like they do on the workspace, except that a smaller floating window is
    // spared some of the shrinking, to stay recognizable.
    assert!(floating.target_scale >= fullscreen.target_scale);
    assert!(
        floating.target_size.w * floating.target_size.h
            < fullscreen.target_size.w * fullscreen.target_size.h,
        "mixed floating grid preview should not visually dominate fullscreen tiling: fullscreen={:?}, floating={:?}",
        fullscreen.target_size,
        floating.target_size
    );
}

#[test]
fn grid_overview_does_not_upscale_single_floating_window() {
    let mut floating = TestWindowParams::new(1);
    floating.is_floating = true;

    let layout = check_ops_with_options(
        Options::default(),
        [
            Op::AddOutput(1),
            Op::AddWindow { params: floating },
            Op::ToggleGridOverview,
            Op::CompleteAnimations,
        ],
    );
    let go = layout
        .active_workspace()
        .and_then(|ws| ws.grid_overview())
        .unwrap();
    let (_, info) = go.layout.entries.first().unwrap();

    approx::assert_abs_diff_eq!(info.target_scale, 1., epsilon = 0.001);
}

#[test]
fn grid_overview_does_not_upscale_mixed_floating_window() {
    let mut floating = TestWindowParams::new(2);
    floating.is_floating = true;

    let layout = check_ops_with_options(
        Options::default(),
        [
            Op::AddOutput(1),
            Op::AddWindow {
                params: TestWindowParams::new(1),
            },
            Op::AddWindow { params: floating },
            Op::FocusWindow(2),
            Op::ToggleGridOverview,
            Op::CompleteAnimations,
        ],
    );
    let go = layout
        .active_workspace()
        .and_then(|ws| ws.grid_overview())
        .unwrap();
    let (_, info) = go
        .layout
        .entries
        .iter()
        .find(|(item, _)| item.window_id() == &2)
        .unwrap();

    assert!(info.target_scale <= 1.);
}

#[test]
fn grid_overview_preserves_maximized() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::MaximizeWindowToEdges { id: None },
        Op::ToggleGridOverview,
    ];

    let mut layout = check_ops(ops);

    assert!(layout.is_grid_overview_open());
    let (_, win) = layout.windows().find(|(_, win)| *win.id() == 1).unwrap();
    assert_eq!(win.pending_sizing_mode(), SizingMode::Maximized);

    check_ops_on_layout(&mut layout, [Op::ToggleGridOverview]);

    assert!(!layout.is_grid_overview_open());
    let (_, win) = layout.windows().find(|(_, win)| *win.id() == 1).unwrap();
    assert_eq!(win.pending_sizing_mode(), SizingMode::Maximized);
}

#[test]
fn grid_navigation_does_not_activate_window_until_confirmed() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::ConsumeOrExpelWindowLeft { id: None },
        Op::ToggleColumnTabbedDisplay,
        Op::FocusWindow(1),
        Op::ToggleGridOverview,
    ];

    let mut layout = check_ops(ops);
    assert!(layout.is_grid_overview_open());
    assert_eq!(
        layout
            .active_workspace()
            .unwrap()
            .active_window()
            .unwrap()
            .id(),
        &1
    );

    layout.focus_right();

    assert!(layout.is_grid_overview_open());
    assert_eq!(layout.grid_focused_window_id(), Some(2));
    assert_eq!(
        layout
            .active_workspace()
            .unwrap()
            .active_window()
            .unwrap()
            .id(),
        &1
    );

    assert!(layout.confirm_grid_selection_for_window(&2));
    assert_eq!(
        layout
            .active_workspace()
            .unwrap()
            .active_window()
            .unwrap()
            .id(),
        &2
    );
}

#[test]
fn grid_confirming_column_tile_updates_grid_tile_focus_before_close_finishes() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::ConsumeOrExpelWindowLeft { id: None },
        Op::FocusWindow(1),
        Op::ToggleGridOverview,
    ]);

    assert!(layout.is_grid_overview_open());
    assert_eq!(layout.grid_focused_window_id(), Some(1));
    assert!(layout.confirm_grid_selection_for_window(&2));

    let ws = layout.active_workspace().unwrap();
    let item = ws.scrolling().grid_item_for_window(&2).unwrap();
    let super::grid_overview::GridItem::Column { col_idx, .. } = item else {
        panic!("expected a non-tabbed column grid item");
    };
    let tile_idx = ws
        .scrolling()
        .columns()
        .nth(col_idx)
        .and_then(|col| col.position(&2))
        .unwrap();
    let go = ws.grid_overview().unwrap();

    assert_eq!(go.focused_id(), Some(2));
    assert_eq!(go.get_column_tile_focus(col_idx), tile_idx);
}

#[test]
fn grid_expel_from_column_focuses_previous_tile_when_focused_tile_is_expelled() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::ConsumeOrExpelWindowLeft { id: None },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::ConsumeOrExpelWindowLeft { id: None },
        Op::FocusWindow(1),
        Op::ToggleGridOverview,
        Op::CompleteAnimations,
    ]);
    assert_eq!(scrolling_column_ids(&layout), vec![vec![1, 2, 3]]);
    assert!(layout
        .active_workspace_mut()
        .unwrap()
        .set_grid_focus_for_window(&3));

    layout.expel_from_column();
    layout.verify_invariants();

    assert_eq!(scrolling_column_ids(&layout), vec![vec![1, 2], vec![3]]);
    assert_eq!(layout.grid_focused_window_id(), Some(2));
    assert_eq!(
        layout
            .active_workspace()
            .unwrap()
            .grid_overview()
            .unwrap()
            .focused_id(),
        Some(2)
    );
}

#[test]
fn grid_stays_open_when_window_is_added() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::ToggleGridOverview,
    ]);

    check_ops_on_layout(
        &mut layout,
        [Op::AddWindow {
            params: TestWindowParams::new(2),
        }],
    );

    assert!(layout.is_grid_overview_open());
    let go = layout
        .active_workspace()
        .and_then(|ws| ws.grid_overview())
        .unwrap();
    assert!(go
        .layout
        .entries
        .iter()
        .any(|(item, _)| item.window_id() == &2));
}

#[test]
fn grid_activation_of_newly_added_window_keeps_grid_open() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::ToggleGridOverview,
    ]);

    layout.add_window(
        TestWindow::new(TestWindowParams::new(2)),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::No,
    );
    layout.verify_invariants();

    assert!(layout.is_grid_overview_open());
    assert_eq!(layout.grid_focused_window_id(), Some(1));

    layout.activate_window_from_activation(&2);
    layout.verify_invariants();

    assert!(layout.is_grid_overview_open());
    assert_eq!(layout.grid_focused_window_id(), Some(2));
    assert_eq!(layout.focus().map(|window| *window.id()), Some(2));
}

#[test]
fn grid_activation_of_existing_window_closes_grid() {
    let mut layout = three_column_grid_layout(1);
    check_ops_on_layout(&mut layout, [Op::ToggleGridOverview]);

    assert!(layout.is_grid_overview_open());
    assert_eq!(layout.grid_focused_window_id(), Some(1));

    layout.activate_window_from_activation(&2);
    layout.verify_invariants();

    assert!(!layout.is_grid_overview_open());
    assert_eq!(layout.focus().map(|window| *window.id()), Some(2));
}

#[test]
fn grid_implicit_window_width_targets_grid_focus() {
    let mut layout = two_column_grid_focused_on_second();

    layout.set_window_width(None, SizeChange::SetFixed(333));
    layout.verify_invariants();

    let win1 = layout.windows().find(|(_, win)| *win.id() == 1).unwrap().1;
    let win2 = layout.windows().find(|(_, win)| *win.id() == 2).unwrap().1;
    assert_ne!(win1.requested_size().unwrap().w, 333);
    assert_eq!(win2.requested_size().unwrap().w, 333);
}

#[test]
fn grid_implicit_window_height_targets_grid_focus() {
    let mut layout = two_column_grid_focused_on_second();

    layout.set_window_height(None, SizeChange::SetFixed(222));
    layout.verify_invariants();

    let win1 = layout.windows().find(|(_, win)| *win.id() == 1).unwrap().1;
    let win2 = layout.windows().find(|(_, win)| *win.id() == 2).unwrap().1;
    assert_ne!(win1.requested_size().unwrap().h, 222);
    assert_eq!(win2.requested_size().unwrap().h, 222);
}

#[test]
fn grid_column_width_targets_grid_focus() {
    let mut layout = two_column_grid_focused_on_second();

    layout.set_column_width(SizeChange::SetFixed(444));
    layout.verify_invariants();

    let win1 = layout.windows().find(|(_, win)| *win.id() == 1).unwrap().1;
    let win2 = layout.windows().find(|(_, win)| *win.id() == 2).unwrap().1;
    assert_ne!(win1.requested_size().unwrap().w, 444);
    assert_eq!(win2.requested_size().unwrap().w, 444);
}

#[test]
fn grid_full_width_targets_grid_focus() {
    let mut layout = two_column_grid_focused_on_second();

    layout.toggle_full_width();
    layout.verify_invariants();

    let win1 = layout.windows().find(|(_, win)| *win.id() == 1).unwrap().1;
    let win2 = layout.windows().find(|(_, win)| *win.id() == 2).unwrap().1;
    assert!(win2.requested_size().unwrap().w > win1.requested_size().unwrap().w);
}

#[test]
fn grid_fullscreen_and_maximize_target_grid_focus() {
    let mut fullscreen = two_column_grid_focused_on_second();
    let focus = *fullscreen.focus().unwrap().id();
    assert_eq!(focus, 2);

    fullscreen.toggle_fullscreen(&focus);
    fullscreen.verify_invariants();

    let win1 = fullscreen
        .windows()
        .find(|(_, win)| *win.id() == 1)
        .unwrap()
        .1;
    let win2 = fullscreen
        .windows()
        .find(|(_, win)| *win.id() == 2)
        .unwrap()
        .1;
    assert_eq!(win1.pending_sizing_mode(), SizingMode::Normal);
    assert_eq!(win2.pending_sizing_mode(), SizingMode::Fullscreen);

    let mut maximized = two_column_grid_focused_on_second();
    let focus = *maximized.focus().unwrap().id();
    assert_eq!(focus, 2);

    maximized.toggle_maximized(&focus);
    maximized.verify_invariants();

    let win1 = maximized
        .windows()
        .find(|(_, win)| *win.id() == 1)
        .unwrap()
        .1;
    let win2 = maximized
        .windows()
        .find(|(_, win)| *win.id() == 2)
        .unwrap()
        .1;
    assert_eq!(win1.pending_sizing_mode(), SizingMode::Normal);
    assert_eq!(win2.pending_sizing_mode(), SizingMode::Maximized);
}

#[test]
fn grid_closing_keeps_all_tabbed_items_visible() {
    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::ConsumeOrExpelWindowLeft { id: None },
        Op::ToggleColumnTabbedDisplay,
        Op::FocusWindow(1),
        Op::ToggleGridOverview,
    ]);

    let scrolling = layout.active_workspace().unwrap().scrolling();
    let active_item = scrolling.grid_item_for_window(&1).unwrap();
    let inactive_item = scrolling.grid_item_for_window(&2).unwrap();

    // All tabs should be visible during closing (active tab will be rendered on top).
    assert!(scrolling.grid_item_visible_when_closing(&active_item));
    assert!(scrolling.grid_item_visible_when_closing(&inactive_item));
}

#[test]
fn grid_closing_renders_fullscreen_tiling_above_other_grid_items() {
    let mut floating = TestWindowParams::new(3);
    floating.is_floating = true;

    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindow { params: floating },
        Op::FocusWindow(1),
        Op::FullscreenWindow(1),
        Op::Communicate(1),
        Op::ToggleGridOverview,
    ]);

    let ws = layout.active_workspace().unwrap();
    let fullscreen_item = ws.scrolling().grid_item_for_window(&1).unwrap();
    let normal_item = ws.scrolling().grid_item_for_window(&2).unwrap();
    let floating_item = super::grid_overview::GridItem::Floating { window_id: 3 };

    assert!(ws.grid_item_renders_on_top_when_grid_closing_for_tests(&fullscreen_item));
    assert!(!ws.grid_item_renders_on_top_when_grid_closing_for_tests(&normal_item));
    assert!(!ws.grid_item_renders_on_top_when_grid_closing_for_tests(&floating_item));
}

#[test]
fn grid_ignores_floating_windows_with_rule() {
    let mut ignored_floating = TestWindowParams::new(2);
    ignored_floating.is_floating = true;
    ignored_floating.rules = Some(ResolvedWindowRules {
        ignore_grid_overview: Some(true),
        ..ResolvedWindowRules::default()
    });

    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: ignored_floating,
        },
        Op::FocusWindow(1),
        Op::ToggleGridOverview,
    ]);

    assert_eq!(layout.grid_focused_window_id(), Some(1));
    assert!(layout.window_is_in_open_grid_overview(&1));
    assert!(!layout.window_is_in_open_grid_overview(&2));
    assert!(!layout.confirm_grid_selection_for_window(&2));

    let ws = layout.active_workspace().unwrap();
    let hit_pos = ws
        .tiles_with_render_positions()
        .find_map(|(tile, pos, _)| {
            (tile.window().id() == &2).then(|| {
                let size = tile.tile_size();
                pos + Point::from((size.w / 2., size.h / 2.))
            })
        })
        .unwrap();
    let (window, _) = ws.ignored_floating_window_under(hit_pos).unwrap();
    assert_eq!(window.id(), &2);

    layout.activate_window(&2);
    assert_eq!(layout.focus().map(|win| win.id()), Some(&2));
}

#[test]
fn grid_includes_floating_windows_without_ignore_rule() {
    let mut included_floating = TestWindowParams::new(2);
    included_floating.is_floating = true;
    included_floating.rules = Some(ResolvedWindowRules {
        ignore_grid_overview: Some(false),
        ..ResolvedWindowRules::default()
    });

    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: included_floating,
        },
        Op::FocusWindow(1),
        Op::ToggleGridOverview,
    ]);

    assert!(layout.window_is_in_open_grid_overview(&2));

    let ws = layout.active_workspace().unwrap();
    let hit_pos = ws
        .tiles_with_render_positions()
        .find_map(|(tile, pos, _)| {
            (tile.window().id() == &2).then(|| {
                let size = tile.tile_size();
                pos + Point::from((size.w / 2., size.h / 2.))
            })
        })
        .unwrap();
    assert!(ws.ignored_floating_window_under(hit_pos).is_none());
}

fn focused_window_above_floating_layout(enabled: bool) -> Layout<TestWindow> {
    let options = Options {
        layout: niri_config::Layout {
            focused_window_above_floating: enabled,
            ..Default::default()
        },
        ..Default::default()
    };

    let mut floating = TestWindowParams::new(2);
    floating.is_floating = true;

    check_ops_with_options(
        options,
        [
            Op::AddOutput(1),
            Op::AddWindow {
                params: TestWindowParams::new(1),
            },
            Op::AddWindow { params: floating },
            Op::Communicate(1),
            Op::Communicate(2),
            // Put the floating window over the corner of the tiled window.
            Op::MoveFloatingWindow {
                id: Some(2),
                x: PositionChange::SetFixed(0.),
                y: PositionChange::SetFixed(0.),
                animate: false,
            },
        ],
    )
}

/// A point where the tiled window 1 and the floating window 2 overlap.
fn tiled_and_floating_overlap(layout: &Layout<TestWindow>) -> Point<f64, Logical> {
    let ws = layout.active_workspace().unwrap();
    let rect_of = |id: usize| {
        ws.tiles_with_render_positions()
            .find_map(|(tile, pos, _)| {
                (*tile.window().id() == id).then(|| Rectangle::new(pos, tile.tile_size()))
            })
            .unwrap()
    };
    let overlap = rect_of(1).intersection(rect_of(2)).unwrap();
    overlap.loc + overlap.size.downscale(2.).to_point()
}

fn window_id_under(layout: &Layout<TestWindow>, pos: Point<f64, Logical>) -> Option<usize> {
    let ws = layout.active_workspace().unwrap();
    ws.window_under(pos).map(|(win, _)| *win.id())
}

#[test]
fn focused_tiled_window_goes_above_floating() {
    let mut layout = focused_window_above_floating_layout(true);
    let pos = tiled_and_floating_overlap(&layout);

    // The new floating window is focused and on top.
    assert!(!layout
        .active_workspace()
        .unwrap()
        .active_tile_above_floating());
    assert_eq!(window_id_under(&layout, pos), Some(2));

    check_ops_on_layout(&mut layout, [Op::FocusTiling]);
    let ws = layout.active_workspace().unwrap();
    assert!(ws.active_tile_above_floating());
    assert_eq!(window_id_under(&layout, pos), Some(1));
    // The render order matches the hit-test order.
    let order: Vec<_> = ws
        .tiles_with_render_positions()
        .map(|(tile, _, _)| *tile.window().id())
        .collect();
    assert_eq!(order, [1, 2]);

    check_ops_on_layout(&mut layout, [Op::FocusFloating]);
    assert!(!layout
        .active_workspace()
        .unwrap()
        .active_tile_above_floating());
    assert_eq!(window_id_under(&layout, pos), Some(2));

    // Clicking the tiled window raises it.
    layout.activate_window(&1);
    assert_eq!(window_id_under(&layout, pos), Some(1));
}

#[test]
fn focus_follows_mouse_does_not_raise_tiled_window_above_floating() {
    let mut layout = focused_window_above_floating_layout(true);
    let pos = tiled_and_floating_overlap(&layout);

    layout.activate_window_without_raising(&1);
    layout.verify_invariants();
    let ws = layout.active_workspace().unwrap();
    assert_eq!(ws.active_window().map(|win| *win.id()), Some(1));
    assert!(!ws.active_tile_above_floating());
    assert_eq!(window_id_under(&layout, pos), Some(2));
}

#[test]
fn floating_stays_above_focused_tiled_window_by_default() {
    let mut layout = focused_window_above_floating_layout(false);
    let pos = tiled_and_floating_overlap(&layout);

    check_ops_on_layout(&mut layout, [Op::FocusTiling]);
    assert!(!layout
        .active_workspace()
        .unwrap()
        .active_tile_above_floating());
    assert_eq!(window_id_under(&layout, pos), Some(2));
}

#[test]
fn grid_closing_focused_first_column_focuses_right() {
    let mut layout = three_column_grid_layout(1);
    check_ops_on_layout(&mut layout, [Op::ToggleGridOverview]);
    assert_eq!(layout.grid_focused_window_id(), Some(1));

    check_ops_on_layout(&mut layout, [Op::CloseWindow(1)]);

    assert!(layout.is_grid_overview_open());
    assert_eq!(layout.grid_focused_window_id(), Some(2));
}

#[test]
fn grid_closing_whole_column_uses_entry_rearrange() {
    let mut layout = three_column_grid_layout(1);
    check_ops_on_layout(&mut layout, [Op::ToggleGridOverview]);
    assert_eq!(layout.grid_focused_window_id(), Some(1));

    check_ops_on_layout(&mut layout, [Op::CloseWindow(1)]);
    layout.verify_invariants();

    let go = layout
        .active_workspace()
        .and_then(|ws| ws.grid_overview())
        .unwrap();
    assert!(go.window_transition_starts.is_empty());
    let has_column_move_offset = layout
        .active_workspace()
        .unwrap()
        .scrolling()
        .columns()
        .any(|col| col.render_offset().x.abs() > 0.001 || col.render_offset().y.abs() > 0.001);
    assert!(!has_column_move_offset);
    assert_eq!(layout.grid_focused_window_id(), Some(2));
}

#[test]
fn grid_closing_focused_middle_column_focuses_right() {
    let mut layout = three_column_grid_layout(1);
    check_ops_on_layout(&mut layout, [Op::ToggleGridOverview]);
    layout.focus_right();
    layout.verify_invariants();
    assert_eq!(layout.grid_focused_window_id(), Some(2));

    check_ops_on_layout(&mut layout, [Op::CloseWindow(2)]);

    assert!(layout.is_grid_overview_open());
    assert_eq!(layout.grid_focused_window_id(), Some(3));
}

#[test]
fn grid_closing_focused_last_column_focuses_left() {
    let mut layout = three_column_grid_layout(1);
    check_ops_on_layout(&mut layout, [Op::ToggleGridOverview]);
    layout.focus_right();
    layout.verify_invariants();
    layout.focus_right();
    layout.verify_invariants();
    assert_eq!(layout.grid_focused_window_id(), Some(3));

    check_ops_on_layout(&mut layout, [Op::CloseWindow(3)]);

    assert!(layout.is_grid_overview_open());
    assert_eq!(layout.grid_focused_window_id(), Some(2));
}

#[test]
fn grid_closing_last_window_keeps_empty_grid_open_for_close_animation() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::ToggleOverview,
        Op::ToggleGridOverview,
    ]);
    assert!(layout.is_overview_open());
    assert!(layout.is_grid_overview_open());
    assert_eq!(layout.grid_focused_window_id(), Some(1));

    check_ops_on_layout(&mut layout, [Op::CloseWindow(1)]);

    assert!(layout.is_overview_open());
    assert!(layout.is_grid_overview_open());
    assert_eq!(layout.grid_focused_window_id(), None);
}

#[test]
fn grid_closing_nonfocused_column_preserves_grid_focus() {
    let mut layout = three_column_grid_layout(1);
    check_ops_on_layout(&mut layout, [Op::ToggleGridOverview]);
    layout.focus_right();
    layout.verify_invariants();
    layout.focus_right();
    layout.verify_invariants();
    assert_eq!(layout.grid_focused_window_id(), Some(3));

    check_ops_on_layout(&mut layout, [Op::CloseWindow(2)]);

    assert!(layout.is_grid_overview_open());
    assert_eq!(layout.grid_focused_window_id(), Some(3));
}

#[test]
fn grid_closing_column_left_of_focus_keeps_grid_focus() {
    let mut ops = vec![Op::AddOutput(1)];
    for id in 1..=4 {
        ops.push(Op::AddWindow {
            params: TestWindowParams::new(id),
        });
    }
    ops.extend([Op::FocusWindow(1), Op::ToggleGridOverview]);
    let mut layout = check_ops(ops);

    layout.focus_right();
    assert_eq!(layout.grid_focused_window_id(), Some(2));

    // Every column right of the closed one shifts left by one index, so a focus kept as a cell
    // position used to land on the next window.
    check_ops_on_layout(&mut layout, [Op::CloseWindow(1)]);
    assert_eq!(layout.grid_focused_window_id(), Some(2));

    let ws = layout.active_workspace().unwrap();
    let go = ws.grid_overview().unwrap();
    let (item, _) = &go.layout.entries[go.focus];
    assert_eq!(item.window_id(), &2);
}

#[test]
fn grid_column_tile_choice_follows_its_column() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::ConsumeOrExpelWindowLeft { id: None },
        Op::AddWindow {
            params: TestWindowParams::new(4),
        },
        Op::AddWindow {
            params: TestWindowParams::new(5),
        },
        Op::ConsumeOrExpelWindowLeft { id: None },
        Op::FocusWindow(2),
        Op::FocusWindow(4),
        Op::FocusWindow(1),
        Op::ToggleGridOverview,
    ]);
    assert_eq!(
        scrolling_column_ids(&layout),
        vec![vec![1], vec![2, 3], vec![4, 5]]
    );

    // Pick the second window of the middle column, then move on to the last column.
    layout.focus_right();
    assert_eq!(layout.grid_focused_window_id(), Some(2));
    layout.focus_down();
    assert_eq!(layout.grid_focused_window_id(), Some(3));
    layout.focus_right();
    assert_eq!(layout.grid_focused_window_id(), Some(4));

    // The remembered window choices used to be keyed by column index, so they moved over to the
    // neighbouring column when a column to the left closed.
    check_ops_on_layout(&mut layout, [Op::CloseWindow(1)]);
    assert_eq!(layout.grid_focused_window_id(), Some(4));
    layout.focus_left();
    assert_eq!(layout.grid_focused_window_id(), Some(3));
}

#[test]
fn grid_closing_focused_minimized_cell_moves_focus_without_restoring() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::MinimizeWindow(2),
        Op::FocusWindow(1),
        Op::ToggleGridOverview,
    ]);
    assert!(layout
        .active_workspace_mut()
        .unwrap()
        .set_grid_focus_for_window(&2));

    // The minimized window's client goes away while its cell is focused. It used to be restored
    // first, to let the scrolling layout pick a neighbour.
    let active_before = layout
        .active_workspace()
        .unwrap()
        .active_window()
        .map(|w| *w.id());
    check_ops_on_layout(&mut layout, [Op::CloseWindow(2)]);

    assert_eq!(layout.grid_focused_window_id(), Some(3));
    assert_eq!(
        layout
            .active_workspace()
            .unwrap()
            .active_window()
            .map(|w| *w.id()),
        active_before
    );
}

#[test]
fn grid_focus_follows_its_window_when_the_grid_reflows() {
    let mut ops = vec![Op::AddOutput(1)];
    for id in 1..=5 {
        ops.push(Op::AddWindow {
            params: TestWindowParams::new(id),
        });
    }
    ops.extend([Op::FocusWindow(5), Op::ToggleGridOverview]);
    let mut layout = check_ops(ops);
    assert_eq!(layout.grid_focused_window_id(), Some(5));
    let cell_of_5 = |layout: &Layout<TestWindow>| {
        let go = layout.active_workspace().unwrap().grid_overview().unwrap();
        let (_, info) = go
            .layout
            .entries
            .iter()
            .find(|(item, _)| item.window_id() == &5)
            .unwrap();
        info.rect()
    };
    let cell_before = cell_of_5(&layout);

    // Rotate the output, so the grid reflows.
    let output = layout.outputs().next().unwrap().clone();
    output.change_current_state(None, Some(Transform::_90), None, None);
    smithay::desktop::layer_map_for_output(&output).arrange();
    layout.update_output_size(&output);
    layout.verify_invariants();

    assert_ne!(cell_of_5(&layout), cell_before);
    let go = layout.active_workspace().unwrap().grid_overview().unwrap();
    assert_eq!(go.layout.entries[go.focus].0.window_id(), &5);
    assert_eq!(layout.grid_focused_window_id(), Some(5));
}

#[test]
fn grid_activating_a_window_moves_the_grid_focus() {
    let mut layout = three_column_grid_layout(1);
    check_ops_on_layout(&mut layout, [Op::ToggleGridOverview]);
    assert_eq!(layout.grid_focused_window_id(), Some(1));

    layout.activate_window(&3);
    layout.verify_invariants();

    // The grid used to keep highlighting window 1, and Return then activated it again.
    assert_eq!(layout.grid_focused_window_id(), Some(3));
    assert_eq!(layout.focus().map(|win| *win.id()), Some(3));
    layout.confirm_grid_selection();
    assert_eq!(
        layout
            .active_workspace()
            .unwrap()
            .active_window()
            .map(|w| *w.id()),
        Some(3)
    );
}

#[test]
fn grid_confirming_on_another_workspace_switches_properly() {
    let ws_windows = |layout: &Layout<TestWindow>| {
        layout
            .monitors()
            .next()
            .unwrap()
            .workspaces
            .iter()
            .map(|ws| ws.windows().count())
            .collect::<Vec<_>>()
    };

    // The active workspace was emptied by closing its last window.
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::FocusWorkspaceDown,
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::CloseWindow(2),
        Op::ToggleOverview,
        Op::ToggleGridOverview,
    ]);
    assert_eq!(ws_windows(&layout), vec![1, 0, 0]);

    // Assigning the active workspace index directly used to leave the emptied workspace behind.
    assert!(layout.confirm_grid_selection_for_window(&1));
    check_ops_on_layout(&mut layout, [Op::CompleteAnimations]);
    assert_eq!(ws_windows(&layout), vec![1, 0]);

    // The workspace switched away from is remembered for focus-workspace-previous.
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::FocusWorkspaceDown,
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::ToggleGridOverview,
    ]);
    assert!(layout.confirm_grid_selection_for_window(&1));
    check_ops_on_layout(
        &mut layout,
        [Op::CompleteAnimations, Op::FocusWorkspacePrevious],
    );
    assert_eq!(
        layout
            .active_workspace()
            .unwrap()
            .active_window()
            .map(|w| *w.id()),
        Some(2)
    );
}

#[test]
fn activating_minimized_window_without_raising_does_nothing() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::MinimizeWindow(1),
        Op::ToggleGridOverview,
    ]);

    // What focus-follows-mouse does; it used to make the placeholder column active.
    layout.activate_window_without_raising(&1);
    layout.verify_invariants();
    assert!(layout.is_window_minimized(&1));
    assert_eq!(
        layout
            .active_workspace()
            .unwrap()
            .active_window()
            .map(|w| *w.id()),
        Some(2)
    );
}

fn grid_open_flags(layout: &Layout<TestWindow>) -> Vec<(bool, Vec<bool>)> {
    layout
        .monitors()
        .map(|mon| {
            (
                mon.is_grid_overview_open(),
                mon.workspaces
                    .iter()
                    .filter(|ws| ws.has_windows())
                    .map(|ws| ws.is_grid_overview_open())
                    .collect(),
            )
        })
        .collect()
}

#[test]
fn grid_extends_to_a_monitor_connected_while_open() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::ToggleGridOverview,
        Op::AddOutput(2),
        Op::FocusOutput(2),
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
    ]);

    // The new monitor used to stay out of the grid, while the open grid elsewhere made it
    // impossible to open it there.
    assert_eq!(
        grid_open_flags(&layout),
        vec![(true, vec![true]), (true, vec![true])]
    );

    check_ops_on_layout(&mut layout, [Op::ToggleGridOverview]);
    assert_eq!(
        grid_open_flags(&layout),
        vec![(false, vec![false]), (false, vec![false])]
    );
}

#[test]
fn grid_state_survives_unplugging_the_monitor() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::ToggleGridOverview,
        Op::RemoveOutput(1),
        Op::AddOutput(1),
    ]);

    // The workspace used to come back with its grid open on a monitor whose grid was closed,
    // which Escape and close-grid-overview could not close.
    assert_eq!(grid_open_flags(&layout), vec![(false, vec![false])]);
    check_ops_on_layout(&mut layout, [Op::ToggleGridOverview]);
    assert_eq!(grid_open_flags(&layout), vec![(true, vec![true])]);
}

#[test]
fn grid_workspace_moved_between_monitors_takes_their_grid_state() {
    let options = Options {
        grid_overview: niri_config::GridOverview {
            grid_all_monitors: false,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut layout = check_ops_with_options(
        options,
        [
            Op::AddOutput(1),
            Op::AddOutput(2),
            Op::FocusOutput(1),
            Op::AddWindow {
                params: TestWindowParams::new(1),
            },
            Op::ToggleGridOverview,
        ],
    );
    assert_eq!(
        grid_open_flags(&layout),
        vec![(true, vec![true]), (false, vec![])]
    );

    check_ops_on_layout(&mut layout, [Op::MoveWorkspaceToOutput(2)]);
    assert_eq!(
        grid_open_flags(&layout),
        vec![(true, vec![]), (false, vec![false])]
    );
}

#[test]
fn grid_all_monitors_turned_on_while_open_opens_everywhere() {
    let options = Options {
        grid_overview: niri_config::GridOverview {
            grid_all_monitors: false,
            ..Default::default()
        },
        ..Default::default()
    };
    let layout = check_ops_with_options(
        options,
        [
            Op::AddOutput(1),
            Op::AddOutput(2),
            Op::FocusOutput(2),
            Op::AddWindow {
                params: TestWindowParams::new(1),
            },
            Op::FocusOutput(1),
            Op::AddWindow {
                params: TestWindowParams::new(2),
            },
            Op::ToggleGridOverview,
            Op::SetGridAllMonitors(true),
        ],
    );
    assert_eq!(
        grid_open_flags(&layout),
        vec![(true, vec![true]), (true, vec![true])]
    );
}

#[test]
fn window_restored_as_the_grid_closes_doesnt_fade_out() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::MinimizeWindow(1),
        Op::ToggleGridOverview,
    ]);
    // An activation request for the minimized window closes the grid, which starts fading the
    // minimized thumbnails out, then restores the window.
    layout.activate_window_from_activation(&1);
    layout.verify_invariants();

    assert!(!layout.is_window_minimized(&1));
    let tile = layout
        .active_workspace()
        .unwrap()
        .tiles()
        .find(|tile| tile.window().id() == &1)
        .unwrap();
    assert!(tile
        .alpha_animation
        .as_ref()
        .is_none_or(|alpha| alpha.anim.to() == 1.));
}

#[test]
fn grid_move_to_workspace_that_goes_nowhere_keeps_minimized_cell() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::MinimizeWindow(1),
        Op::ToggleGridOverview,
    ]);
    assert!(layout
        .active_workspace_mut()
        .unwrap()
        .set_grid_focus_for_window(&1));

    // This is the first workspace, so there is nowhere up to move. It used to restore the
    // window anyway, before finding out.
    layout.move_to_workspace_up(true);
    layout.move_column_to_workspace_up(true);
    layout.verify_invariants();
    assert!(layout.is_window_minimized(&1));
    assert_eq!(layout.grid_focused_window_id(), Some(1));
}

#[test]
fn grid_unfocused_new_window_keeps_grid_selection() {
    let mut layout = three_column_grid_layout(1);
    check_ops_on_layout(&mut layout, [Op::ToggleGridOverview]);
    layout.focus_right();
    layout.focus_right();
    assert_eq!(layout.grid_focused_window_id(), Some(3));

    // Like a window opening with open-focused false.
    layout.add_window(
        TestWindow::new(TestWindowParams::new(4)),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::No,
    );
    layout.verify_invariants();

    // The grid selection used to jump back to the active window.
    assert_eq!(layout.grid_focused_window_id(), Some(3));
}

#[test]
fn grid_reopened_while_closing_starts_from_the_current_state() {
    let mut layout = three_column_grid_layout(1);
    check_ops_on_layout(
        &mut layout,
        [Op::ToggleGridOverview, Op::CompleteAnimations],
    );

    // Pick window 3 and close the grid; the view scrolls over to it.
    layout.focus_right();
    layout.focus_right();
    check_ops_on_layout(&mut layout, [Op::ToggleGridOverview]);
    assert_eq!(
        layout
            .active_workspace()
            .unwrap()
            .active_window()
            .map(|w| *w.id()),
        Some(3)
    );

    // Window 2 goes away during the close animation, then the grid opens again.
    check_ops_on_layout(&mut layout, [Op::CloseWindow(2), Op::ToggleGridOverview]);

    let ws = layout.active_workspace().unwrap();
    let go = ws.grid_overview().unwrap();
    // It used to keep the previous session's cells, one for the closed window included.
    let cells: Vec<_> = go
        .layout
        .entries
        .iter()
        .map(|(item, _)| *item.window_id())
        .collect();
    assert_eq!(cells, vec![1, 3]);
    assert_eq!(go.saved_active_window_id, Some(3));
    assert_eq!(layout.grid_focused_window_id(), Some(3));
}

#[test]
fn grid_shows_minimized_ignored_window_as_a_minimized_cell() {
    let mut ignored_floating = TestWindowParams::new(2);
    ignored_floating.is_floating = true;
    ignored_floating.rules = Some(ResolvedWindowRules {
        ignore_grid_overview: Some(true),
        ..ResolvedWindowRules::default()
    });

    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: ignored_floating,
        },
        Op::MinimizeWindow(2),
        Op::ToggleGridOverview,
    ]);

    // It used to fade in at its floating position over the grid, with no cell, and stay
    // clickable there.
    assert!(layout.window_is_in_open_grid_overview(&2));
    let ws = layout.active_workspace().unwrap();
    let (tile, tile_pos, _) = ws
        .tiles_with_render_positions()
        .find(|(tile, _, _)| tile.window().id() == &2)
        .unwrap();
    let center = tile_pos + tile.tile_size().downscale(2.).to_point();
    assert!(ws.ignored_floating_window_under(center).is_none());

    // Restoring it from its cell makes it an ignored window again.
    assert!(layout.confirm_grid_selection_for_window(&2));
    layout.verify_invariants();
    assert!(!layout.is_window_minimized(&2));
    check_ops_on_layout(&mut layout, [Op::ToggleGridOverview]);
    assert!(!layout.window_is_in_open_grid_overview(&2));
}

fn grid_with_minimized_ignored_floating() -> Layout<TestWindow> {
    let mut ignored_floating = TestWindowParams::new(2);
    ignored_floating.is_floating = true;
    ignored_floating.bbox = Rectangle::from_size(Size::from((1000, 700)));
    ignored_floating.rules = Some(ResolvedWindowRules {
        ignore_grid_overview: Some(true),
        ..ResolvedWindowRules::default()
    });

    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: ignored_floating,
        },
        Op::MinimizeWindow(2),
        Op::ToggleGridOverview,
        Op::CompleteAnimations,
    ]);
    assert!(layout
        .active_workspace_mut()
        .unwrap()
        .set_grid_focus_for_window(&2));
    layout
}

fn grid_entry_render_scale(layout: &Layout<TestWindow>, window: usize) -> Option<f64> {
    let ws = layout.active_workspace().unwrap();
    let go = ws.grid_overview()?;
    let (item, info) = go
        .layout
        .entries
        .iter()
        .find(|(item, _)| item.window_id() == &window)?;
    Some(ws.grid_item_render_visual_transform(go, item, info).1)
}

#[test]
fn grid_restoring_ignored_window_in_place_keeps_its_cell() {
    let mut layout = grid_with_minimized_ignored_floating();

    // What minimize-window does on a minimized cell. The window used to drop out of the grid
    // straight away: it popped up over the grid at its floating position, and the grid focus
    // jumped to another cell.
    layout.unminimize_window(&2, false);
    layout.verify_invariants();
    assert!(!layout.is_window_minimized(&2));
    assert!(layout.is_grid_overview_open());
    assert!(layout.window_is_in_open_grid_overview(&2));
    assert_eq!(layout.grid_focused_window_id(), Some(2));

    // Minimizing and restoring it again keeps working from the same cell.
    check_ops_on_layout(&mut layout, [Op::MinimizeWindow(2)]);
    assert_eq!(layout.grid_focused_window_id(), Some(2));
    layout.unminimize_window(&2, false);
    layout.verify_invariants();
    assert_eq!(layout.grid_focused_window_id(), Some(2));

    // It is ignored again in the next grid session.
    check_ops_on_layout(
        &mut layout,
        [
            Op::ToggleGridOverview,
            Op::CompleteAnimations,
            Op::ToggleGridOverview,
        ],
    );
    assert!(!layout.window_is_in_open_grid_overview(&2));
    assert!(layout.window_is_in_open_grid_overview(&1));
}

#[test]
fn grid_activating_minimized_ignored_window_flies_it_back() {
    let mut layout = grid_with_minimized_ignored_floating();
    let cell_scale = grid_entry_render_scale(&layout, 2).unwrap();
    assert!(cell_scale < 0.99, "{cell_scale}");

    // Like clicking its cell. Restored, it used to lose its cell right away and appear at its
    // floating position without any animation.
    assert!(layout.confirm_grid_selection_for_window(&2));
    layout.verify_invariants();
    assert!(!layout.is_grid_overview_open());
    assert_eq!(layout.focus().map(|win| *win.id()), Some(2));

    check_ops_on_layout(&mut layout, [Op::AdvanceAnimations { msec_delta: 50 }]);
    let ws = layout.active_workspace().unwrap();
    assert!(ws.is_grid_overview_animation());
    let scale = grid_entry_render_scale(&layout, 2).expect("the closing grid carries the window");
    assert!(
        cell_scale < scale && scale < 1.,
        "{cell_scale} < {scale} < 1"
    );
}

#[test]
fn grid_column_focus_actions_move_the_grid_focus() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::ConsumeOrExpelWindowLeft { id: None },
        Op::AddWindow {
            params: TestWindowParams::new(4),
        },
        Op::FocusWindow(1),
        Op::ToggleGridOverview,
    ]);
    assert_eq!(
        scrolling_column_ids(&layout),
        vec![vec![1], vec![2, 3], vec![4]]
    );
    let active = |layout: &Layout<TestWindow>| {
        layout
            .active_workspace()
            .unwrap()
            .active_window()
            .map(|w| *w.id())
    };

    // These used to act on the scrolling layout hidden behind the grid, leaving the grid focus
    // where it was.
    layout.focus_column_last();
    assert_eq!(layout.grid_focused_window_id(), Some(4));
    layout.focus_column_first();
    assert_eq!(layout.grid_focused_window_id(), Some(1));
    // The middle column's cell remembers its active window, 3.
    layout.focus_column(2);
    assert_eq!(layout.grid_focused_window_id(), Some(3));
    layout.focus_window_top();
    assert_eq!(layout.grid_focused_window_id(), Some(2));
    layout.focus_window_bottom();
    assert_eq!(layout.grid_focused_window_id(), Some(3));
    layout.focus_window_in_column(1);
    assert_eq!(layout.grid_focused_window_id(), Some(2));
    layout.focus_window_down_or_top();
    assert_eq!(layout.grid_focused_window_id(), Some(3));
    layout.focus_window_down_or_top();
    assert_eq!(layout.grid_focused_window_id(), Some(2));
    layout.focus_column_left_or_last();
    assert_eq!(layout.grid_focused_window_id(), Some(1));
    layout.focus_column_left_or_last();
    assert_eq!(layout.grid_focused_window_id(), Some(4));

    // Like grid navigation, none of this activates windows before the grid closes.
    assert_eq!(active(&layout), Some(1));
    layout.verify_invariants();
}

#[test]
fn grid_or_monitor_focus_actions_cross_to_the_other_monitor_at_the_edge() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddOutput(2),
        Op::FocusOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::FocusOutput(2),
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::FocusOutput(1),
        Op::ToggleGridOverview,
    ]);
    let output2 = layout
        .outputs()
        .find(|o| o.name() == "output2")
        .unwrap()
        .clone();

    // The grid has a single cell, so there's nowhere to go within it. These used to keep
    // quietly moving the hidden scrolling layout, or never cross over.
    assert!(layout.focus_column_right_or_output(&output2));
    assert_eq!(layout.active_output().unwrap().name(), "output2");

    let output1 = layout
        .outputs()
        .find(|o| o.name() == "output1")
        .unwrap()
        .clone();
    assert!(layout.focus_window_down_or_output(&output1));
    assert_eq!(layout.active_output().unwrap().name(), "output1");
    layout.verify_invariants();
}

fn grid_cell_ids(layout: &Layout<TestWindow>, output: &str) -> Vec<usize> {
    let mon = layout
        .monitors()
        .find(|mon| mon.output_name() == output)
        .unwrap();
    let ws = mon.active_workspace_ref();
    ws.grid_overview()
        .filter(|go| go.open)
        .map(|go| {
            go.layout
                .entries
                .iter()
                .map(|(item, _)| *item.window_id())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn grid_window_moved_to_another_monitor_gets_a_cell_there() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddOutput(2),
        Op::FocusOutput(2),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::FocusOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::ToggleGridOverview,
    ]);
    let output2 = layout
        .outputs()
        .find(|o| o.name() == "output2")
        .unwrap()
        .clone();

    // The grid focus moves over with activation.
    layout.move_to_output(None, &output2, None, ActivateWindow::Smart);
    layout.verify_invariants();
    assert_eq!(grid_cell_ids(&layout, "output1"), vec![2]);
    // The target grid used to keep its old layout, and its old focus, until a commit.
    assert_eq!(grid_cell_ids(&layout, "output2"), vec![1, 3]);
    assert_eq!(layout.grid_focused_window_id(), Some(3));

    // By window id, the grid it leaves is refreshed too.
    layout.move_to_output(Some(&2), &output2, None, ActivateWindow::No);
    layout.verify_invariants();
    assert_eq!(grid_cell_ids(&layout, "output1"), Vec::<usize>::new());
    assert_eq!(grid_cell_ids(&layout, "output2"), vec![1, 3, 2]);
}

#[test]
fn grid_move_to_workspace_by_window_id_moves_it() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::FocusWindow(1),
        Op::ToggleGridOverview,
    ]);

    // Like `niri msg action move-window-to-workspace --window-id 2 2`. It used to report success
    // and do nothing while the grid was open.
    layout.move_to_workspace(Some(&2), 1, ActivateWindow::No);
    layout.verify_invariants();
    let ws_windows: Vec<Vec<usize>> = layout
        .monitors()
        .next()
        .unwrap()
        .workspaces
        .iter()
        .map(|ws| ws.windows().map(|w| *w.id()).collect())
        .collect();
    assert_eq!(ws_windows[0], vec![1]);
    assert_eq!(ws_windows[1], vec![2]);
    assert_eq!(grid_cell_ids(&layout, "output1"), vec![1]);
    assert_eq!(layout.grid_focused_window_id(), Some(1));
}

#[test]
fn grid_reflows_when_a_window_goes_fullscreen_out_of_its_column() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::ConsumeOrExpelWindowLeft { id: None },
        Op::AddWindow {
            params: TestWindowParams::new(4),
        },
        Op::FocusWindow(1),
        Op::ToggleGridOverview,
    ]);
    layout.focus_column_last();
    assert_eq!(layout.grid_focused_window_id(), Some(4));

    // Window 3 asks to go fullscreen, which moves it into a column of its own.
    layout.set_fullscreen(&3, true);
    layout.verify_invariants();
    assert_eq!(
        scrolling_column_ids(&layout),
        vec![vec![1], vec![2], vec![3], vec![4]]
    );

    // The grid used to keep its three cells until some window committed.
    let go = layout.active_workspace().unwrap().grid_overview().unwrap();
    assert_eq!(go.layout.entries.len(), 4);
    assert_eq!(layout.grid_focused_window_id(), Some(4));
}

#[test]
fn grid_with_a_gap_too_large_keeps_positive_sizes() {
    let options = Options {
        grid_overview: niri_config::GridOverview {
            gap: 5000.,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut ops = vec![Op::AddOutput(1)];
    for id in 1..=9 {
        ops.push(Op::AddWindow {
            params: TestWindowParams::new(id),
        });
    }
    ops.push(Op::ToggleGridOverview);
    let layout = check_ops_with_options(options, ops);

    let go = layout.active_workspace().unwrap().grid_overview().unwrap();
    for (_, info) in &go.layout.entries {
        assert!(info.target_scale > 0., "{info:?}");
        assert!(
            info.target_size.w > 0. && info.target_size.h > 0.,
            "{info:?}"
        );
    }
}

type GridLayoutItems = Vec<(
    super::grid_overview::GridItem<TestWindow>,
    super::grid_overview::GridSource,
)>;

/// A tiled window for [`grid_layout_items`]: column index and tile size.
type GridTiledSpec = (usize, (f64, f64));
/// A floating window for [`grid_layout_items`]: position and size.
type GridFloatingSpec = ((f64, f64), (f64, f64));

/// Grid layout items: tiled windows, with the tabs of one column sharing its index, then floating
/// windows. Window ids count up from 1 in that order.
fn grid_layout_items(tiled: &[GridTiledSpec], floating: &[GridFloatingSpec]) -> GridLayoutItems {
    use super::grid_overview::{GridItem, GridSource};

    let tab_counts = |col_idx: usize| tiled.iter().filter(|(c, _)| *c == col_idx).count();
    let mut items = Vec::new();
    let mut id = 0;
    for (i, &(col_idx, (w, h))) in tiled.iter().enumerate() {
        id += 1;
        let item = if tab_counts(col_idx) > 1 {
            let tile_idx = tiled[..i].iter().filter(|(c, _)| *c == col_idx).count();
            GridItem::Tab {
                col_idx,
                tile_idx,
                window_id: id,
            }
        } else {
            GridItem::Column {
                col_idx,
                window_id: id,
            }
        };
        let source = GridSource {
            size: Size::from((w, h)),
            pos: None,
        };
        items.push((item, source));
    }
    for &((x, y), (w, h)) in floating {
        id += 1;
        let source = GridSource {
            size: Size::from((w, h)),
            pos: Some(Point::from((x, y))),
        };
        items.push((GridItem::Floating { window_id: id }, source));
    }
    items
}

fn assert_grid_layout_is_sound(
    layout: &super::grid_overview::GridLayout<TestWindow>,
    area: Rectangle<f64, Logical>,
    config: &niri_config::GridOverview,
) {
    let padding = config.padding;
    let min_x = area.loc.x + padding.left - 0.001;
    let min_y = area.loc.y + padding.top - 0.001;
    let max_x = area.loc.x + area.size.w - padding.right + 0.001;
    let max_y = area.loc.y + area.size.h - padding.bottom + 0.001;
    for (item, info) in &layout.entries {
        let r = info.rect();
        assert!(
            r.loc.x >= min_x
                && r.loc.y >= min_y
                && r.loc.x + r.size.w <= max_x
                && r.loc.y + r.size.h <= max_y,
            "{:?} at {r:?} leaves the grid",
            item.window_id(),
        );
        // Every window at its own scale, never larger than it is.
        assert!(
            info.target_scale > 0. && info.target_scale <= 1. + 0.000001,
            "{info:?}"
        );
    }
    for (i, (a, info_a)) in layout.entries.iter().enumerate() {
        for (b, info_b) in &layout.entries[i + 1..] {
            let (ra, rb) = (info_a.rect(), info_b.rect());
            // Floating windows are pushed apart until they are at least 40% of the gap apart.
            let floating = info_a.row.is_none() || info_b.row.is_none();
            let gap = if floating {
                layout.gap * 0.4
            } else {
                layout.gap
            } - 0.001;
            let apart = ra.loc.x + ra.size.w + gap <= rb.loc.x
                || rb.loc.x + rb.size.w + gap <= ra.loc.x
                || ra.loc.y + ra.size.h + gap <= rb.loc.y
                || rb.loc.y + rb.size.h + gap <= ra.loc.y;
            assert!(
                apart,
                "{:?} at {ra:?} and {:?} at {rb:?} overlap",
                a.window_id(),
                b.window_id()
            );
        }
    }
}

#[test]
fn grid_layout_keeps_strip_order_and_tabs_together() {
    use super::grid_overview::GridLayout;

    let config = niri_config::GridOverview::default();
    let area = Rectangle::from_size(Size::from((1920., 1080.)));
    // Columns 0 and 1, a tabbed column 2 with three tabs, and columns 3 to 5.
    let tiled = [
        (0, (960., 1040.)),
        (1, (640., 1040.)),
        (2, (960., 1040.)),
        (2, (960., 1040.)),
        (2, (960., 1040.)),
        (3, (1280., 1040.)),
        (4, (640., 1040.)),
        (5, (960., 1040.)),
    ];
    let items = grid_layout_items(&tiled, &[]);
    let layout = GridLayout::compute(&items, area, &config, None);
    assert_grid_layout_is_sound(&layout, area, &config);

    // Reading order (rows top to bottom, then left to right) is strip order.
    let mut reading: Vec<_> = layout
        .entries
        .iter()
        .map(|(item, info)| (info.row.unwrap(), info.target_pos.x, *item.window_id()))
        .collect();
    reading.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
    let ids: Vec<_> = reading.iter().map(|(_, _, id)| *id).collect();
    assert_eq!(ids, (1..=tiled.len()).collect::<Vec<_>>());

    // The three tabs share a row.
    let row_of = |id: usize| layout.entries[id - 1].1.row;
    assert_eq!(row_of(3), row_of(4));
    assert_eq!(row_of(4), row_of(5));
}

#[test]
fn grid_layout_is_sound_for_many_shapes() {
    use super::grid_overview::GridLayout;

    let config = niri_config::GridOverview::default();
    let area = Rectangle::new(Point::from((0., 30.)), Size::from((1920., 1050.)));

    // A small deterministic generator, so that the shapes vary without a dependency.
    let mut seed = 0x2545_f491_u64;
    let mut next = |max: f64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % 10_000) as f64 / 10_000. * max
    };

    for _ in 0..200 {
        let tiled_count = next(9.) as usize;
        let floating_count = next(6.) as usize;
        let mut tiled = Vec::new();
        let mut col_idx = 0;
        for _ in 0..tiled_count {
            // Now and then, a tab in the same column as the previous window.
            if tiled.is_empty() || next(1.) > 0.2 {
                col_idx = tiled.len();
            }
            tiled.push((col_idx, (300. + next(1500.), 400. + next(640.))));
        }
        let floating: Vec<_> = (0..floating_count)
            .map(|_| {
                let size = (150. + next(1200.), 120. + next(700.));
                ((next(1920.) - 100., 30. + next(1050.) - 100.), size)
            })
            .collect();
        if tiled.is_empty() && floating.is_empty() {
            continue;
        }

        let items = grid_layout_items(&tiled, &floating);
        let layout = GridLayout::compute(&items, area, &config, None);
        assert_grid_layout_is_sound(&layout, area, &config);

        // Recomputing from itself, as while the grid stays open, keeps it sound.
        let again = GridLayout::compute(&items, area, &config, Some(&layout));
        assert_grid_layout_is_sound(&again, area, &config);
    }
}

#[test]
fn grid_layout_puts_floating_windows_into_the_gap_of_a_short_row() {
    use super::grid_overview::GridLayout;

    let config = niri_config::GridOverview::default();
    let area = Rectangle::from_size(Size::from((1920., 1080.)));
    // A wide first row and a last row with one window, centered, leaving room on both sides.
    let tiled = [
        (0, (960., 1040.)),
        (1, (640., 1040.)),
        (2, (960., 1040.)),
        (2, (960., 1040.)),
        (2, (960., 1040.)),
        (3, (1280., 1040.)),
    ];
    // Down in the bottom left corner, where the short row leaves room.
    let floating = [((40., 700.), (700., 340.))];
    let items = grid_layout_items(&tiled, &floating);
    let layout = GridLayout::compute(&items, area, &config, None);
    assert_grid_layout_is_sound(&layout, area, &config);

    let tiled_bounds = layout
        .entries
        .iter()
        .filter(|(_, info)| info.row.is_some())
        .map(|(_, info)| info.rect())
        .reduce(|a, b| a.merge(b))
        .unwrap();
    let (_, floating) = layout.entries.last().unwrap();
    assert!(
        tiled_bounds.overlaps(floating.rect()),
        "the floating window at {:?} should sit in the gap within the tiled windows at {tiled_bounds:?}",
        floating.rect()
    );
}

#[test]
fn grid_layout_keeps_its_scale_for_small_changes_while_open() {
    use super::grid_overview::GridLayout;

    let config = niri_config::GridOverview::default();
    let tiled = [(0, (900., 1040.)), (1, (900., 1040.)), (2, (900., 1040.))];
    let items = grid_layout_items(&tiled, &[]);
    let area = |grow: f64| Rectangle::from_size(Size::from((1920. * grow, 1080. * grow)));
    let open = GridLayout::compute(&items, area(1.), &config, None);

    // A little more room: fresh, the windows would grow a bit, but the open grid keeps them.
    let fresh = GridLayout::compute(&items, area(1.03), &config, None);
    let kept = GridLayout::compute(&items, area(1.03), &config, Some(&open));
    assert!(fresh.scale > open.scale && fresh.scale < open.scale * 1.08);
    approx::assert_abs_diff_eq!(kept.scale, open.scale);
    assert_grid_layout_is_sound(&kept, area(1.03), &config);

    // A lot more room, and they grow.
    let kept = GridLayout::compute(&items, area(1.25), &config, Some(&open));
    assert!(
        kept.scale > open.scale * 1.08,
        "{} {}",
        kept.scale,
        open.scale
    );
}

fn floating_centers(
    layout: &super::grid_overview::GridLayout<TestWindow>,
) -> Vec<(usize, Point<f64, Logical>)> {
    layout
        .entries
        .iter()
        .filter(|(item, _)| matches!(item, super::grid_overview::GridItem::Floating { .. }))
        .map(|(item, info)| {
            let r = info.rect();
            let center = r.loc + Point::from((r.size.w / 2., r.size.h / 2.));
            (*item.window_id(), center)
        })
        .collect()
}

#[test]
fn grid_layout_keeps_floating_windows_on_their_side_of_the_screen() {
    use super::grid_overview::GridLayout;

    let config = niri_config::GridOverview {
        gap: 12.,
        padding: niri_config::GridOverviewPadding::uniform(40.),
        ..Default::default()
    };
    let area = Rectangle::from_size(Size::from((1280., 720.)));
    // Two tiled windows, and floating windows along the bottom, large top right and small top
    // left, last opened first like the floating layout has them, as in a screenshot of the
    // problem.
    let tiled = [(0, (628., 704.)), (1, (628., 704.))];
    let floating = [
        ((230., 420.), (700., 290.)),
        ((560., 20.), (600., 380.)),
        ((30., 15.), (150., 210.)),
    ];
    let items = grid_layout_items(&tiled, &floating);
    let layout = GridLayout::compute(&items, area, &config, None);
    assert_grid_layout_is_sound(&layout, area, &config);

    let padding = config.padding;
    let middle: Point<f64, Logical> = Point::from((
        (padding.left + area.size.w - padding.right) / 2.,
        (padding.top + area.size.h - padding.bottom) / 2.,
    ));
    let half: Size<f64, Logical> = Size::from((
        (area.size.w - padding.left - padding.right) / 2.,
        (area.size.h - padding.top - padding.bottom) / 2.,
    ));
    for (((x, y), (w, h)), (id, shown)) in floating.iter().zip(floating_centers(&layout)) {
        let real: Point<f64, Logical> = Point::from((x + w / 2., y + h / 2.));
        let side = |real: f64, shown: f64, middle: f64, half: f64| {
            let real = (real - middle) / half;
            real.abs() <= 0.25 || (shown - middle).signum() == real.signum()
        };
        assert!(
            side(real.x, shown.x, middle.x, half.w) && side(real.y, shown.y, middle.y, half.h),
            "floating window {id} at {real:?} on the workspace shows at {shown:?}: {layout:?}"
        );
    }
}

#[test]
fn grid_up_from_a_centered_last_row_goes_to_the_cell_above_it() {
    use super::grid_overview::{GridDirection, GridItem, GridLayout, GridOverview, GridSource};

    let options = Rc::new(Options::default());
    let items: Vec<_> = (1..=7)
        .map(|id| {
            let item = GridItem::Column {
                col_idx: id - 1,
                window_id: id,
            };
            let source = GridSource {
                size: Size::from((400., 400.)),
                pos: None,
            };
            (item, source)
        })
        .collect();
    let area = Rectangle::from_size(Size::from((1000., 1000.)));
    let mut go = GridOverview::<TestWindow>::new(Clock::with_time(Duration::ZERO), options.clone());
    go.layout = GridLayout::compute(&items, area, &options.grid_overview, None);
    // Rows of 3, 3 and 1; the last cell is centered under the middle one of the row above.
    let rows: Vec<_> = go.layout.entries.iter().map(|(_, info)| info.row).collect();
    let r = |row| Some(row);
    assert_eq!(rows, [r(0), r(0), r(0), r(1), r(1), r(1), r(2)]);

    go.focus = 6;
    go.navigate(GridDirection::Up, |_| 1);
    // It used to go to the first column, by index.
    assert_eq!(go.focus, 4);
}

#[test]
fn grid_retarget_during_spring_overshoot_doesnt_jump() {
    use niri_config::animations::{Kind, SpringParams};

    use super::grid_overview::{GridItem, GridOverview, GridSource};
    use super::OverviewProgress;

    let mut options = Options::default();
    options.animations.window_movement.0 = niri_config::Animation {
        off: false,
        kind: Kind::Spring(SpringParams {
            damping_ratio: 0.3,
            stiffness: 800,
            epsilon: 0.0001,
        }),
    };
    let options = Rc::new(options);
    let mut clock = Clock::with_time(Duration::ZERO);
    let mut go = GridOverview::<TestWindow>::new(clock.clone(), options);
    go.open = true;
    go.progress = Some(OverviewProgress::Open);

    let area = Rectangle::from_size(Size::from((1000., 1000.)));
    let items = |sizes: &[f64]| {
        sizes
            .iter()
            .enumerate()
            .map(|(id, w)| {
                let source = GridSource {
                    size: Size::from((*w, 300.)),
                    pos: None,
                };
                (GridItem::Floating { window_id: id + 1 }, source)
            })
            .collect::<Vec<_>>()
    };
    go.compute_layout(&items(&[300., 300.]), area, true);
    // A change that starts the rearrange animation.
    go.compute_layout(&items(&[300., 900.]), area, true);

    // Run the spring into its overshoot.
    let mut t = Duration::ZERO;
    while go.rearrange_anim.as_ref().unwrap().value() <= 1.05 {
        t += Duration::from_millis(1);
        assert!(t < Duration::from_secs(2), "the spring never overshot");
        clock.set_unadjusted(t);
    }

    // The second window is the one whose cell changes.
    let visual = |go: &GridOverview<TestWindow>| {
        let (item, info) = &go.layout.entries[1];
        go.entry_visual_transform(item, info, info.target_pos)
    };
    let before = visual(&go);

    // A passive recompute, like a window committing a new size, retargets the running animation.
    go.compute_layout(&items(&[300., 600.]), area, false);
    let after = visual(&go);

    approx::assert_abs_diff_eq!(before.0.x, after.0.x, epsilon = 0.5);
    approx::assert_abs_diff_eq!(before.0.y, after.0.y, epsilon = 0.5);
    approx::assert_abs_diff_eq!(before.1, after.1, epsilon = 0.001);
}

#[test]
fn grid_column_inserted_in_the_middle_doesnt_take_over_its_neighbours_cell() {
    use super::grid_overview::{GridItem, GridOverview, GridSource};
    use super::OverviewProgress;

    let options = Rc::new(Options::default());
    let mut go = GridOverview::<TestWindow>::new(Clock::with_time(Duration::ZERO), options);
    go.open = true;
    go.progress = Some(OverviewProgress::Open);

    let area = Rectangle::from_size(Size::from((1600., 1000.)));
    let columns = |ids: &[usize]| {
        ids.iter()
            .enumerate()
            .map(|(col_idx, id)| {
                let item = GridItem::Column {
                    col_idx,
                    window_id: *id,
                };
                let source = GridSource {
                    size: Size::from((400., 300.)),
                    pos: None,
                };
                (item, source)
            })
            .collect::<Vec<_>>()
    };
    go.compute_layout(&columns(&[1, 2, 3]), area, true);
    let old_cell_of_2 = go.find_grid_info(&2).unwrap().target_pos;

    // Window 4 opens as a new column between 1 and 2.
    go.compute_layout(&columns(&[1, 4, 2, 3]), area, true);

    // The new column used to match the old column at its index, window 2's, and fly out of
    // window 2's old cell with window 2's scale.
    let start_of_4 = go
        .entry_positions
        .iter()
        .find(|(item, _)| item.window_id() == &4)
        .map(|(_, pos)| *pos)
        .unwrap();
    assert_ne!(start_of_4, old_cell_of_2);
}

#[test]
fn grid_stays_open_on_workspace_switch() {
    let mut layout = three_column_grid_layout(1);
    check_ops_on_layout(&mut layout, [Op::ToggleGridOverview]);
    layout.focus_right();
    layout.verify_invariants();
    assert_eq!(layout.grid_focused_window_id(), Some(2));

    check_ops_on_layout(&mut layout, [Op::FocusWorkspaceDown]);

    assert!(layout.is_grid_overview_open());

    check_ops_on_layout(&mut layout, [Op::FocusWorkspaceUp]);

    assert!(layout.is_grid_overview_open());
}

fn three_column_grid_layout(active: usize) -> Layout<TestWindow> {
    check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::FocusWindow(active),
    ])
}

fn two_column_grid_focused_on_second() -> Layout<TestWindow> {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::FocusWindow(1),
        Op::ToggleGridOverview,
    ]);
    layout.focus_right();
    layout.verify_invariants();

    assert!(layout.is_grid_overview_open());
    assert_eq!(layout.grid_focused_window_id(), Some(2));
    assert_eq!(
        layout
            .active_workspace()
            .unwrap()
            .active_window()
            .unwrap()
            .id(),
        &1
    );

    layout
}

fn tabbed_column_and_column_grid() -> Layout<TestWindow> {
    tabbed_column_and_column_grid_with_options(Options::default())
}

fn tabbed_column_and_column_grid_with_options(options: Options) -> Layout<TestWindow> {
    check_ops_with_options(
        options,
        [
            Op::AddOutput(1),
            Op::AddWindow {
                params: TestWindowParams::new(1),
            },
            Op::AddWindow {
                params: TestWindowParams::new(2),
            },
            Op::ConsumeOrExpelWindowLeft { id: None },
            Op::ToggleColumnTabbedDisplay,
            Op::AddWindow {
                params: TestWindowParams::new(3),
            },
            Op::ToggleGridOverview,
        ],
    )
}

fn large_grid_layout() -> Layout<TestWindow> {
    let mut ops = vec![Op::AddOutput(1)];
    for id in 1..=3 {
        let mut params = TestWindowParams::new(id);
        params.bbox = Rectangle::from_size(Size::from((1600, 1200)));
        ops.push(Op::AddWindow { params });
    }
    ops.push(Op::FocusWindow(1));
    ops.push(Op::ToggleGridOverview);
    check_ops(ops)
}

fn grid_focus_ring_layout(transition: niri_config::GridFocusRingTransition) -> Layout<TestWindow> {
    let mut options = Options::default();
    options.grid_overview.focus_ring_transition = transition;
    let mut ops = vec![Op::AddOutput(1)];
    for id in 1..=3 {
        ops.push(Op::AddWindow {
            params: TestWindowParams::new(id),
        });
    }
    ops.push(Op::FocusWindow(1));
    ops.push(Op::ToggleGridOverview);
    ops.push(Op::CompleteAnimations);
    let mut layout = check_ops_with_options(options, ops);
    layout.update_render_elements(None);
    layout
}

fn grid_focus_ring_windows(layout: &Layout<TestWindow>) -> Vec<usize> {
    let go = layout.active_workspace().unwrap().grid_overview().unwrap();
    let mut ids: Vec<usize> = go.focus_ring_windows().copied().collect();
    ids.sort();
    ids
}

#[test]
fn grid_focus_ring_grow_shrinks_the_old_ring() {
    let mut layout = grid_focus_ring_layout(niri_config::GridFocusRingTransition::Grow);
    assert_eq!(grid_focus_ring_windows(&layout), [1]);

    layout.focus_right();
    layout.update_render_elements(None);
    // The old ring shrinks back while the new one grows.
    assert_eq!(grid_focus_ring_windows(&layout), [1, 2]);
    assert!(layout.are_animations_ongoing(None));

    check_ops_on_layout(&mut layout, [Op::CompleteAnimations]);
    layout.update_render_elements(None);
    assert_eq!(grid_focus_ring_windows(&layout), [2]);
}

#[test]
fn grid_focus_ring_slide_and_none_keep_one_ring() {
    for transition in [
        niri_config::GridFocusRingTransition::Slide,
        niri_config::GridFocusRingTransition::None,
    ] {
        let mut layout = grid_focus_ring_layout(transition);
        assert_eq!(grid_focus_ring_windows(&layout), [1]);

        layout.focus_right();
        layout.update_render_elements(None);
        assert_eq!(grid_focus_ring_windows(&layout), [2]);
        if transition == niri_config::GridFocusRingTransition::Slide {
            assert!(layout.are_animations_ongoing(None));
        }

        check_ops_on_layout(&mut layout, [Op::CompleteAnimations]);
        layout.update_render_elements(None);
        assert_eq!(grid_focus_ring_windows(&layout), [2]);
    }
}

#[test]
fn grid_focus_ring_follows_window_focus_ring_off() {
    let mut options = Options::default();
    options.layout.focus_ring.off = true;
    let mut layout = check_ops_with_options(
        options,
        [
            Op::AddOutput(1),
            Op::AddWindow {
                params: TestWindowParams::new(1),
            },
            Op::ToggleGridOverview,
            Op::CompleteAnimations,
        ],
    );
    layout.update_render_elements(None);
    assert!(grid_focus_ring_windows(&layout).is_empty());
}

fn windows_with_visible_focus_ring(layout: &Layout<TestWindow>) -> Vec<usize> {
    let ws = layout.active_workspace().unwrap();
    let mut ids: Vec<usize> = ws
        .tiles()
        .filter(|tile| tile.focus_ring_visible())
        .map(|tile| *tile.window().id())
        .collect();
    ids.sort();
    ids
}

#[test]
fn focus_ring_shrinks_back_on_the_window_that_lost_focus() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::FocusWindow(2),
        Op::CompleteAnimations,
    ]);
    layout.update_render_elements(None);
    assert_eq!(windows_with_visible_focus_ring(&layout), [2]);

    layout.activate_window(&1);
    layout.update_render_elements(None);
    // Window 2 keeps its ring while it shrinks back.
    assert_eq!(windows_with_visible_focus_ring(&layout), [1, 2]);

    check_ops_on_layout(&mut layout, [Op::CompleteAnimations]);
    layout.update_render_elements(None);
    assert_eq!(windows_with_visible_focus_ring(&layout), [1]);
}

#[test]
fn focus_ring_moves_to_floating_and_back() {
    let mut floating = TestWindowParams::new(2);
    floating.is_floating = true;
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow { params: floating },
        Op::CompleteAnimations,
    ]);
    layout.update_render_elements(None);
    assert_eq!(windows_with_visible_focus_ring(&layout), [2]);

    check_ops_on_layout(&mut layout, [Op::FocusTiling]);
    layout.update_render_elements(None);
    assert_eq!(windows_with_visible_focus_ring(&layout), [1, 2]);

    check_ops_on_layout(&mut layout, [Op::CompleteAnimations]);
    layout.update_render_elements(None);
    assert_eq!(windows_with_visible_focus_ring(&layout), [1]);
}

#[test]
fn grid_tab_cells_get_tab_indicators() {
    let mut layout = tabbed_column_and_column_grid();
    check_ops_on_layout(&mut layout, [Op::CompleteAnimations]);
    layout.update_render_elements(None);

    let go = layout.active_workspace().unwrap().grid_overview().unwrap();
    let mut ids: Vec<usize> = go.tab_indicator_windows().copied().collect();
    ids.sort();
    // The two tabs of the tabbed column, not the window in its own column.
    assert_eq!(ids, [1, 2]);

    // Closing the grid hands over to the column's own indicator right away.
    check_ops_on_layout(&mut layout, [Op::ToggleGridOverview]);
    layout.update_render_elements(None);
    let go = layout.active_workspace().unwrap().grid_overview().unwrap();
    assert_eq!(go.tab_indicator_windows().count(), 0);
}

#[test]
fn grid_tab_indicator_folds_away_when_tab_leaves() {
    let mut layout = tabbed_column_and_column_grid();
    check_ops_on_layout(&mut layout, [Op::CompleteAnimations]);
    layout.update_render_elements(None);

    // Window 2 leaves the tabbed column into a column of its own.
    assert!(layout
        .active_workspace_mut()
        .unwrap()
        .set_grid_focus_for_window(&2));
    layout.consume_or_expel_window_left(None);
    layout.verify_invariants();
    layout.update_render_elements(None);

    let go = layout.active_workspace().unwrap().grid_overview().unwrap();
    let mut ids: Vec<usize> = go.tab_indicator_windows().copied().collect();
    ids.sort();
    // Window 2 keeps its indicator while it folds away.
    assert!(ids.contains(&2), "{ids:?}");
    assert!(layout.are_animations_ongoing(None));

    check_ops_on_layout(&mut layout, [Op::CompleteAnimations]);
    layout.update_render_elements(None);
    let go = layout.active_workspace().unwrap().grid_overview().unwrap();
    assert!(!go.tab_indicator_windows().any(|id| *id == 2));
}

fn scrolling_column_ids(layout: &Layout<TestWindow>) -> Vec<Vec<usize>> {
    layout
        .active_workspace()
        .unwrap()
        .scrolling()
        .columns()
        .map(|col| col.tiles().map(|(tile, _)| *tile.window().id()).collect())
        .collect()
}

fn window_render_pos(layout: &Layout<TestWindow>, window: usize) -> Point<f64, Logical> {
    layout
        .active_workspace()
        .unwrap()
        .tiles_with_render_positions()
        .find_map(|(tile, pos, _)| (tile.window().id() == &window).then_some(pos))
        .unwrap()
}

fn wide_625_params(id: usize) -> TestWindowParams {
    let mut params = TestWindowParams::new(id);
    params.bbox = Rectangle::from_size(Size::from((625, 200)));
    params
}

fn wide_columns(count: usize, options: Options) -> Layout<TestWindow> {
    let mut ops = vec![Op::AddOutput(1)];
    for id in 1..=count {
        ops.push(Op::AddWindow {
            params: wide_625_params(id),
        });
        ops.push(Op::Communicate(id));
    }

    check_ops_with_options(options, ops)
}

fn scrolling_view_pos(layout: &Layout<TestWindow>) -> f64 {
    layout.active_workspace().unwrap().scrolling().view_pos()
}

fn grid_window_point(
    layout: &Layout<TestWindow>,
    window: usize,
    x_frac: f64,
    y_frac: f64,
) -> (Output, Point<f64, Logical>, f64) {
    let MonitorSet::Normal { monitors, .. } = &layout.monitor_set else {
        unreachable!()
    };

    let mon = &monitors[0];
    let (ws, geo) = mon
        .workspaces_with_render_geo()
        .find(|(ws, _)| ws.has_window(&window))
        .unwrap();
    let (tile_pos, scale) = ws.grid_window_visual_transform(&window).unwrap();
    let tile_size = ws
        .tiles_with_render_positions()
        .find(|(tile, _, _)| tile.window().id() == &window)
        .map(|(tile, _, _)| tile.tile_size())
        .unwrap();
    let offset = Point::from((tile_size.w * scale * x_frac, tile_size.h * scale * y_frac));

    (mon.output.clone(), geo.loc + tile_pos + offset, scale)
}

fn grid_window_visual_rect(layout: &Layout<TestWindow>, window: usize) -> Rectangle<f64, Logical> {
    let ws = layout.active_workspace().unwrap();
    let (tile_pos, scale) = ws.grid_window_visual_transform(&window).unwrap();
    let tile_size = ws
        .tiles_with_render_positions()
        .find(|(tile, _, _)| tile.window().id() == &window)
        .map(|(tile, _, _)| tile.tile_size())
        .unwrap();

    Rectangle::new(tile_pos, tile_size.upscale(scale))
}

fn grid_entry_target_size(layout: &Layout<TestWindow>, window: usize) -> Size<f64, Logical> {
    layout
        .active_workspace()
        .unwrap()
        .grid_overview()
        .unwrap()
        .layout
        .entries
        .iter()
        .find_map(|(item, info)| (item.window_id() == &window).then_some(info.target_size))
        .unwrap()
}

fn grid_rearrange_anim_value(layout: &Layout<TestWindow>) -> f64 {
    layout
        .active_workspace()
        .unwrap()
        .grid_overview()
        .unwrap()
        .rearrange_anim
        .as_ref()
        .unwrap()
        .value()
}

fn grid_has_rearrange_anim(layout: &Layout<TestWindow>) -> bool {
    layout
        .active_workspace()
        .unwrap()
        .grid_overview()
        .unwrap()
        .rearrange_anim
        .is_some()
}

#[test]
fn grid_move_column_moves_tabbed_column_as_group() {
    let mut layout = tabbed_column_and_column_grid();
    assert!(layout
        .active_workspace_mut()
        .unwrap()
        .set_grid_focus_for_window(&1));

    layout.move_right();
    layout.verify_invariants();

    assert_eq!(scrolling_column_ids(&layout), vec![vec![3], vec![1, 2]]);
    assert!(layout.is_grid_overview_open());
    assert_eq!(layout.grid_focused_window_id(), Some(1));
}

#[test]
fn grid_move_window_down_reorders_focused_tab_only() {
    let mut layout = tabbed_column_and_column_grid();
    assert!(layout
        .active_workspace_mut()
        .unwrap()
        .set_grid_focus_for_window(&1));

    layout.move_down();
    layout.verify_invariants();

    assert_eq!(scrolling_column_ids(&layout), vec![vec![2, 1], vec![3]]);
    assert!(layout.is_grid_overview_open());
    assert_eq!(layout.grid_focused_window_id(), Some(1));
}

#[test]
fn grid_move_window_up_reorders_focused_column_tile() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::ConsumeOrExpelWindowLeft { id: None },
        Op::FocusWindow(1),
        Op::ToggleGridOverview,
    ]);

    layout.focus_down();
    layout.verify_invariants();
    assert_eq!(layout.grid_focused_window_id(), Some(2));

    layout.move_up();
    layout.verify_invariants();

    assert_eq!(scrolling_column_ids(&layout), vec![vec![2, 1]]);
    assert!(layout.is_grid_overview_open());
    assert_eq!(layout.grid_focused_window_id(), Some(2));
}

#[test]
fn grid_move_window_up_preserves_tile_move_animation() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::ConsumeOrExpelWindowLeft { id: None },
        Op::FocusWindow(1),
        Op::ToggleGridOverview,
    ]);

    layout.focus_down();
    layout.verify_invariants();
    let item = layout
        .active_workspace()
        .unwrap()
        .scrolling()
        .grid_item_for_window(&2)
        .unwrap();
    let before_origin = layout
        .active_workspace()
        .unwrap()
        .scrolling()
        .grid_preview_with_stable_origin(&item)
        .unwrap()
        .normal_pos;

    layout.move_up();
    layout.verify_invariants();
    check_ops_on_layout(&mut layout, [Op::AdvanceAnimations { msec_delta: 50 }]);

    let has_vertical_move_animation = layout
        .active_workspace()
        .unwrap()
        .scrolling()
        .columns()
        .flat_map(|col| col.tiles())
        .any(|(tile, _)| tile.render_offset().y.abs() > 0.001);
    assert!(has_vertical_move_animation);

    let item = layout
        .active_workspace()
        .unwrap()
        .scrolling()
        .grid_item_for_window(&2)
        .unwrap();
    let after_origin = layout
        .active_workspace()
        .unwrap()
        .scrolling()
        .grid_preview_with_stable_origin(&item)
        .unwrap()
        .normal_pos;
    approx::assert_abs_diff_eq!(after_origin.x, before_origin.x, epsilon = 0.001);
    approx::assert_abs_diff_eq!(after_origin.y, before_origin.y, epsilon = 0.001);
}

#[test]
fn grid_move_window_within_column_does_not_rearrange_unchanged_entries() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::ConsumeOrExpelWindowLeft { id: None },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::FocusWindow(1),
        Op::ToggleGridOverview,
        Op::CompleteAnimations,
    ]);
    layout.focus_down();
    check_ops_on_layout(&mut layout, [Op::CompleteAnimations]);

    let before_3 = layout
        .active_workspace()
        .unwrap()
        .grid_window_visual_transform(&3)
        .unwrap();

    layout.move_up();
    layout.verify_invariants();

    let go = layout
        .active_workspace()
        .and_then(|ws| ws.grid_overview())
        .unwrap();
    assert!(go.rearrange_anim.is_none());

    check_ops_on_layout(&mut layout, [Op::AdvanceAnimations { msec_delta: 50 }]);
    let after_3 = layout
        .active_workspace()
        .unwrap()
        .grid_window_visual_transform(&3)
        .unwrap();
    approx::assert_abs_diff_eq!(after_3.0.x, before_3.0.x, epsilon = 0.001);
    approx::assert_abs_diff_eq!(after_3.0.y, before_3.0.y, epsilon = 0.001);
    approx::assert_abs_diff_eq!(after_3.1, before_3.1, epsilon = 0.001);
}

#[test]
fn grid_close_top_window_preserves_lower_tile_move_animation() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::ConsumeOrExpelWindowLeft { id: None },
        Op::FocusWindow(1),
        Op::ToggleGridOverview,
    ]);

    assert_eq!(scrolling_column_ids(&layout), vec![vec![1, 2]]);
    assert_eq!(layout.grid_focused_window_id(), Some(1));

    let before = grid_window_visual_rect(&layout, 2);

    check_ops_on_layout(&mut layout, [Op::CloseWindow(1)]);
    layout.verify_invariants();

    assert_eq!(scrolling_column_ids(&layout), vec![vec![2]]);
    let after = grid_window_visual_rect(&layout, 2);

    approx::assert_abs_diff_eq!(after.loc.x, before.loc.x, epsilon = 0.001);
    approx::assert_abs_diff_eq!(after.loc.y, before.loc.y, epsilon = 0.001);
}

#[test]
fn grid_focus_window_down_or_output_navigates_focused_column_tile() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddOutput(2),
        Op::FocusOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::ConsumeOrExpelWindowLeft { id: None },
        Op::FocusWindow(1),
        Op::ToggleGridOverview,
    ]);
    let output = layout
        .outputs()
        .find(|output| output.name() == "output2")
        .cloned()
        .unwrap();

    assert!(layout.is_grid_overview_open());
    assert_eq!(layout.grid_focused_window_id(), Some(1));
    assert_eq!(
        layout
            .active_workspace()
            .and_then(|ws| ws.active_window())
            .map(|window| *window.id()),
        Some(1)
    );

    assert!(!layout.focus_window_down_or_output(&output));
    layout.verify_invariants();

    assert!(layout.is_grid_overview_open());
    assert_eq!(layout.grid_focused_window_id(), Some(2));
    assert_eq!(
        layout
            .active_workspace()
            .and_then(|ws| ws.active_window())
            .map(|window| *window.id()),
        Some(1)
    );

    assert!(!layout.focus_window_up_or_output(&output));
    layout.verify_invariants();

    assert_eq!(layout.grid_focused_window_id(), Some(1));
    assert_eq!(
        layout
            .active_workspace()
            .and_then(|ws| ws.active_window())
            .map(|window| *window.id()),
        Some(1)
    );
}

#[test]
fn grid_focus_window_or_workspace_down_prefers_grid_then_workspace() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::ConsumeOrExpelWindowLeft { id: None },
        Op::FocusWindow(1),
        Op::ToggleGridOverview,
    ]);

    assert!(layout.is_grid_overview_open());
    assert_eq!(layout.grid_focused_window_id(), Some(1));

    layout.focus_window_or_workspace_down();
    layout.verify_invariants();
    assert_eq!(layout.grid_focused_window_id(), Some(2));

    layout.focus_window_or_workspace_up();
    layout.verify_invariants();
    assert_eq!(layout.grid_focused_window_id(), Some(1));
}

#[test]
fn grid_toggle_column_tabbed_display_refreshes_grid_items() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::ConsumeOrExpelWindowLeft { id: None },
        Op::FocusWindow(1),
        Op::ToggleGridOverview,
    ]);

    layout.toggle_column_tabbed_display();
    layout.verify_invariants();

    let ws = layout.active_workspace().unwrap();
    let go = ws.grid_overview().unwrap();
    assert!(go.layout.entries.iter().any(|(item, _)| matches!(
        item,
        super::grid_overview::GridItem::Tab { window_id, .. } if window_id == &1
    )));
    assert!(go.layout.entries.iter().any(|(item, _)| matches!(
        item,
        super::grid_overview::GridItem::Tab { window_id, .. } if window_id == &2
    )));
    assert!(ws.grid_window_visual_transform(&1).is_some());
    assert!(ws.grid_window_visual_transform(&2).is_some());
}

#[test]
fn grid_column_to_tabs_starts_windows_from_previous_visual_positions() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::ConsumeOrExpelWindowLeft { id: None },
        Op::FocusWindow(1),
        Op::ToggleGridOverview,
        Op::CompleteAnimations,
    ]);
    let before_2 = layout
        .active_workspace()
        .unwrap()
        .grid_window_visual_transform(&2)
        .unwrap();

    layout.toggle_column_tabbed_display();
    layout.verify_invariants();

    let after_2 = layout
        .active_workspace()
        .unwrap()
        .grid_window_visual_transform(&2)
        .unwrap();
    approx::assert_abs_diff_eq!(after_2.0.x, before_2.0.x, epsilon = 0.001);
    approx::assert_abs_diff_eq!(after_2.0.y, before_2.0.y, epsilon = 0.001);
    approx::assert_abs_diff_eq!(after_2.1, before_2.1, epsilon = 0.001);
}

#[test]
fn grid_tabs_to_column_starts_windows_from_previous_visual_positions() {
    let mut layout = tabbed_column_and_column_grid();
    assert!(layout
        .active_workspace_mut()
        .unwrap()
        .set_grid_focus_for_window(&1));
    check_ops_on_layout(&mut layout, [Op::CompleteAnimations]);
    let before_1 = layout
        .active_workspace()
        .unwrap()
        .grid_window_visual_transform(&1)
        .unwrap();
    let before_2 = layout
        .active_workspace()
        .unwrap()
        .grid_window_visual_transform(&2)
        .unwrap();

    layout.set_column_display(ColumnDisplay::Normal);
    layout.verify_invariants();

    let after_1 = layout
        .active_workspace()
        .unwrap()
        .grid_window_visual_transform(&1)
        .unwrap();
    let after_2 = layout
        .active_workspace()
        .unwrap()
        .grid_window_visual_transform(&2)
        .unwrap();
    approx::assert_abs_diff_eq!(after_1.0.x, before_1.0.x, epsilon = 0.001);
    approx::assert_abs_diff_eq!(after_1.0.y, before_1.0.y, epsilon = 0.001);
    approx::assert_abs_diff_eq!(after_1.1, before_1.1, epsilon = 0.001);
    approx::assert_abs_diff_eq!(after_2.0.x, before_2.0.x, epsilon = 0.001);
    approx::assert_abs_diff_eq!(after_2.0.y, before_2.0.y, epsilon = 0.001);
    approx::assert_abs_diff_eq!(after_2.1, before_2.1, epsilon = 0.001);
}

#[test]
fn grid_merge_starts_windows_from_previous_visual_positions() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::FocusWindow(1),
        Op::ToggleGridOverview,
        Op::CompleteAnimations,
    ]);
    assert!(layout
        .active_workspace_mut()
        .unwrap()
        .set_grid_focus_for_window(&2));
    let before_1 = layout
        .active_workspace()
        .unwrap()
        .grid_window_visual_transform(&1)
        .unwrap();
    let before_2 = layout
        .active_workspace()
        .unwrap()
        .grid_window_visual_transform(&2)
        .unwrap();

    layout.consume_or_expel_window_left(None);
    layout.verify_invariants();

    let after_1 = layout
        .active_workspace()
        .unwrap()
        .grid_window_visual_transform(&1)
        .unwrap();
    let after_2 = layout
        .active_workspace()
        .unwrap()
        .grid_window_visual_transform(&2)
        .unwrap();
    approx::assert_abs_diff_eq!(after_1.0.x, before_1.0.x, epsilon = 0.001);
    approx::assert_abs_diff_eq!(after_1.0.y, before_1.0.y, epsilon = 0.001);
    approx::assert_abs_diff_eq!(after_1.1, before_1.1, epsilon = 0.001);
    approx::assert_abs_diff_eq!(after_2.0.x, before_2.0.x, epsilon = 0.001);
    approx::assert_abs_diff_eq!(after_2.0.y, before_2.0.y, epsilon = 0.001);
    approx::assert_abs_diff_eq!(after_2.1, before_2.1, epsilon = 0.001);
}

#[test]
fn grid_action_snapshots_visuals_before_activating_grid_focus() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::FocusWindow(1),
        Op::ToggleGridOverview,
        Op::CompleteAnimations,
    ]);
    assert_eq!(
        layout
            .active_workspace()
            .unwrap()
            .active_window()
            .unwrap()
            .id(),
        &1
    );
    assert!(layout
        .active_workspace_mut()
        .unwrap()
        .set_grid_focus_for_window(&2));
    let before_2 = layout
        .active_workspace()
        .unwrap()
        .grid_window_visual_transform(&2)
        .unwrap();

    layout.consume_or_expel_window_left(None);
    layout.verify_invariants();

    let after_2 = layout
        .active_workspace()
        .unwrap()
        .grid_window_visual_transform(&2)
        .unwrap();
    approx::assert_abs_diff_eq!(after_2.0.x, before_2.0.x, epsilon = 0.001);
    approx::assert_abs_diff_eq!(after_2.0.y, before_2.0.y, epsilon = 0.001);
}

#[test]
fn grid_set_column_display_normal_refreshes_grid_items() {
    let mut layout = tabbed_column_and_column_grid();
    assert!(layout
        .active_workspace_mut()
        .unwrap()
        .set_grid_focus_for_window(&1));

    layout.set_column_display(ColumnDisplay::Normal);
    layout.verify_invariants();

    let ws = layout.active_workspace().unwrap();
    let go = ws.grid_overview().unwrap();
    assert!(!go
        .layout
        .entries
        .iter()
        .any(|(item, _)| matches!(item, super::grid_overview::GridItem::Tab { .. })));
    assert!(go.layout.entries.iter().any(|(item, _)| matches!(
        item,
        super::grid_overview::GridItem::Column { window_id, .. } if window_id == &1
    )));
    assert!(ws.grid_window_visual_transform(&1).is_some());
    assert!(ws.grid_window_visual_transform(&2).is_some());
}

#[test]
fn grid_move_window_to_workspace_keeps_source_grid_open() {
    let mut layout = two_column_grid_focused_on_second();
    let source_ws_id = layout.active_workspace().unwrap().id();

    layout.move_to_workspace_down(true);
    layout.verify_invariants();

    let source_ws = layout.find_workspace_by_id(source_ws_id).unwrap().1;
    assert!(source_ws.is_grid_overview_open());
    assert!(!source_ws.has_window(&2));
    assert_eq!(source_ws.grid_focused_window_id(), Some(1));
}

#[test]
fn grid_grab_minimized_window_restores_it() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::MinimizeWindow(1),
        Op::FocusWindow(2),
        Op::ToggleGridOverview,
        Op::CompleteAnimations,
    ]);

    assert!(layout.is_window_minimized(&1));

    // Grab the minimized window at its grid cell.
    let output = layout.outputs().next().unwrap().clone();
    let ws = layout.active_workspace().unwrap();
    let (pos, _scale) = ws.grid_window_visual_transform(&1).unwrap();
    let geo = layout
        .active_monitor_ref()
        .unwrap()
        .workspaces_render_geo()
        .next()
        .unwrap();
    let grab_pos = geo.loc + pos + Point::from((10., 10.));

    assert!(layout.interactive_move_begin(1, &output, grab_pos));
    assert!(!layout.is_window_minimized(&1));
}

#[test]
fn grid_move_column_to_workspace_flies_in_from_source_cells() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::ConsumeOrExpelWindowLeft { id: None },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::FocusWindow(2),
        Op::ToggleGridOverview,
        Op::CompleteAnimations,
    ]);

    let source_ws_id = layout.active_workspace().unwrap().id();
    let starts: Vec<_> = [1, 2]
        .iter()
        .map(|id| {
            layout
                .active_workspace()
                .unwrap()
                .grid_window_visual_transform(id)
                .unwrap()
        })
        .collect();

    layout.move_column_to_workspace(1, true);
    layout.verify_invariants();

    // Each window of the moved column gets a transition start at its source grid cell in the
    // destination workspace's grid.
    let target_ws = layout.active_workspace().unwrap();
    assert_ne!(target_ws.id(), source_ws_id);
    assert!(target_ws.is_grid_overview_open());

    let go = target_ws.grid_overview().unwrap();
    let geo: Vec<_> = layout
        .active_monitor_ref()
        .unwrap()
        .workspaces_render_geo()
        .collect();
    let ws_height_with_gap = geo[1].loc.y - geo[0].loc.y;

    for (id, (start_pos, start_scale)) in [1, 2].iter().zip(starts) {
        let (_, trans_pos, trans_scale) = go
            .window_transition_starts
            .iter()
            .find(|(wid, _, _)| wid == id)
            .unwrap_or_else(|| panic!("window {id} should fly in from its source grid cell"));

        approx::assert_abs_diff_eq!(*trans_scale, start_scale, epsilon = 0.001);
        approx::assert_abs_diff_eq!(trans_pos.x, start_pos.x, epsilon = 0.001);
        approx::assert_abs_diff_eq!(
            trans_pos.y,
            start_pos.y - ws_height_with_gap,
            epsilon = 0.001
        );
    }

    assert!(go.flying_in_windows.iter().any(|id| [1, 2].contains(id)));
}

#[test]
fn grid_move_window_to_workspace_flies_in_from_source_cell() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::FocusWindow(2),
        Op::ToggleGridOverview,
    ]);

    let source_ws_id = layout.active_workspace().unwrap().id();
    let (start_pos, start_scale) = layout
        .active_workspace()
        .unwrap()
        .grid_window_visual_transform(&2)
        .unwrap();

    layout.move_to_workspace(None, 1, ActivateWindow::Yes);
    layout.verify_invariants();

    // The destination workspace's grid renders the fly-in: the moved window gets a transition
    // start at its source grid cell, one workspace height with gap above the destination.
    let target_ws = layout.active_workspace().unwrap();
    assert_ne!(target_ws.id(), source_ws_id);
    assert!(target_ws.is_grid_overview_open());

    let go = target_ws.grid_overview().unwrap();
    let (_, trans_pos, trans_scale) = go
        .window_transition_starts
        .iter()
        .find(|(id, _, _)| *id == 2)
        .expect("moved window should fly in from its source grid cell");

    assert!(go.rearrange_anim.is_some());
    assert_eq!(go.flying_in_windows, [2]);

    let geo: Vec<_> = layout
        .active_monitor_ref()
        .unwrap()
        .workspaces_render_geo()
        .collect();
    let ws_height_with_gap = geo[1].loc.y - geo[0].loc.y;

    approx::assert_abs_diff_eq!(*trans_scale, start_scale, epsilon = 0.001);
    approx::assert_abs_diff_eq!(trans_pos.x, start_pos.x, epsilon = 0.001);
    approx::assert_abs_diff_eq!(
        trans_pos.y,
        start_pos.y - ws_height_with_gap,
        epsilon = 0.001
    );

    // The grid renders the fly, so the tile itself has no between-workspaces movement.
    let tile = target_ws
        .scrolling()
        .columns()
        .flat_map(|col| col.tiles())
        .find(|(tile, _)| tile.window().id() == &2)
        .map(|(tile, _)| tile)
        .unwrap();
    assert!(!tile.is_moving_between_workspaces());
}

#[test]
fn grid_closing_minimized_window_flies_back_while_fading() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::MinimizeWindow(1),
        Op::FocusWindow(2),
        Op::ToggleGridOverview,
        Op::CompleteAnimations,
    ]);

    let ws = layout.active_workspace().unwrap();
    let go = ws.grid_overview().unwrap();
    let item = ws.scrolling().grid_item_for_window(&1).unwrap();
    let (_, info) = go
        .layout
        .entries
        .iter()
        .find(|(entry, _)| entry == &item)
        .unwrap();
    let (open_pos, _) = ws.grid_item_render_visual_transform(go, &item, info);
    let normal_pos = ws.scrolling().grid_preview(&item).unwrap().normal_pos;

    layout.close_grid_overview();
    layout.clock.set_unadjusted(Duration::from_millis(80));
    layout.advance_animations();

    let (_, ws) = layout
        .active_monitor_ref()
        .unwrap()
        .workspaces
        .iter()
        .enumerate()
        .find(|(_, ws)| ws.has_window(&1))
        .unwrap();
    let go = ws.grid_overview().unwrap();
    let item = ws.scrolling().grid_item_for_window(&1).unwrap();
    let (_, info) = go
        .layout
        .entries
        .iter()
        .find(|(entry, _)| entry == &item)
        .unwrap();
    let (close_pos, _) = ws.grid_item_render_visual_transform(go, &item, info);

    let dist = |a: Point<f64, Logical>, b: Point<f64, Logical>| {
        ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
    };

    // The minimized window is on its way from the grid cell back to its normal position
    // (rather than being pinned to the grid cell while only alpha changes).
    assert!(dist(close_pos, normal_pos) < dist(open_pos, normal_pos));
    assert!(dist(close_pos, info.target_pos) > 0.5);

    // Its alpha is fading out at the same time.
    let tile = ws
        .scrolling()
        .tiles()
        .find(|tile| tile.window().id() == &1)
        .unwrap();
    let alpha = tile.alpha_animation.as_ref().unwrap();
    assert_eq!(alpha.anim.to(), 0.);
    assert!(alpha.anim.value() < 1.);
}

#[test]
fn grid_reopen_mid_close_continues_from_current_visual() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::ToggleGridOverview,
        Op::CompleteAnimations,
    ]);

    layout.close_grid_overview();
    layout.clock.set_unadjusted(Duration::from_millis(50));
    layout.advance_animations();

    let visual = |layout: &Layout<TestWindow>| {
        let ws = layout.active_workspace().unwrap();
        let go = ws.grid_overview().unwrap();
        let item = ws.scrolling().grid_item_for_window(&1).unwrap();
        let (_, info) = go
            .layout
            .entries
            .iter()
            .find(|(entry, _)| entry == &item)
            .unwrap();
        ws.grid_item_render_visual_transform(go, &item, info)
    };

    let (before_pos, before_scale) = visual(&layout);

    layout.toggle_grid_overview();

    let (after_pos, after_scale) = visual(&layout);
    approx::assert_abs_diff_eq!(after_pos.x, before_pos.x, epsilon = 1.5);
    approx::assert_abs_diff_eq!(after_pos.y, before_pos.y, epsilon = 1.5);
    approx::assert_abs_diff_eq!(after_scale, before_scale, epsilon = 0.01);
}

#[test]
fn grid_click_minimized_window_does_not_jump_to_focused_scale() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::MinimizeWindow(1),
        Op::FocusWindow(2),
        Op::ToggleGridOverview,
        Op::CompleteAnimations,
    ]);

    let ws = layout.active_workspace().unwrap();
    let go = ws.grid_overview().unwrap();
    let item = ws.scrolling().grid_item_for_window(&1).unwrap();
    let (_, info) = go
        .layout
        .entries
        .iter()
        .find(|(entry, _)| entry == &item)
        .unwrap();
    let (_, pre_scale) = ws.grid_item_render_visual_transform(go, &item, info);

    assert!(layout.confirm_grid_selection_for_window(&1));

    let (_, ws) = layout
        .active_monitor_ref()
        .unwrap()
        .workspaces
        .iter()
        .enumerate()
        .find(|(_, ws)| ws.has_window(&1))
        .unwrap();
    let go = ws.grid_overview().unwrap();
    let item = ws.scrolling().grid_item_for_window(&1).unwrap();
    let (_, info) = go
        .layout
        .entries
        .iter()
        .find(|(entry, _)| entry == &item)
        .unwrap();
    let (_, close_start_scale) = ws.grid_item_render_visual_transform(go, &item, info);

    assert!(!layout.is_window_minimized(&1));
    approx::assert_abs_diff_eq!(close_start_scale, pre_scale, epsilon = 0.001);
}

#[test]
fn grid_cross_workspace_move_uses_workspace_switch_animation_for_fly_in() {
    use niri_config::animations::{Curve, EasingParams, Kind};

    let mut options = Options::default();
    options.animations.workspace_switch.0.kind = Kind::Easing(EasingParams {
        duration_ms: 1000,
        curve: Curve::Linear,
    });
    options.animations.window_movement.0.kind = Kind::Easing(EasingParams {
        duration_ms: 1,
        curve: Curve::Linear,
    });

    let mut layout = check_ops_with_options(
        options,
        [
            Op::AddOutput(1),
            Op::AddWindow {
                params: TestWindowParams::new(1),
            },
            Op::AddWindow {
                params: TestWindowParams::new(2),
            },
            Op::FocusWindow(2),
            Op::ToggleGridOverview,
            Op::CompleteAnimations,
        ],
    );

    layout.move_to_workspace(None, 1, ActivateWindow::Yes);
    layout.clock.set_unadjusted(Duration::from_millis(100));
    layout.advance_animations();

    let (_, ws) = layout
        .active_monitor_ref()
        .unwrap()
        .workspaces
        .iter()
        .enumerate()
        .find(|(_, ws)| ws.has_window(&2))
        .unwrap();
    let go = ws.grid_overview().unwrap();
    let value = go.rearrange_anim.as_ref().unwrap().value();
    assert!(value > 0.05);
    assert!(value < 0.5, "value {value}");
}

#[test]
fn grid_move_column_reversed_midflight_keeps_visual_continuity() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::ConsumeOrExpelWindowLeft { id: None },
        Op::FocusWindow(2),
        Op::ToggleGridOverview,
        Op::CompleteAnimations,
    ]);

    layout.move_column_to_workspace_down(true);
    layout.clock.set_unadjusted(Duration::from_millis(100));
    layout.advance_animations();

    let visual = |layout: &Layout<TestWindow>, id: usize| {
        let mon = layout.active_monitor_ref().unwrap();
        let geo: Vec<_> = mon.workspaces_render_geo().collect();
        let (ws_idx, ws) = mon
            .workspaces
            .iter()
            .enumerate()
            .find(|(_, ws)| ws.has_window(&id))
            .unwrap();
        let (pos, scale) = ws.grid_window_visual_transform(&id).unwrap();
        (geo[ws_idx].loc + pos, scale)
    };

    let before: Vec<_> = [1, 2].iter().map(|id| visual(&layout, *id)).collect();

    layout.move_column_to_workspace_up(true);

    for (id, (before_pos, before_scale)) in [1, 2].iter().zip(&before) {
        let (after_pos, after_scale) = visual(&layout, *id);
        approx::assert_abs_diff_eq!(after_pos.x, before_pos.x, epsilon = 0.5);
        approx::assert_abs_diff_eq!(after_pos.y, before_pos.y, epsilon = 0.5);
        approx::assert_abs_diff_eq!(after_scale, before_scale, epsilon = 0.001);
    }

    let (_, ws) = layout
        .active_monitor_ref()
        .unwrap()
        .workspaces
        .iter()
        .enumerate()
        .find(|(_, ws)| ws.has_window(&1))
        .unwrap();
    let go = ws.grid_overview().unwrap();
    assert!(go.rearrange_anim.is_some());
    assert!(go.flying_in_windows.contains(&1));
    assert!(go.flying_in_windows.contains(&2));
}

#[test]
fn grid_move_window_reversed_midflight_keeps_visual_continuity() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
    ]);
    layout.move_to_workspace(Some(&3), 1, ActivateWindow::No);
    layout.activate_window(&2);
    layout.toggle_grid_overview();
    layout.clock.set_complete_instantly(true);
    layout.advance_animations();
    layout.clock.set_complete_instantly(false);

    layout.move_to_workspace_down(true);
    layout.clock.set_unadjusted(Duration::from_millis(100));
    layout.advance_animations();

    let visual = |layout: &Layout<TestWindow>| {
        let mon = layout.active_monitor_ref().unwrap();
        let geo: Vec<_> = mon.workspaces_render_geo().collect();
        let (ws_idx, ws) = mon
            .workspaces
            .iter()
            .enumerate()
            .find(|(_, ws)| ws.has_window(&2))
            .unwrap();
        let (pos, scale) = ws.grid_window_visual_transform(&2).unwrap();
        (geo[ws_idx].loc + pos, scale)
    };

    let (before_pos, before_scale) = visual(&layout);

    layout.move_to_workspace_up(true);

    let (after_pos, after_scale) = visual(&layout);
    approx::assert_abs_diff_eq!(after_pos.x, before_pos.x, epsilon = 0.5);
    approx::assert_abs_diff_eq!(after_pos.y, before_pos.y, epsilon = 0.5);
    approx::assert_abs_diff_eq!(after_scale, before_scale, epsilon = 0.001);

    let (_, ws) = layout
        .active_monitor_ref()
        .unwrap()
        .workspaces
        .iter()
        .enumerate()
        .find(|(_, ws)| ws.has_window(&2))
        .unwrap();
    let go = ws.grid_overview().unwrap();
    assert!(go.rearrange_anim.is_some());
    assert!(go.flying_in_windows.contains(&2));
}

#[test]
fn grid_move_column_triple_reversal_keeps_visual_continuity() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::ConsumeOrExpelWindowLeft { id: None },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::FocusWindow(2),
        Op::ToggleGridOverview,
        Op::CompleteAnimations,
    ]);

    // Focus a different column before moving, like pressing grid focus keys first.
    layout
        .active_workspace_mut()
        .unwrap()
        .set_grid_focus_for_window(&3);

    let visual = |layout: &Layout<TestWindow>, id: usize| {
        let mon = layout.active_monitor_ref().unwrap();
        let geo: Vec<_> = mon.workspaces_render_geo().collect();
        let (ws_idx, ws) = mon
            .workspaces
            .iter()
            .enumerate()
            .find(|(_, ws)| ws.has_window(&id))
            .unwrap();
        let (pos, scale) = ws.grid_window_visual_transform(&id).unwrap();
        (geo[ws_idx].loc + pos, scale)
    };

    layout.move_column_to_workspace_down(true);
    layout.clock.set_unadjusted(Duration::from_millis(100));
    layout.advance_animations();

    layout.move_column_to_workspace_up(true);
    layout.clock.set_unadjusted(Duration::from_millis(120));
    layout.advance_animations();
    let before = visual(&layout, 3);

    layout.move_column_to_workspace_down(true);
    let (after_pos, after_scale) = visual(&layout, 3);

    approx::assert_abs_diff_eq!(after_pos.x, before.0.x, epsilon = 0.5);
    approx::assert_abs_diff_eq!(after_pos.y, before.0.y, epsilon = 0.5);
    approx::assert_abs_diff_eq!(after_scale, before.1, epsilon = 0.001);
}

#[test]
fn grid_focus_moves_real_focus_during_close_animation() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::AddWindow {
            params: TestWindowParams::new(4),
        },
        Op::AddWindow {
            params: TestWindowParams::new(5),
        },
        Op::FocusWindow(1),
        Op::ToggleGridOverview,
        Op::CompleteAnimations,
    ]);

    layout.close_grid_overview();
    layout.clock.set_unadjusted(Duration::from_millis(20));
    layout.advance_animations();

    let before = layout.focus().map(|w| *w.id());
    layout.focus_right();

    assert_ne!(layout.focus().map(|w| *w.id()), before);
    assert_eq!(
        layout.grid_focused_window_id(),
        layout.focus().map(|w| *w.id())
    );
}

#[test]
fn grid_focus_navigation_works_during_close_animation() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::FocusWindow(1),
        Op::ToggleGridOverview,
        Op::CompleteAnimations,
    ]);

    layout.close_grid_overview();
    layout.clock.set_unadjusted(Duration::from_millis(20));
    layout.advance_animations();

    let before = layout.grid_focused_window_id();
    layout.focus_right();
    let after = layout.grid_focused_window_id();

    assert_ne!(before, after);
    assert!(layout
        .active_workspace()
        .unwrap()
        .grid_overview()
        .is_some_and(|go| !go.open));
}

#[test]
fn grid_noop_action_preserves_ongoing_window_transitions() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::FocusWindow(2),
        Op::ToggleGridOverview,
        Op::CompleteAnimations,
    ]);

    layout.move_to_workspace_down(true);
    layout.clock.set_unadjusted(Duration::from_millis(80));
    layout.advance_animations();

    let mon = layout.active_monitor_ref().unwrap();
    let (ws_idx, _) = mon
        .workspaces
        .iter()
        .enumerate()
        .find(|(_, ws)| ws.has_window(&2))
        .unwrap();
    let transitions = mon.workspaces[ws_idx]
        .grid_overview()
        .unwrap()
        .window_transition_starts
        .clone();
    let geo: Vec<_> = mon.workspaces_render_geo().collect();
    let (pos, scale) = mon.workspaces[ws_idx]
        .grid_window_visual_transform(&2)
        .unwrap();
    let before = (geo[ws_idx].loc + pos, scale);
    assert!(!transitions.is_empty());

    // A no-op grid action while the fly-in is still running must not clear its transition.
    layout.move_left();

    let mon = layout.active_monitor_ref().unwrap();
    let (ws_idx, _) = mon
        .workspaces
        .iter()
        .enumerate()
        .find(|(_, ws)| ws.has_window(&2))
        .unwrap();
    let after_transitions = mon.workspaces[ws_idx]
        .grid_overview()
        .unwrap()
        .window_transition_starts
        .clone();
    let geo: Vec<_> = mon.workspaces_render_geo().collect();
    let (pos, scale) = mon.workspaces[ws_idx]
        .grid_window_visual_transform(&2)
        .unwrap();
    let after = (geo[ws_idx].loc + pos, scale);

    assert_eq!(after_transitions, transitions);
    approx::assert_abs_diff_eq!(after.0.x, before.0.x, epsilon = 0.5);
    approx::assert_abs_diff_eq!(after.0.y, before.0.y, epsilon = 0.5);
    approx::assert_abs_diff_eq!(after.1, before.1, epsilon = 0.001);
}

#[test]
fn grid_move_column_at_edge_repeat_does_not_jump_midflight() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::ConsumeOrExpelWindowLeft { id: None },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::FocusWindow(2),
        Op::ToggleGridOverview,
        Op::CompleteAnimations,
    ]);
    layout.move_column_to_workspace_down(true);
    layout.clock.set_complete_instantly(true);
    layout.advance_animations();
    layout.clock.set_complete_instantly(false);
    layout.move_column_to_workspace_down(true);
    layout.clock.set_complete_instantly(true);
    layout.advance_animations();
    layout.clock.set_complete_instantly(false);

    layout.move_column_to_workspace_up(true);
    layout.clock.set_unadjusted(Duration::from_millis(100));
    layout.advance_animations();

    let visual = |layout: &Layout<TestWindow>, id: usize| {
        let mon = layout.active_monitor_ref().unwrap();
        let geo: Vec<_> = mon.workspaces_render_geo().collect();
        let (ws_idx, ws) = mon
            .workspaces
            .iter()
            .enumerate()
            .find(|(_, ws)| ws.has_window(&id))
            .unwrap();
        let (pos, scale) = ws.grid_window_visual_transform(&id).unwrap();
        (geo[ws_idx].loc + pos, scale)
    };

    let before: Vec<_> = [1, 2].iter().map(|id| visual(&layout, *id)).collect();

    // Already at the top workspace: this move is a no-op and must not restart the animation.
    layout.move_column_to_workspace_up(true);

    for (id, (before_pos, before_scale)) in [1, 2].iter().zip(&before) {
        let (after_pos, after_scale) = visual(&layout, *id);
        approx::assert_abs_diff_eq!(after_pos.x, before_pos.x, epsilon = 0.5);
        approx::assert_abs_diff_eq!(after_pos.y, before_pos.y, epsilon = 0.5);
        approx::assert_abs_diff_eq!(after_scale, before_scale, epsilon = 0.001);
    }
}

#[test]
fn grid_move_window_at_edge_repeat_does_not_jump_midflight() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
    ]);
    layout.toggle_grid_overview();
    layout.clock.set_complete_instantly(true);
    layout.advance_animations();
    layout.clock.set_complete_instantly(false);

    layout.move_to_workspace_down(true);
    layout.clock.set_complete_instantly(true);
    layout.advance_animations();
    layout.clock.set_complete_instantly(false);

    layout.move_to_workspace_up(true);
    layout.clock.set_unadjusted(Duration::from_millis(100));
    layout.advance_animations();

    let visual = |layout: &Layout<TestWindow>| {
        let mon = layout.active_monitor_ref().unwrap();
        let geo: Vec<_> = mon.workspaces_render_geo().collect();
        let (ws_idx, ws) = mon
            .workspaces
            .iter()
            .enumerate()
            .find(|(_, ws)| ws.has_window(&1))
            .unwrap();
        let (pos, scale) = ws.grid_window_visual_transform(&1).unwrap();
        (geo[ws_idx].loc + pos, scale)
    };

    let (before_pos, before_scale) = visual(&layout);

    // Already at the top workspace: this move is a no-op and must not restart the animation.
    layout.move_to_workspace_up(true);

    let (after_pos, after_scale) = visual(&layout);
    approx::assert_abs_diff_eq!(after_pos.x, before_pos.x, epsilon = 0.5);
    approx::assert_abs_diff_eq!(after_pos.y, before_pos.y, epsilon = 0.5);
    approx::assert_abs_diff_eq!(after_scale, before_scale, epsilon = 0.001);
}

#[test]
fn grid_move_window_reversed_midflight_keeps_visual_continuity_from_empty_source() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
    ]);
    layout.toggle_grid_overview();
    layout.clock.set_complete_instantly(true);
    layout.advance_animations();
    layout.clock.set_complete_instantly(false);

    layout.move_to_workspace_down(true);
    layout.clock.set_unadjusted(Duration::from_millis(100));
    layout.advance_animations();

    let visual = |layout: &Layout<TestWindow>| {
        let mon = layout.active_monitor_ref().unwrap();
        let geo: Vec<_> = mon.workspaces_render_geo().collect();
        let (ws_idx, ws) = mon
            .workspaces
            .iter()
            .enumerate()
            .find(|(_, ws)| ws.has_window(&1))
            .unwrap();
        let (pos, scale) = ws.grid_window_visual_transform(&1).unwrap();
        (geo[ws_idx].loc + pos, scale)
    };

    let (before_pos, before_scale) = visual(&layout);

    layout.move_to_workspace_up(true);

    let (after_pos, after_scale) = visual(&layout);
    approx::assert_abs_diff_eq!(after_pos.x, before_pos.x, epsilon = 0.5);
    approx::assert_abs_diff_eq!(after_pos.y, before_pos.y, epsilon = 0.5);
    approx::assert_abs_diff_eq!(after_scale, before_scale, epsilon = 0.001);
}

#[test]
fn grid_move_window_down_twice_midflight_keeps_visual_continuity() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
    ]);
    layout.move_to_workspace(Some(&3), 1, ActivateWindow::No);
    layout.activate_window(&2);
    layout.toggle_grid_overview();
    layout.clock.set_complete_instantly(true);
    layout.advance_animations();
    layout.clock.set_complete_instantly(false);

    layout.move_to_workspace_down(true);
    layout.clock.set_unadjusted(Duration::from_millis(100));
    layout.advance_animations();

    let visual = |layout: &Layout<TestWindow>| {
        let mon = layout.active_monitor_ref().unwrap();
        let geo: Vec<_> = mon.workspaces_render_geo().collect();
        let (ws_idx, ws) = mon
            .workspaces
            .iter()
            .enumerate()
            .find(|(_, ws)| ws.has_window(&2))
            .unwrap();
        let (pos, scale) = ws.grid_window_visual_transform(&2).unwrap();
        (geo[ws_idx].loc + pos, scale)
    };

    let (before_pos, before_scale) = visual(&layout);

    layout.move_to_workspace_down(true);

    let (after_pos, after_scale) = visual(&layout);
    approx::assert_abs_diff_eq!(after_pos.x, before_pos.x, epsilon = 0.5);
    approx::assert_abs_diff_eq!(after_pos.y, before_pos.y, epsilon = 0.5);
    approx::assert_abs_diff_eq!(after_scale, before_scale, epsilon = 0.001);

    let (_, ws) = layout
        .active_monitor_ref()
        .unwrap()
        .workspaces
        .iter()
        .enumerate()
        .find(|(_, ws)| ws.has_window(&2))
        .unwrap();
    let go = ws.grid_overview().unwrap();
    assert!(go.rearrange_anim.is_some());
    assert!(go.flying_in_windows.contains(&2));
}

fn grid_layout_with_occupied_workspace_below() -> Layout<TestWindow> {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(10),
        },
    ]);
    layout.move_to_workspace(None, 1, ActivateWindow::Yes);
    layout.switch_workspace(0);
    check_ops_on_layout(
        &mut layout,
        [
            Op::AddWindow {
                params: TestWindowParams::new(1),
            },
            Op::AddWindow {
                params: TestWindowParams::new(2),
            },
            Op::ToggleGridOverview,
        ],
    );
    layout
}

fn grid_layout_with_occupied_workspace_above() -> Layout<TestWindow> {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(10),
        },
    ]);
    layout.move_to_workspace(None, 1, ActivateWindow::Yes);
    check_ops_on_layout(
        &mut layout,
        [
            Op::AddWindow {
                params: TestWindowParams::new(1),
            },
            Op::AddWindow {
                params: TestWindowParams::new(2),
            },
            Op::ToggleGridOverview,
        ],
    );
    layout
}

#[test]
fn grid_move_column_down_to_occupied_workspace_focuses_moved_window() {
    let mut layout = grid_layout_with_occupied_workspace_below();

    layout.move_column_to_workspace_down(true);
    layout.verify_invariants();

    assert_eq!(layout.focus().map(|window| *window.id()), Some(2));
    assert_eq!(layout.grid_focused_window_id(), Some(2));
}

#[test]
fn grid_move_column_up_to_occupied_workspace_focuses_moved_window() {
    let mut layout = grid_layout_with_occupied_workspace_above();

    layout.move_column_to_workspace_up(true);
    layout.verify_invariants();

    assert_eq!(layout.focus().map(|window| *window.id()), Some(2));
    assert_eq!(layout.grid_focused_window_id(), Some(2));
}

#[test]
fn grid_move_column_to_occupied_workspace_by_index_focuses_moved_window() {
    let mut layout = grid_layout_with_occupied_workspace_below();

    layout.move_column_to_workspace(1, true);
    layout.verify_invariants();

    assert_eq!(layout.focus().map(|window| *window.id()), Some(2));
    assert_eq!(layout.grid_focused_window_id(), Some(2));
}

#[test]
fn grid_move_column_to_occupied_output_focuses_moved_window() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddOutput(2),
        Op::FocusOutput(2),
        Op::AddWindow {
            params: TestWindowParams::new(10),
        },
        Op::FocusOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::ToggleGridOverview,
    ]);
    let output = layout
        .outputs()
        .find(|output| output.name() == "output2")
        .cloned()
        .unwrap();

    layout.move_column_to_output(&output, None, true);
    layout.verify_invariants();

    assert_eq!(layout.focus().map(|window| *window.id()), Some(2));
    assert_eq!(layout.grid_focused_window_id(), Some(2));
}

#[test]
fn grid_window_scope_is_limited_to_open_grid_workspace() {
    let options = Options {
        grid_overview: niri_config::GridOverview {
            grid_all_monitors: false,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut layout = check_ops_with_options(
        options,
        [
            Op::AddOutput(1),
            Op::AddOutput(2),
            Op::FocusOutput(1),
            Op::AddWindow {
                params: TestWindowParams::new(1),
            },
            Op::FocusOutput(2),
            Op::AddWindow {
                params: TestWindowParams::new(2),
            },
            Op::FocusOutput(1),
            Op::ToggleGridOverview,
        ],
    );

    assert!(layout.window_is_in_open_grid_overview(&1));
    assert!(!layout.window_is_in_open_grid_overview(&2));
    assert!(!layout.confirm_grid_selection_for_window(&2));
    assert!(layout.is_grid_overview_open());
}

#[test]
fn grid_drop_position_uses_visual_cell_edges() {
    let mut layout = tabbed_column_and_column_grid();
    assert!(layout
        .active_workspace_mut()
        .unwrap()
        .set_grid_focus_for_window(&1));
    check_ops_on_layout(&mut layout, [Op::CompleteAnimations]);

    let ws = layout.active_workspace().unwrap();
    let (_, tab_middle, _) = grid_window_point(&layout, 1, 0.5, 0.25);
    let (_, tab_left_edge, _) = grid_window_point(&layout, 1, 0.1, 0.5);
    let (_, tab_right_edge, _) = grid_window_point(&layout, 1, 0.9, 0.5);
    let (_, column_middle, _) = grid_window_point(&layout, 3, 0.5, 0.25);
    let (_, column_left_edge, _) = grid_window_point(&layout, 3, 0.1, 0.5);
    let (_, column_right_edge, _) = grid_window_point(&layout, 3, 0.9, 0.5);

    assert_eq!(
        ws.grid_insert_position(tab_middle),
        Some(InsertPosition::InColumn(0, 0))
    );
    assert_eq!(
        ws.grid_insert_position(tab_left_edge),
        Some(InsertPosition::NewColumn(0))
    );
    assert_eq!(
        ws.grid_insert_position(tab_right_edge),
        Some(InsertPosition::NewColumn(1))
    );
    assert_eq!(
        ws.grid_insert_position(column_middle),
        Some(InsertPosition::InColumn(1, 0))
    );
    assert_eq!(
        ws.grid_insert_position(column_left_edge),
        Some(InsertPosition::NewColumn(1))
    );
    assert_eq!(
        ws.grid_insert_position(column_right_edge),
        Some(InsertPosition::NewColumn(2))
    );
}

#[test]
fn grid_insert_hint_area_uses_grid_visual_coordinates() {
    let mut layout = tabbed_column_and_column_grid();
    assert!(layout
        .active_workspace_mut()
        .unwrap()
        .set_grid_focus_for_window(&1));
    check_ops_on_layout(&mut layout, [Op::CompleteAnimations]);

    let ws = layout.active_workspace().unwrap();
    let (_, tab_left_edge, _) = grid_window_point(&layout, 1, 0.1, 0.5);
    let (_, tab_right_edge, _) = grid_window_point(&layout, 1, 0.9, 0.5);
    let (_, column_left_edge, _) = grid_window_point(&layout, 3, 0.1, 0.5);
    let tab_rect = grid_window_visual_rect(&layout, 1);
    let tab_group_right_rect = grid_window_visual_rect(&layout, 2);
    let column_rect = grid_window_visual_rect(&layout, 3);

    let (position, area) = ws
        .grid_insert_position_and_hint_area(tab_left_edge)
        .unwrap();
    assert_eq!(position, InsertPosition::NewColumn(0));
    assert!(area.loc.x + area.size.w <= tab_rect.loc.x);

    let (position, area) = ws
        .grid_insert_position_and_hint_area(tab_right_edge)
        .unwrap();
    assert_eq!(position, InsertPosition::NewColumn(1));
    assert!(area.loc.x < tab_group_right_rect.loc.x + tab_group_right_rect.size.w);
    assert!(column_rect.loc.x < area.loc.x + area.size.w);

    let gap_point = Point::from((
        (tab_right_edge.x + column_left_edge.x) / 2.,
        tab_right_edge.y,
    ));
    assert!(ws.grid_insert_position_and_hint_area(gap_point).is_some());
}

#[test]
fn grid_drop_far_from_every_cell_goes_beside_the_nearest_one() {
    // Wide padding, to have room for points far from every cell.
    let options = Options {
        grid_overview: niri_config::GridOverview {
            padding: niri_config::GridOverviewPadding::uniform(300.),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut ops = vec![Op::AddOutput(1)];
    for id in 1..=3 {
        ops.push(Op::AddWindow {
            params: TestWindowParams::new(id),
        });
    }
    ops.extend([
        Op::FocusWindow(1),
        Op::ToggleGridOverview,
        Op::CompleteAnimations,
    ]);
    let mut layout = check_ops_with_options(options, ops);

    let first = grid_window_visual_rect(&layout, 1);
    let last = grid_window_visual_rect(&layout, 3);
    let ws = layout.active_workspace().unwrap();

    // Points in the padding around the grid, far from every cell. These used to have no insert
    // position, and a drop there fell back to an arbitrary column.
    let far_left = Point::from((first.loc.x - 150., first.loc.y + first.size.h / 2.));
    let far_right = Point::from((
        last.loc.x + last.size.w + 150.,
        last.loc.y + last.size.h / 2.,
    ));
    assert!(far_left.x > 0.);
    let (position, _) = ws.grid_insert_position_and_hint_area(far_left).unwrap();
    assert_eq!(position, InsertPosition::NewColumn(0));
    let (position, _) = ws.grid_insert_position_and_hint_area(far_right).unwrap();
    assert_eq!(position, InsertPosition::NewColumn(3));

    // Dropping window 1 there makes it the last column.
    let (output, start, _) = grid_window_point(&layout, 1, 0.5, 0.5);
    assert!(layout.interactive_move_begin(1, &output, start));
    assert!(layout.interactive_move_update(&1, Point::from((300., 0.)), output.clone(), far_right));
    layout.interactive_move_end(&1);
    layout.verify_invariants();
    assert_eq!(
        scrolling_column_ids(&layout),
        vec![vec![2], vec![3], vec![1]]
    );
}

#[test]
fn grid_new_column_insert_hint_does_not_span_wrapped_rows() {
    let mut ops = vec![Op::AddOutput(1)];
    for id in 1..=8 {
        let mut params = TestWindowParams::new(id);
        params.bbox = Rectangle::from_size(Size::from((800, 500)));
        ops.push(Op::AddWindow { params });
    }
    for id in 1..=8 {
        ops.push(Op::Communicate(id));
    }
    ops.push(Op::ToggleGridOverview);
    ops.push(Op::CompleteAnimations);
    let layout = check_ops(ops);

    let rects: Vec<_> = (1..=8)
        .map(|id| (id, grid_window_visual_rect(&layout, id)))
        .collect();
    let first_row_y = rects[0].1.loc.y;
    let (source_id, _) = rects
        .iter()
        .take_while(|(_, rect)| (rect.loc.y - first_row_y).abs() < 0.001)
        .last()
        .unwrap();

    let ws = layout.active_workspace().unwrap();
    let (_, right_edge, _) = grid_window_point(&layout, *source_id, 0.9, 0.5);
    let (position, area) = ws.grid_insert_position_and_hint_area(right_edge).unwrap();
    let source_rect = grid_window_visual_rect(&layout, *source_id);
    assert_eq!(position, InsertPosition::NewColumn(*source_id));
    assert!(area.loc.y >= source_rect.loc.y);
    assert!(area.loc.y + area.size.h <= source_rect.loc.y + source_rect.size.h);
}

#[test]
fn grid_interactive_move_keeps_visual_scale_and_can_merge() {
    let mut layout = large_grid_layout();
    assert!(layout
        .active_workspace_mut()
        .unwrap()
        .set_grid_focus_for_window(&1));
    check_ops_on_layout(&mut layout, [Op::CompleteAnimations]);

    let (output, start, visual_scale) = grid_window_point(&layout, 3, 0.5, 0.5);
    let (_, target, _) = grid_window_point(&layout, 1, 0.5, 0.25);

    assert!(visual_scale < 1.);
    assert!(layout.interactive_move_begin(3, &output, start));
    assert!(layout.interactive_move_update(&3, Point::from((300., 0.)), output.clone(), target));

    let Some(InteractiveMoveState::Moving(move_)) = &layout.interactive_move else {
        panic!("expected an interactive move");
    };
    approx::assert_abs_diff_eq!(move_.visual_scale, visual_scale, epsilon = 0.001);
    approx::assert_abs_diff_eq!(move_.total_scale(1.), visual_scale, epsilon = 0.001);

    // While the grab is ongoing, the focus belongs to the grabbed window itself, so that the
    // focus ring doesn't move to some other cell and back on drop.
    assert_eq!(
        layout
            .active_workspace()
            .unwrap()
            .grid_overview()
            .unwrap()
            .grabbed_window,
        Some(3),
    );
    assert_eq!(layout.grid_focused_window_id(), Some(3));

    layout.interactive_move_end(&3);
    layout.verify_invariants();

    assert_eq!(scrolling_column_ids(&layout), vec![vec![3, 1], vec![2]]);
    assert!(layout.is_grid_overview_open());
    assert_eq!(layout.grid_focused_window_id(), Some(3));
    assert_eq!(
        layout
            .active_workspace()
            .unwrap()
            .grid_overview()
            .unwrap()
            .grabbed_window,
        None,
    );
}

#[test]
fn grid_width_actions_during_drag_do_nothing() {
    let mut layout = large_grid_layout();
    check_ops_on_layout(&mut layout, [Op::CompleteAnimations]);

    let (output, start, _) = grid_window_point(&layout, 3, 0.5, 0.5);
    let (_, target, _) = grid_window_point(&layout, 1, 0.5, 0.25);
    assert!(layout.interactive_move_begin(3, &output, start));
    assert!(layout.interactive_move_update(&3, Point::from((300., 0.)), output.clone(), target));

    // The grid focus is the dragged window, which isn't in any workspace while it moves. These
    // used to look it up there and panic.
    let widths = |layout: &Layout<TestWindow>| {
        layout
            .active_workspace()
            .unwrap()
            .scrolling()
            .columns()
            .map(|col| *col.column_width())
            .collect::<Vec<_>>()
    };
    let before = widths(&layout);
    layout.toggle_width(true);
    layout.toggle_full_width();
    layout.set_column_width(SizeChange::SetProportion(50.));
    layout.verify_invariants();
    assert_eq!(widths(&layout), before);

    layout.interactive_move_end(&3);
    layout.verify_invariants();
}

fn minimized_ids(layout: &Layout<TestWindow>) -> Vec<usize> {
    let mut ids: Vec<_> = layout
        .windows()
        .map(|(_, win)| *win.id())
        .filter(|id| layout.is_window_minimized(id))
        .collect();
    ids.sort();
    ids
}

#[test]
fn grid_closing_with_all_windows_minimized_restores_nothing() {
    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::MinimizeWindow(1),
        Op::MinimizeWindow(2),
        Op::ToggleGridOverview,
        Op::ToggleGridOverview,
    ]);
    layout.verify_invariants();

    // Nothing was picked: the grid focus only sat on the first cell for lack of an active window.
    assert_eq!(minimized_ids(&layout), vec![1, 2]);
}

#[test]
fn grid_closing_on_a_minimized_cell_restores_it() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::MinimizeWindow(1),
        Op::FocusWindow(3),
        Op::ToggleGridOverview,
    ]);

    // Closing the grid confirms its focused cell, minimized or not.
    assert!(layout
        .active_workspace_mut()
        .unwrap()
        .set_grid_focus_for_window(&1));
    check_ops_on_layout(&mut layout, [Op::ToggleGridOverview]);
    assert_eq!(minimized_ids(&layout), Vec::<usize>::new());
    assert_eq!(
        layout
            .active_workspace()
            .unwrap()
            .active_window()
            .map(|w| *w.id()),
        Some(1)
    );

    // That includes a window just minimized in the grid, which keeps the grid focus.
    check_ops_on_layout(&mut layout, [Op::ToggleGridOverview, Op::MinimizeWindow(1)]);
    assert_eq!(layout.grid_focused_window_id(), Some(1));
    check_ops_on_layout(&mut layout, [Op::ToggleGridOverview]);
    assert_eq!(minimized_ids(&layout), Vec::<usize>::new());
}

#[test]
fn grid_closing_keeps_ignored_floating_window_active() {
    let mut ignored_floating = TestWindowParams::new(2);
    ignored_floating.is_floating = true;
    ignored_floating.rules = Some(ResolvedWindowRules {
        ignore_grid_overview: Some(true),
        ..ResolvedWindowRules::default()
    });

    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: ignored_floating,
        },
        Op::FocusWindow(2),
        Op::ToggleGridOverview,
        Op::ToggleGridOverview,
    ]);
    layout.verify_invariants();

    assert_eq!(
        layout
            .active_workspace()
            .unwrap()
            .active_window()
            .map(|w| *w.id()),
        Some(2)
    );
}

#[test]
fn grid_dragged_window_keeps_focus() {
    let mut layout = large_grid_layout();
    check_ops_on_layout(&mut layout, [Op::CompleteAnimations]);

    let (output, start, _) = grid_window_point(&layout, 3, 0.5, 0.5);
    let (_, target, _) = grid_window_point(&layout, 1, 0.5, 0.25);
    assert!(layout.interactive_move_begin(3, &output, start));
    assert!(layout.interactive_move_update(&3, Point::from((300., 0.)), output.clone(), target));

    // The dragged window used to lose keyboard focus for the whole drag.
    assert_eq!(layout.focus().map(|win| *win.id()), Some(3));

    layout.interactive_move_end(&3);
    layout.verify_invariants();
    assert_eq!(layout.focus().map(|win| *win.id()), Some(3));
}

#[test]
fn grid_merge_insert_computes_final_targets_immediately() {
    let mut ops = vec![Op::AddOutput(1)];
    for id in 1..=3 {
        let mut params = TestWindowParams::new(id);
        params.bbox = Rectangle::from_size(Size::from((1600, 1200)));
        ops.push(Op::AddWindow { params });
        ops.push(Op::Communicate(id));
    }
    ops.push(Op::FocusWindow(1));
    ops.push(Op::ToggleGridOverview);
    ops.push(Op::CompleteAnimations);
    let mut layout = check_ops(ops);

    // Grab window 3 and merge it into window 1's column.
    let (output, start, _) = grid_window_point(&layout, 3, 0.5, 0.5);
    let (_, target, _) = grid_window_point(&layout, 1, 0.5, 0.25);
    assert!(layout.interactive_move_begin(3, &output, start));
    assert!(layout.interactive_move_update(&3, Point::from((300., 0.)), output.clone(), target));
    layout.interactive_move_end(&3);
    layout.verify_invariants();
    assert_eq!(scrolling_column_ids(&layout), vec![vec![3, 1], vec![2]]);

    let entry_targets = |layout: &Layout<TestWindow>| -> Vec<_> {
        layout
            .active_workspace()
            .unwrap()
            .grid_overview()
            .unwrap()
            .layout
            .entries
            .iter()
            .map(|(_, info)| (info.target_pos, info.target_size, info.target_scale))
            .collect()
    };

    // The targets computed at drop time must already account for the pending resizes of the
    // merged column, so that the windows' subsequent commits don't shift every entry again
    // (previously visible as all windows shaking once).
    let targets_at_drop = entry_targets(&layout);

    check_ops_on_layout(
        &mut layout,
        [Op::Communicate(1), Op::Communicate(2), Op::Communicate(3)],
    );

    let targets_after_commit = entry_targets(&layout);
    assert_eq!(targets_at_drop.len(), targets_after_commit.len());
    for (before, after) in targets_at_drop.iter().zip(&targets_after_commit) {
        approx::assert_abs_diff_eq!(before.0.x, after.0.x, epsilon = 0.001);
        approx::assert_abs_diff_eq!(before.0.y, after.0.y, epsilon = 0.001);
        approx::assert_abs_diff_eq!(before.1.w, after.1.w, epsilon = 0.001);
        approx::assert_abs_diff_eq!(before.1.h, after.1.h, epsilon = 0.001);
        approx::assert_abs_diff_eq!(before.2, after.2, epsilon = 0.001);
    }
}

#[test]
fn grid_interactive_insert_preserves_pushed_tile_move_animation() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::ConsumeOrExpelWindowLeft { id: None },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::ToggleGridOverview,
        Op::CompleteAnimations,
    ]);

    let (output, start, _) = grid_window_point(&layout, 3, 0.5, 0.5);
    let (_, target, _) = grid_window_point(&layout, 2, 0.5, 0.25);
    assert!(layout.interactive_move_begin(3, &output, start));
    assert!(layout.interactive_move_update(&3, Point::from((300., 0.)), output, target));
    layout.interactive_move_end(&3);
    layout.verify_invariants();

    assert_eq!(scrolling_column_ids(&layout), vec![vec![1, 3, 2]]);
    let pushed_offset = layout
        .active_workspace()
        .unwrap()
        .scrolling()
        .columns()
        .flat_map(|col| col.tiles())
        .find(|(tile, _)| tile.window().id() == &2)
        .map(|(tile, _)| tile.render_offset().y)
        .unwrap();

    assert!(pushed_offset.abs() > 0.001);
}

#[test]
fn grid_toggle_applies_to_all_monitors_by_default() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddOutput(2),
        Op::FocusOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::FocusOutput(2),
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::FocusOutput(1),
    ]);

    layout.toggle_grid_overview();
    layout.verify_invariants();

    let MonitorSet::Normal { monitors, .. } = &layout.monitor_set else {
        unreachable!();
    };
    assert!(monitors[0].is_grid_overview_open());
    assert!(monitors[1].is_grid_overview_open());
    assert!(monitors[0].workspaces[0].is_grid_overview_open());
    assert!(monitors[1].workspaces[0].is_grid_overview_open());

    layout.toggle_grid_overview();
    layout.verify_invariants();

    let MonitorSet::Normal { monitors, .. } = &layout.monitor_set else {
        unreachable!();
    };
    assert!(!monitors[0].is_grid_overview_open());
    assert!(!monitors[1].is_grid_overview_open());
}

#[test]
fn grid_all_monitors_false_toggles_only_focused_monitor() {
    let options = Options {
        grid_overview: niri_config::GridOverview {
            grid_all_monitors: false,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut layout = check_ops_with_options(
        options,
        [
            Op::AddOutput(1),
            Op::AddOutput(2),
            Op::FocusOutput(1),
            Op::AddWindow {
                params: TestWindowParams::new(1),
            },
            Op::FocusOutput(2),
            Op::AddWindow {
                params: TestWindowParams::new(2),
            },
            Op::FocusOutput(1),
        ],
    );

    layout.toggle_grid_overview();
    layout.verify_invariants();

    let MonitorSet::Normal { monitors, .. } = &layout.monitor_set else {
        unreachable!();
    };
    assert!(monitors[0].is_grid_overview_open());
    assert!(!monitors[1].is_grid_overview_open());
}

#[test]
fn grid_all_monitors_false_cross_output_move_uses_target_monitor_grid_state() {
    let options = Options {
        grid_overview: niri_config::GridOverview {
            grid_all_monitors: false,
            ..Default::default()
        },
        ..Default::default()
    };
    let layout = check_ops_with_options(
        options,
        [
            Op::AddOutput(1),
            Op::AddOutput(2),
            Op::FocusOutput(1),
            Op::AddWindow {
                params: TestWindowParams::new(1),
            },
            Op::FocusOutput(2),
            Op::AddWindow {
                params: TestWindowParams::new(2),
            },
            Op::ToggleGridOverview,
            Op::FocusOutput(1),
            Op::MoveWindowToOutput {
                window_id: Some(1),
                output_id: 2,
                target_ws_idx: Some(1),
            },
        ],
    );

    let MonitorSet::Normal { monitors, .. } = &layout.monitor_set else {
        unreachable!();
    };
    assert!(!monitors[0].is_grid_overview_open());
    assert!(monitors[1].is_grid_overview_open());
    assert!(monitors[1].workspaces[1].has_window(&1));
    assert!(monitors[1].workspaces[1].is_grid_overview_open());
}

#[test]
fn grid_mode_applies_to_workspace_that_gets_first_window() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::ToggleGridOverview,
    ]);

    layout.move_to_workspace(None, 1, ActivateWindow::Yes);
    layout.verify_invariants();

    let MonitorSet::Normal { monitors, .. } = &layout.monitor_set else {
        unreachable!();
    };
    let monitor = &monitors[0];
    assert!(monitor.is_grid_overview_open());
    assert!(monitor.workspaces[1].has_windows());
    assert!(monitor.workspaces[1].is_grid_overview_open());
}

#[test]
fn overview_close_preserves_grid_mode() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::ToggleGridOverview,
        Op::ToggleOverview,
    ]);

    layout.toggle_overview();
    layout.verify_invariants();

    assert!(!layout.is_overview_open());
    assert!(layout.is_grid_overview_open());
}

#[test]
fn overview_grid_confirm_closes_overview_and_grid() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::ToggleOverview,
        Op::ToggleGridOverview,
    ]);

    assert!(layout.confirm_grid_selection_for_window(&1));
    layout.verify_invariants();

    assert!(!layout.is_overview_open());
    assert!(!layout.is_grid_overview_open());
}

#[test]
fn overview_grid_close_commits_grid_focus_and_keeps_overview_open() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::FocusWindow(1),
        Op::ToggleOverview,
        Op::ToggleGridOverview,
    ]);

    layout.focus_right();
    layout.focus_right();
    layout.verify_invariants();

    assert_eq!(layout.grid_focused_window_id(), Some(3));
    assert_eq!(
        layout
            .active_workspace()
            .unwrap()
            .active_window()
            .map(|window| *window.id()),
        Some(1)
    );

    assert!(layout.close_grid_overview());
    layout.verify_invariants();

    assert!(layout.is_overview_open());
    assert!(!layout.is_grid_overview_open());
    assert_eq!(layout.focus().map(|window| *window.id()), Some(3));
}

#[test]
fn grid_close_after_moving_focused_window_to_workspace_refits_view() {
    let wide_params = |id| {
        let mut params = TestWindowParams::new(id);
        params.bbox = Rectangle::from_size(Size::from((600, 200)));
        params
    };

    let expected = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: wide_params(1),
        },
        Op::AddWindow {
            params: wide_params(2),
        },
        Op::FocusWindow(2),
    ]);
    let expected_view_pos = expected
        .active_workspace()
        .unwrap()
        .scrolling()
        .target_view_pos();

    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: wide_params(1),
        },
        Op::AddWindow {
            params: wide_params(2),
        },
        Op::AddWindow {
            params: wide_params(3),
        },
        Op::FocusWindow(3),
    ]);
    layout.toggle_grid_overview();
    layout.move_to_workspace(None, 1, ActivateWindow::Smart);
    layout.switch_workspace(0);
    layout.verify_invariants();

    assert!(layout.is_grid_overview_open());
    assert_eq!(layout.grid_focused_window_id(), Some(2));

    assert!(layout.close_grid_overview());
    layout.verify_invariants();

    assert!(!layout.is_grid_overview_open());
    assert_eq!(layout.focus().map(|window| *window.id()), Some(2));
    let actual_view_pos = layout
        .active_workspace()
        .unwrap()
        .scrolling()
        .target_view_pos();
    approx::assert_abs_diff_eq!(actual_view_pos, expected_view_pos, epsilon = 0.001);
}

#[test]
fn removing_active_middle_column_refits_view_to_surviving_columns() {
    let expected = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: wide_625_params(1),
        },
        Op::Communicate(1),
        Op::AddWindow {
            params: wide_625_params(3),
        },
        Op::Communicate(3),
        Op::FocusWindow(1),
        Op::CompleteAnimations,
    ]);
    let expected_1 = window_render_pos(&expected, 1);
    let expected_3 = window_render_pos(&expected, 3);

    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: wide_625_params(1),
        },
        Op::Communicate(1),
        Op::AddWindow {
            params: wide_625_params(2),
        },
        Op::Communicate(2),
        Op::AddWindow {
            params: wide_625_params(3),
        },
        Op::Communicate(3),
        Op::FocusWindow(2),
        Op::CompleteAnimations,
        Op::CloseWindow(2),
        Op::CompleteAnimations,
    ]);

    assert_eq!(layout.focus().map(|window| *window.id()), Some(1));
    let actual_1 = window_render_pos(&layout, 1);
    let actual_3 = window_render_pos(&layout, 3);
    approx::assert_abs_diff_eq!(actual_1.x, expected_1.x, epsilon = 0.001);
    approx::assert_abs_diff_eq!(actual_1.y, expected_1.y, epsilon = 0.001);
    approx::assert_abs_diff_eq!(actual_3.x, expected_3.x, epsilon = 0.001);
    approx::assert_abs_diff_eq!(actual_3.y, expected_3.y, epsilon = 0.001);
}

#[test]
fn removing_active_middle_column_uses_window_movement_for_left_refill() {
    use niri_config::animations::{Curve, EasingParams, Kind};

    let mut options = Options::default();
    options.animations.horizontal_view_movement.0.kind = Kind::Easing(EasingParams {
        duration_ms: 100,
        curve: Curve::EaseOutExpo,
    });
    options.animations.window_movement.0.kind = Kind::Easing(EasingParams {
        duration_ms: 1000,
        curve: Curve::Linear,
    });

    let expected = check_ops_with_options(
        options.clone(),
        [
            Op::AddOutput(1),
            Op::AddWindow {
                params: wide_625_params(1),
            },
            Op::Communicate(1),
            Op::AddWindow {
                params: wide_625_params(3),
            },
            Op::Communicate(3),
            Op::FocusWindow(1),
            Op::CompleteAnimations,
        ],
    );
    let final_1 = window_render_pos(&expected, 1);
    let final_3 = window_render_pos(&expected, 3);

    let mut layout = check_ops_with_options(
        options,
        [
            Op::AddOutput(1),
            Op::AddWindow {
                params: wide_625_params(1),
            },
            Op::Communicate(1),
            Op::AddWindow {
                params: wide_625_params(2),
            },
            Op::Communicate(2),
            Op::AddWindow {
                params: wide_625_params(3),
            },
            Op::Communicate(3),
            Op::FocusWindow(2),
            Op::CompleteAnimations,
        ],
    );
    let before_1 = window_render_pos(&layout, 1);
    let before_3 = window_render_pos(&layout, 3);

    check_ops_on_layout(&mut layout, [Op::CloseWindow(2)]);
    approx::assert_abs_diff_eq!(window_render_pos(&layout, 1).x, before_1.x, epsilon = 0.001);

    check_ops_on_layout(&mut layout, [Op::AdvanceAnimations { msec_delta: 100 }]);
    let expected_1_x = before_1.x + (final_1.x - before_1.x) * 0.1;
    let expected_3_x = before_3.x + (final_3.x - before_3.x) * 0.1;
    approx::assert_abs_diff_eq!(window_render_pos(&layout, 1).x, expected_1_x, epsilon = 1.0);
    approx::assert_abs_diff_eq!(window_render_pos(&layout, 3).x, expected_3_x, epsilon = 1.0);
    assert_eq!(layout.focus().map(|window| *window.id()), Some(1));
}

#[test]
fn removing_active_middle_column_focuses_right_refill() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: wide_625_params(1),
        },
        Op::Communicate(1),
        Op::AddWindow {
            params: wide_625_params(2),
        },
        Op::Communicate(2),
        Op::AddWindow {
            params: wide_625_params(3),
        },
        Op::Communicate(3),
        Op::FocusWindow(1),
        Op::CompleteAnimations,
        Op::FocusWindow(2),
        Op::CompleteAnimations,
    ]);

    check_ops_on_layout(&mut layout, [Op::CloseWindow(2), Op::CompleteAnimations]);

    assert_eq!(layout.focus().map(|window| *window.id()), Some(3));
}

#[test]
fn removing_rightmost_columns_refills_view_repeatedly() {
    let mut expected = wide_columns(2, Options::default());
    check_ops_on_layout(&mut expected, [Op::CompleteAnimations]);
    let mut expected_after_first_close = wide_columns(3, Options::default());
    check_ops_on_layout(&mut expected_after_first_close, [Op::CompleteAnimations]);

    let mut layout = wide_columns(4, Options::default());
    check_ops_on_layout(
        &mut layout,
        [
            Op::CompleteAnimations,
            Op::CloseWindow(4),
            Op::CompleteAnimations,
        ],
    );
    approx::assert_abs_diff_eq!(
        scrolling_view_pos(&layout),
        scrolling_view_pos(&expected_after_first_close),
        epsilon = 0.001,
    );

    check_ops_on_layout(&mut layout, [Op::CloseWindow(3), Op::CompleteAnimations]);

    assert_eq!(layout.focus().map(|window| *window.id()), Some(2));
    approx::assert_abs_diff_eq!(
        scrolling_view_pos(&layout),
        scrolling_view_pos(&expected),
        epsilon = 0.001,
    );
}

#[test]
fn removing_rightmost_column_refills_view_after_focus_change() {
    let mut expected = wide_columns(3, Options::default());
    check_ops_on_layout(&mut expected, [Op::CompleteAnimations]);

    let mut layout = wide_columns(4, Options::default());
    check_ops_on_layout(
        &mut layout,
        [
            Op::CompleteAnimations,
            Op::FocusWindow(3),
            Op::CompleteAnimations,
            Op::FocusWindow(4),
            Op::CompleteAnimations,
            Op::CloseWindow(4),
            Op::CompleteAnimations,
        ],
    );

    assert_eq!(layout.focus().map(|window| *window.id()), Some(3));
    approx::assert_abs_diff_eq!(
        scrolling_view_pos(&layout),
        scrolling_view_pos(&expected),
        epsilon = 0.001,
    );
}

#[test]
fn removing_rightmost_column_preserves_centering_mode() {
    for mode in [CenterFocusedColumn::Always, CenterFocusedColumn::OnOverflow] {
        let mut options = Options::default();
        options.layout.center_focused_column = mode;

        let mut expected = wide_columns(3, options.clone());
        check_ops_on_layout(&mut expected, [Op::CompleteAnimations]);

        let mut layout = wide_columns(4, options);
        check_ops_on_layout(
            &mut layout,
            [
                Op::CompleteAnimations,
                Op::FocusWindow(3),
                Op::CompleteAnimations,
                Op::FocusWindow(4),
                Op::CompleteAnimations,
                Op::CloseWindow(4),
                Op::CompleteAnimations,
            ],
        );

        approx::assert_abs_diff_eq!(
            scrolling_view_pos(&layout),
            scrolling_view_pos(&expected),
            epsilon = 0.001,
        );
    }
}

#[test]
fn grid_close_after_focused_window_removal_refits_view() {
    let mut options = Options::default();
    options.layout.center_focused_column = CenterFocusedColumn::OnOverflow;

    let expected = check_ops_with_options(
        options.clone(),
        [
            Op::AddOutput(1),
            Op::AddWindow {
                params: wide_625_params(1),
            },
            Op::Communicate(1),
            Op::AddWindow {
                params: wide_625_params(2),
            },
            Op::Communicate(2),
            Op::FocusWindow(2),
            Op::CompleteAnimations,
        ],
    );
    let expected_1 = window_render_pos(&expected, 1);
    let expected_2 = window_render_pos(&expected, 2);

    let mut layout = check_ops_with_options(
        options,
        [
            Op::AddOutput(1),
            Op::AddWindow {
                params: wide_625_params(1),
            },
            Op::Communicate(1),
            Op::AddWindow {
                params: wide_625_params(2),
            },
            Op::Communicate(2),
            Op::AddWindow {
                params: wide_625_params(3),
            },
            Op::Communicate(3),
            Op::FocusWindow(3),
            Op::CompleteAnimations,
            Op::ToggleGridOverview,
            Op::CloseWindow(3),
        ],
    );

    assert!(layout.close_grid_overview());
    check_ops_on_layout(&mut layout, [Op::CompleteAnimations]);

    assert_eq!(layout.focus().map(|window| *window.id()), Some(2));
    let actual_1 = window_render_pos(&layout, 1);
    let actual_2 = window_render_pos(&layout, 2);
    approx::assert_abs_diff_eq!(actual_1.x, expected_1.x, epsilon = 0.001);
    approx::assert_abs_diff_eq!(actual_1.y, expected_1.y, epsilon = 0.001);
    approx::assert_abs_diff_eq!(actual_2.x, expected_2.x, epsilon = 0.001);
    approx::assert_abs_diff_eq!(actual_2.y, expected_2.y, epsilon = 0.001);
}

#[test]
fn grid_close_after_moving_focused_window_to_workspace_refits_render_positions() {
    let mut options = Options::default();
    options.layout.center_focused_column = CenterFocusedColumn::OnOverflow;

    let expected = check_ops_with_options(
        options.clone(),
        [
            Op::AddOutput(1),
            Op::AddWindow {
                params: wide_625_params(1),
            },
            Op::Communicate(1),
            Op::AddWindow {
                params: wide_625_params(2),
            },
            Op::Communicate(2),
            Op::FocusWindow(2),
            Op::CompleteAnimations,
        ],
    );
    let expected_1 = window_render_pos(&expected, 1);
    let expected_2 = window_render_pos(&expected, 2);

    let mut layout = check_ops_with_options(
        options,
        [
            Op::AddOutput(1),
            Op::AddWindow {
                params: wide_625_params(1),
            },
            Op::Communicate(1),
            Op::AddWindow {
                params: wide_625_params(2),
            },
            Op::Communicate(2),
            Op::AddWindow {
                params: wide_625_params(3),
            },
            Op::Communicate(3),
            Op::FocusWindow(3),
            Op::CompleteAnimations,
            Op::ToggleGridOverview,
        ],
    );
    layout.move_to_workspace(None, 1, ActivateWindow::Smart);
    layout.switch_workspace(0);
    layout.verify_invariants();

    assert!(layout.close_grid_overview());
    check_ops_on_layout(&mut layout, [Op::CompleteAnimations]);

    assert_eq!(layout.focus().map(|window| *window.id()), Some(2));
    let actual_1 = window_render_pos(&layout, 1);
    let actual_2 = window_render_pos(&layout, 2);
    approx::assert_abs_diff_eq!(actual_1.x, expected_1.x, epsilon = 0.001);
    approx::assert_abs_diff_eq!(actual_1.y, expected_1.y, epsilon = 0.001);
    approx::assert_abs_diff_eq!(actual_2.x, expected_2.x, epsilon = 0.001);
    approx::assert_abs_diff_eq!(actual_2.y, expected_2.y, epsilon = 0.001);
}

#[test]
fn overview_grid_close_after_moving_focused_window_to_workspace_refits_source_workspace() {
    let mut options = Options::default();
    options.layout.center_focused_column = CenterFocusedColumn::OnOverflow;

    let expected = check_ops_with_options(
        options.clone(),
        [
            Op::AddOutput(1),
            Op::AddWindow {
                params: wide_625_params(1),
            },
            Op::Communicate(1),
            Op::AddWindow {
                params: wide_625_params(2),
            },
            Op::Communicate(2),
            Op::FocusWindow(2),
            Op::CompleteAnimations,
        ],
    );
    let expected_1 = window_render_pos(&expected, 1);
    let expected_2 = window_render_pos(&expected, 2);

    let mut layout = check_ops_with_options(
        options,
        [
            Op::AddOutput(1),
            Op::AddWindow {
                params: wide_625_params(1),
            },
            Op::Communicate(1),
            Op::AddWindow {
                params: wide_625_params(2),
            },
            Op::Communicate(2),
            Op::AddWindow {
                params: wide_625_params(3),
            },
            Op::Communicate(3),
            Op::FocusWindow(3),
            Op::CompleteAnimations,
            Op::ToggleOverview,
            Op::ToggleGridOverview,
        ],
    );
    layout.move_to_workspace(None, 1, ActivateWindow::Smart);
    layout.verify_invariants();

    assert!(layout.close_grid_overview());
    layout.switch_workspace(0);
    check_ops_on_layout(&mut layout, [Op::CompleteAnimations]);

    assert_eq!(layout.focus().map(|window| *window.id()), Some(2));
    let actual_1 = window_render_pos(&layout, 1);
    let actual_2 = window_render_pos(&layout, 2);
    approx::assert_abs_diff_eq!(actual_1.x, expected_1.x, epsilon = 0.001);
    approx::assert_abs_diff_eq!(actual_1.y, expected_1.y, epsilon = 0.001);
    approx::assert_abs_diff_eq!(actual_2.x, expected_2.x, epsilon = 0.001);
    approx::assert_abs_diff_eq!(actual_2.y, expected_2.y, epsilon = 0.001);
}

#[test]
fn grid_interactive_move_starts_with_small_drag() {
    let mut layout = large_grid_layout();
    assert!(layout
        .active_workspace_mut()
        .unwrap()
        .set_grid_focus_for_window(&1));
    check_ops_on_layout(&mut layout, [Op::CompleteAnimations]);

    let (output, start, _) = grid_window_point(&layout, 3, 0.5, 0.5);
    let delta = Point::from((40., 0.));
    assert!(layout.interactive_move_begin(3, &output, start));
    assert!(layout.interactive_move_update(&3, delta, output, start + delta));

    assert!(matches!(
        layout.interactive_move,
        Some(InteractiveMoveState::Moving(_))
    ));
}

#[test]
fn grid_recomputes_after_window_communicate_while_open() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::ToggleGridOverview,
        Op::CompleteAnimations,
    ]);
    let initial_size = grid_entry_target_size(&layout, 1);

    check_ops_on_layout(
        &mut layout,
        [
            Op::SetForcedSize {
                id: 1,
                size: Some(Size::from((1000, 200))),
            },
            Op::Communicate(1),
        ],
    );
    let after_communicate = grid_entry_target_size(&layout, 1);

    check_ops_on_layout(
        &mut layout,
        [
            Op::ToggleGridOverview,
            Op::CompleteAnimations,
            Op::ToggleGridOverview,
            Op::CompleteAnimations,
        ],
    );
    let after_reopen = grid_entry_target_size(&layout, 1);

    // The open grid shows the new size, at the scale it had; the window got smaller, but not by
    // enough to zoom everything in while the grid stays open. Opened afresh, it shows the window
    // as large as fits.
    assert_ne!(after_communicate, initial_size);
    approx::assert_abs_diff_eq!(
        after_communicate.w / after_communicate.h,
        5.,
        epsilon = 0.001
    );
    assert!(after_reopen.w >= after_communicate.w);
}

#[test]
fn grid_passive_recompute_while_opening_preserves_visual_position() {
    let mut layout = large_grid_layout();
    check_ops_on_layout(&mut layout, [Op::AdvanceAnimations { msec_delta: 50 }]);

    let progress_before = layout
        .active_workspace()
        .unwrap()
        .grid_overview()
        .unwrap()
        .progress_value();
    assert!(progress_before > 0. && progress_before < 1.);
    let before = grid_window_visual_rect(&layout, 2).loc;

    check_ops_on_layout(
        &mut layout,
        [
            Op::SetForcedSize {
                id: 2,
                size: Some(Size::from((1000, 200))),
            },
            Op::Communicate(2),
        ],
    );

    let progress_after = layout
        .active_workspace()
        .unwrap()
        .grid_overview()
        .unwrap()
        .progress_value();
    let after = grid_window_visual_rect(&layout, 2).loc;

    approx::assert_abs_diff_eq!(progress_after, progress_before, epsilon = 0.0001);
    approx::assert_abs_diff_eq!(after.x, before.x, epsilon = 0.001);
    approx::assert_abs_diff_eq!(after.y, before.y, epsilon = 0.001);
}

#[test]
fn grid_recompute_while_rearranging_does_not_restart_animation() {
    let mut layout = three_column_grid_layout(1);
    check_ops_on_layout(
        &mut layout,
        [Op::ToggleGridOverview, Op::CompleteAnimations],
    );

    layout.move_right();
    layout.verify_invariants();

    check_ops_on_layout(&mut layout, [Op::AdvanceAnimations { msec_delta: 50 }]);
    let value_before = grid_rearrange_anim_value(&layout);
    assert!(value_before > 0.);

    check_ops_on_layout(
        &mut layout,
        [
            Op::SetForcedSize {
                id: 1,
                size: Some(Size::from((1000, 200))),
            },
            Op::Communicate(1),
        ],
    );
    let value_after = grid_rearrange_anim_value(&layout);

    assert!(value_after >= value_before);
}

#[test]
fn grid_passive_recompute_preserves_sources_for_unchanged_targets() {
    let mut layout = three_column_grid_layout(1);
    check_ops_on_layout(
        &mut layout,
        [Op::ToggleGridOverview, Op::CompleteAnimations],
    );

    layout.move_right();
    check_ops_on_layout(&mut layout, [Op::AdvanceAnimations { msec_delta: 50 }]);

    let (positions_before, scales_before) = {
        let go = layout
            .active_workspace()
            .and_then(|ws| ws.grid_overview())
            .unwrap();
        (go.entry_positions.clone(), go.entry_scales.clone())
    };

    layout.update_window(&1, None);

    let go = layout
        .active_workspace()
        .and_then(|ws| ws.grid_overview())
        .unwrap();
    assert_eq!(go.entry_positions, positions_before);
    assert_eq!(go.entry_scales, scales_before);
}

#[test]
fn grid_passive_recompute_when_idle_does_not_start_rearrange_animation() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::ToggleGridOverview,
        Op::CompleteAnimations,
    ]);

    assert!(!grid_has_rearrange_anim(&layout));
    check_ops_on_layout(
        &mut layout,
        [
            Op::SetForcedSize {
                id: 1,
                size: Some(Size::from((1000, 200))),
            },
            Op::Communicate(1),
        ],
    );

    assert!(!grid_has_rearrange_anim(&layout));
}

#[test]
fn grid_action_while_rearranging_restarts_animation() {
    let mut layout = three_column_grid_layout(1);
    check_ops_on_layout(
        &mut layout,
        [Op::ToggleGridOverview, Op::CompleteAnimations],
    );

    layout.move_right();
    layout.verify_invariants();
    check_ops_on_layout(&mut layout, [Op::AdvanceAnimations { msec_delta: 50 }]);
    let value_before = grid_rearrange_anim_value(&layout);
    assert!(value_before > 0.);

    layout.move_right();
    layout.verify_invariants();
    let value_after = grid_rearrange_anim_value(&layout);

    assert!(value_after < value_before);
    approx::assert_abs_diff_eq!(value_after, 0., epsilon = 0.001);
}

#[test]
fn grid_confirm_matches_normal_focus_view_offset_at_edges() {
    let mut direct = three_column_grid_layout(1);
    check_ops_on_layout(&mut direct, [Op::FocusWindow(3)]);
    let expected_right = direct
        .active_workspace()
        .unwrap()
        .scrolling()
        .target_view_pos();

    let mut grid = three_column_grid_layout(1);
    check_ops_on_layout(&mut grid, [Op::ToggleGridOverview]);
    grid.focus_right();
    grid.verify_invariants();
    grid.focus_right();
    grid.verify_invariants();
    assert_eq!(grid.grid_focused_window_id(), Some(3));
    assert!(grid.confirm_grid_selection_for_window(&3));
    grid.verify_invariants();
    let actual_right = grid
        .active_workspace()
        .unwrap()
        .scrolling()
        .target_view_pos();
    approx::assert_abs_diff_eq!(actual_right, expected_right, epsilon = 0.001);

    let mut direct = three_column_grid_layout(3);
    check_ops_on_layout(&mut direct, [Op::FocusWindow(1)]);
    let expected_left = direct
        .active_workspace()
        .unwrap()
        .scrolling()
        .target_view_pos();

    let mut grid = three_column_grid_layout(3);
    check_ops_on_layout(&mut grid, [Op::ToggleGridOverview]);
    grid.focus_left();
    grid.verify_invariants();
    grid.focus_left();
    grid.verify_invariants();
    assert_eq!(grid.grid_focused_window_id(), Some(1));
    assert!(grid.confirm_grid_selection_for_window(&1));
    grid.verify_invariants();
    let actual_left = grid
        .active_workspace()
        .unwrap()
        .scrolling()
        .target_view_pos();
    approx::assert_abs_diff_eq!(actual_left, expected_left, epsilon = 0.001);
}

#[test]
fn move_column_to_workspace_maximize_and_fullscreen() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::MaximizeWindowToEdges { id: None },
        Op::FullscreenWindow(1),
        Op::MoveColumnToWorkspaceDown(true),
        Op::FullscreenWindow(1),
    ];

    let layout = check_ops(ops);
    let (_, win) = layout.windows().next().unwrap();

    // Unfullscreening should return to maximized because the window was maximized before.
    assert_eq!(win.pending_sizing_mode(), SizingMode::Maximized);
}

fn tile_width(layout: &Layout<TestWindow>, id: usize) -> f64 {
    let ws = layout.active_workspace().unwrap();
    let tile = ws.tiles().find(|tile| *tile.window().id() == id).unwrap();
    tile.tile_size().w
}

/// Starts an interactive move of maximized window 2 (with a custom column width), next to a
/// normal window 1. Returns the unmaximized width of window 2 and the render rect of window 1
/// once window 2 is lifted out of the layout.
fn interactive_move_maximized_window_set_up() -> (Layout<TestWindow>, f64, Rectangle<f64, Logical>)
{
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::SetColumnWidth(SizeChange::SetProportion(30.)),
        Op::Communicate(2),
        Op::CompleteAnimations,
    ];
    let mut layout = check_ops(ops);
    let width = tile_width(&layout, 2);

    let ops = [
        Op::MaximizeWindowToEdges { id: Some(2) },
        Op::Communicate(2),
        Op::CompleteAnimations,
        Op::InteractiveMoveBegin {
            window: 2,
            output_idx: 1,
            px: 640.,
            py: 360.,
        },
        Op::InteractiveMoveUpdate {
            window: 2,
            dx: 0.,
            dy: 1000.,
            output_idx: 1,
            px: 640.,
            py: 360.,
        },
        Op::CompleteAnimations,
    ];
    check_ops_on_layout(&mut layout, ops);

    let Some(InteractiveMoveState::Moving(move_)) = &layout.interactive_move else {
        panic!("window 2 must be moving");
    };
    // Grabbing must not unmaximize the window.
    assert!(move_.tile.window().pending_sizing_mode().is_maximized());
    assert_eq!(move_.width, ColumnWidth::Proportion(0.3));

    let ws = layout.active_workspace().unwrap();
    let (tile, pos, _) = ws
        .tiles_with_render_positions()
        .find(|(tile, _, _)| *tile.window().id() == 1)
        .unwrap();
    let rect = Rectangle::new(pos, tile.tile_size());
    (layout, width, rect)
}

#[test]
fn interactive_move_maximized_into_new_column_stays_maximized() {
    let (mut layout, width, rect) = interactive_move_maximized_window_set_up();

    // Drop it right of window 1, between columns.
    let pos: Point<f64, Logical> =
        Point::from((rect.loc.x + rect.size.w + 1., rect.loc.y + rect.size.h / 2.));
    let ops = [
        Op::InteractiveMoveUpdate {
            window: 2,
            dx: 0.,
            dy: 0.,
            output_idx: 1,
            px: pos.x,
            py: pos.y,
        },
        Op::InteractiveMoveEnd { window: 2 },
        Op::Communicate(2),
        Op::CompleteAnimations,
    ];
    check_ops_on_layout(&mut layout, ops);

    let ws = layout.active_workspace().unwrap();
    let col = ws
        .scrolling()
        .columns()
        .find(|col| col.contains(&2))
        .unwrap();
    assert!(!col.contains(&1));
    assert!(col.is_pending_maximized());
    let (_, win) = layout.windows().find(|(_, win)| win.0.id == 2).unwrap();
    assert!(win.pending_sizing_mode().is_maximized());

    // Unmaximizing returns to the width from before the move.
    let ops = [
        Op::MaximizeWindowToEdges { id: Some(2) },
        Op::Communicate(2),
        Op::CompleteAnimations,
    ];
    check_ops_on_layout(&mut layout, ops);
    let ws = layout.active_workspace().unwrap();
    let col = ws
        .scrolling()
        .columns()
        .find(|col| col.contains(&2))
        .unwrap();
    assert!(!col.is_pending_maximized());
    assert_eq!(tile_width(&layout, 2), width);
}

#[test]
fn interactive_move_keeps_full_width_column() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::MaximizeColumn,
        Op::Communicate(2),
        Op::CompleteAnimations,
    ];
    let mut layout = check_ops(ops);
    let width = tile_width(&layout, 2);

    let ops = [
        Op::InteractiveMoveBegin {
            window: 2,
            output_idx: 1,
            px: 640.,
            py: 360.,
        },
        Op::InteractiveMoveUpdate {
            window: 2,
            dx: 0.,
            dy: 1000.,
            output_idx: 1,
            px: 640.,
            py: 360.,
        },
        Op::Communicate(2),
        Op::CompleteAnimations,
    ];
    check_ops_on_layout(&mut layout, ops);

    let Some(InteractiveMoveState::Moving(move_)) = &layout.interactive_move else {
        panic!("window 2 must be moving");
    };
    assert!(move_.is_full_width);
    assert_eq!(move_.tile.tile_size().w, width);

    let ops = [
        Op::InteractiveMoveEnd { window: 2 },
        Op::Communicate(2),
        Op::CompleteAnimations,
    ];
    check_ops_on_layout(&mut layout, ops);
    assert_eq!(tile_width(&layout, 2), width);
}

#[test]
fn interactive_move_maximized_into_column_becomes_normal() {
    let (mut layout, _, rect) = interactive_move_maximized_window_set_up();

    // Drop it at the bottom edge of window 1, inside its column.
    let pos: Point<f64, Logical> =
        Point::from((rect.loc.x + rect.size.w / 2., rect.loc.y + rect.size.h - 5.));
    let ops = [
        Op::InteractiveMoveUpdate {
            window: 2,
            dx: 0.,
            dy: 0.,
            output_idx: 1,
            px: pos.x,
            py: pos.y,
        },
        Op::InteractiveMoveEnd { window: 2 },
        Op::Communicate(2),
        Op::CompleteAnimations,
    ];
    check_ops_on_layout(&mut layout, ops);

    // A column with several tiles can't be maximized, so the window joins it as a normal tile.
    let ws = layout.active_workspace().unwrap();
    let col = ws
        .scrolling()
        .columns()
        .find(|col| col.contains(&2))
        .unwrap();
    assert!(col.contains(&1));
    assert!(!col.is_pending_maximized());
    let (_, win) = layout.windows().find(|(_, win)| win.0.id == 2).unwrap();
    assert!(win.pending_sizing_mode().is_normal());
}

#[test]
fn move_window_to_workspace_maximize_and_fullscreen() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::MaximizeWindowToEdges { id: None },
        Op::FullscreenWindow(1),
        Op::MoveWindowToWorkspaceDown(true),
        Op::FullscreenWindow(1),
    ];

    let layout = check_ops(ops);
    let (_, win) = layout.windows().next().unwrap();

    // Unfullscreening should return to maximized because the window was maximized before.
    //
    // FIXME: it currently doesn't because windows themselves can only be either fullscreen or
    // maximized. So when a window is fullscreen, whether it is also maximized or not is stored in
    // the column. MoveWindowToWorkspace removes the window from the column and this information is
    // forgotten.
    assert_eq!(win.pending_sizing_mode(), SizingMode::Normal);
}

#[test]
fn tabs_with_different_border() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams {
                rules: Some(ResolvedWindowRules {
                    border: niri_config::BorderRule {
                        on: true,
                        ..Default::default()
                    },
                    ..ResolvedWindowRules::default()
                }),
                ..TestWindowParams::new(2)
            },
        },
        Op::SwitchPresetWindowHeight { id: None },
        Op::ToggleColumnTabbedDisplay,
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::ConsumeOrExpelWindowLeft { id: None },
    ];

    let options = Options {
        layout: niri_config::Layout {
            struts: Struts {
                left: FloatOrInt(0.),
                right: FloatOrInt(0.),
                top: FloatOrInt(20000.),
                bottom: FloatOrInt(0.),
            },
            ..Default::default()
        },
        ..Default::default()
    };
    check_ops_with_options(options, ops);
}

#[test]
fn expel_pending_left_from_fullscreen_tabbed_column() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::FullscreenWindow(1),
        Op::Communicate(1),
        // 1 is now fullscreen, view_offset_to_restore is set.
        Op::ToggleColumnTabbedDisplay,
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::ConsumeOrExpelWindowLeft { id: Some(2) },
        // 2 is consumed into a fullscreen column, fullscreen is requested but not applied.
        //
        // Now, get it back out while keeping it focused.
        //
        // Importantly, we expel it *left*, which results in adding a new column with the exact
        // same active_column_idx.
        Op::FocusWindow(2),
        Op::ConsumeOrExpelWindowLeft { id: None },
    ];

    check_ops(ops);
}

#[test]
fn minimize_restores_to_original_position() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::MinimizeWindow(2),
    ];

    let mut layout = check_ops(ops);

    assert!(layout.is_window_minimized(&2));
    assert!(layout.has_window(&2));
    assert_ne!(layout.focus().map(|w| *w.id()), Some(2));

    check_ops_on_layout(&mut layout, [Op::UnminimizeWindow(2)]);

    assert!(!layout.is_window_minimized(&2));
    assert_eq!(layout.focus().map(|w| *w.id()), Some(2));

    // The window must return between its old neighbors.
    let (_, _, ws) = layout.workspaces().next().unwrap();
    let ids: Vec<usize> = ws.windows().map(|w| *w.id()).collect();
    assert_eq!(ids, [1, 2, 3]);
}

#[test]
fn minimize_focused_window_moves_focus() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::MinimizeWindow(2),
    ];

    let layout = check_ops(ops);

    assert!(layout.is_window_minimized(&2));
    assert_eq!(layout.focus().map(|w| *w.id()), Some(1));
}

#[test]
fn close_window_while_minimized() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::MinimizeWindow(1),
        Op::CloseWindow(1),
    ];

    let layout = check_ops(ops);

    assert!(!layout.has_window(&1));
    assert!(layout.has_window(&2));
}

#[test]
fn minimize_fullscreen_window_roundtrip() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::SetFullscreenWindow {
            window: 1,
            is_fullscreen: true,
        },
        Op::Communicate(1),
        Op::MinimizeWindow(1),
        Op::UnminimizeWindow(1),
        Op::Communicate(1),
    ];

    let layout = check_ops(ops);

    assert!(!layout.is_window_minimized(&1));
    let (_, win) = layout.windows().find(|(_, w)| *w.id() == 1).unwrap();
    assert!(win.pending_sizing_mode().is_fullscreen());
}

#[test]
fn workspace_with_only_minimized_windows_survives() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::MinimizeWindow(1),
        Op::FocusWorkspaceDown,
        Op::FocusWorkspaceUp,
    ];

    let layout = check_ops(ops);

    assert!(layout.has_window(&1));
    assert!(layout.is_window_minimized(&1));
}

#[test]
fn minimize_from_tabbed_column_restores_into_it() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::ToggleColumnTabbedDisplay,
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::ConsumeOrExpelWindowLeft { id: Some(2) },
        Op::MinimizeWindow(2),
        Op::UnminimizeWindow(2),
    ];

    let layout = check_ops(ops);

    assert!(!layout.is_window_minimized(&2));
    assert!(layout.has_window(&1));
    assert!(layout.has_window(&2));
}

#[test]
fn minimize_floating_window_roundtrip() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: {
                let mut params = TestWindowParams::new(2);
                params.is_floating = true;
                params
            },
        },
        Op::MinimizeWindow(2),
        Op::UnminimizeWindow(2),
    ];

    let layout = check_ops(ops);

    assert!(!layout.is_window_minimized(&2));
    assert!(layout.has_window(&2));
}

#[test]
fn unminimize_last_restores_most_recent() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::MinimizeWindow(1),
        Op::MinimizeWindow(2),
    ];

    let mut layout = check_ops(ops);

    assert_eq!(layout.last_minimized_window(), Some(2));
    layout.unminimize_last_window();
    layout.verify_invariants();

    assert!(!layout.is_window_minimized(&2));
    assert!(layout.is_window_minimized(&1));
    assert_eq!(layout.last_minimized_window(), Some(1));
}

/// Three columns with the middle one minimized, plus its grid overview open and focused on
/// `focus_id`.
fn grid_with_minimized_middle(focus_id: usize) -> Layout<TestWindow> {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::MinimizeWindow(2),
        Op::ToggleGridOverview,
    ]);

    assert!(layout.is_grid_overview_open());
    for _ in 0..8 {
        if layout.grid_focused_window_id() == Some(focus_id) {
            break;
        }
        layout.focus_left();
    }
    for _ in 0..8 {
        if layout.grid_focused_window_id() == Some(focus_id) {
            break;
        }
        layout.focus_right();
    }
    assert_eq!(layout.grid_focused_window_id(), Some(focus_id));
    layout.verify_invariants();

    layout
}

#[track_caller]
fn assert_window_order(layout: &Layout<TestWindow>, expected: [usize; 3]) {
    let ws = layout.active_workspace().unwrap();
    let ids: Vec<usize> = ws.windows().map(|w| *w.id()).collect();
    assert_eq!(ids, expected);
}

#[test]
fn grid_move_column_left_steps_over_minimized_cell() {
    let mut layout = grid_with_minimized_middle(3);

    // The minimized window has its own grid cell, so the move lands on it instead of jumping
    // across to the next visible column.
    check_ops_on_layout(&mut layout, [Op::MoveColumnLeft]);
    assert_window_order(&layout, [1, 3, 2]);
    assert!(layout.is_window_minimized(&2));

    check_ops_on_layout(&mut layout, [Op::MoveColumnLeft]);
    assert_window_order(&layout, [3, 1, 2]);
}

#[test]
fn grid_move_column_right_steps_over_minimized_cell() {
    let mut layout = grid_with_minimized_middle(1);

    check_ops_on_layout(&mut layout, [Op::MoveColumnRight]);
    assert_window_order(&layout, [2, 1, 3]);
    assert!(layout.is_window_minimized(&2));

    check_ops_on_layout(&mut layout, [Op::MoveColumnRight]);
    assert_window_order(&layout, [2, 3, 1]);
}

#[test]
fn grid_move_column_to_first_and_index_count_minimized_cells() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::MinimizeWindow(1),
        Op::ToggleGridOverview,
    ]);
    assert_eq!(layout.grid_focused_window_id(), Some(3));

    check_ops_on_layout(&mut layout, [Op::MoveColumnToFirst]);
    assert_window_order(&layout, [3, 1, 2]);

    check_ops_on_layout(&mut layout, [Op::MoveColumnToIndex(2)]);
    assert_window_order(&layout, [1, 3, 2]);

    check_ops_on_layout(&mut layout, [Op::MoveColumnToLast]);
    assert_window_order(&layout, [1, 2, 3]);
}

#[test]
fn move_column_left_skips_minimized_outside_grid() {
    // Without the grid overview, minimized windows are invisible, so moves jump over them.
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::MinimizeWindow(2),
        Op::FocusWindow(3),
    ]);

    check_ops_on_layout(&mut layout, [Op::MoveColumnLeft]);
    assert_window_order(&layout, [3, 1, 2]);
}

#[test]
fn grid_moving_minimized_cell_keeps_it_minimized() {
    let mut layout = grid_with_minimized_middle(2);

    check_ops_on_layout(&mut layout, [Op::MoveColumnLeft]);
    assert_window_order(&layout, [2, 1, 3]);
    assert!(layout.is_window_minimized(&2));
    assert_eq!(layout.grid_focused_window_id(), Some(2));
    // Moving it must not make it the active window either.
    assert_ne!(
        layout
            .active_workspace()
            .unwrap()
            .active_window()
            .map(|w| *w.id()),
        Some(2)
    );

    check_ops_on_layout(&mut layout, [Op::MoveColumnLeft]);
    assert_window_order(&layout, [2, 1, 3]);

    check_ops_on_layout(&mut layout, [Op::MoveColumnToLast]);
    assert_window_order(&layout, [1, 3, 2]);
    assert!(layout.is_window_minimized(&2));

    // There is nothing above or below a minimized window inside its own column.
    check_ops_on_layout(&mut layout, [Op::MoveWindowUp, Op::MoveWindowDown]);
    assert_window_order(&layout, [1, 3, 2]);
    assert!(layout.is_window_minimized(&2));
}

#[test]
fn grid_restore_does_not_leave_strip_move_animations() {
    // The grid reads the columns' render offsets to know where a cell flies back to, so the
    // insert animation from a restore must not be left running: the neighbors would fly to their
    // pre-restore spots and then slide back.
    let mut layout = grid_with_minimized_middle(3);
    check_ops_on_layout(&mut layout, [Op::CompleteAnimations]);
    check_ops_on_layout(&mut layout, [Op::UnminimizeWindow(2)]);

    let ws = layout.active_workspace().unwrap();
    for col in ws.scrolling().columns() {
        assert_eq!(col.render_offset(), Point::from((0., 0.)));
    }
}

#[test]
fn grid_minimizing_leaves_the_grid_focus_alone() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::FocusWindow(2),
        Op::ToggleGridOverview,
    ]);
    assert_eq!(layout.grid_focused_window_id(), Some(2));

    // Minimizing the focused cell keeps the focus on it: the cell is still there, and pressing
    // minimize again restores it. The strip's activation moves on, which is what the grid focus
    // used to follow.
    layout.set_window_minimized(&2, true);
    layout.verify_invariants();
    assert_eq!(layout.grid_focused_window_id(), Some(2));
    assert!(layout.is_window_minimized(&2));
    assert_ne!(
        layout
            .active_workspace()
            .unwrap()
            .active_window()
            .map(|w| *w.id()),
        Some(2)
    );

    // Minimizing some other window must not pull the focus over to it either.
    layout.set_window_minimized(&3, true);
    layout.verify_invariants();
    assert_eq!(layout.grid_focused_window_id(), Some(2));

    // Restoring does focus the window it restored.
    layout.set_window_minimized(&3, false);
    layout.verify_invariants();
    assert_eq!(layout.grid_focused_window_id(), Some(3));
}

#[test]
fn grid_unminimize_in_place_keeps_the_grid_open_and_focused() {
    // Pressing minimize on an already-minimized cell restores it without activating, so the grid
    // stays open on the window and the previously active window keeps the activation.
    let mut layout = grid_with_minimized_middle(2);
    let active_before = layout
        .active_workspace()
        .unwrap()
        .active_window()
        .map(|w| *w.id());

    layout.unminimize_window(&2, false);
    layout.verify_invariants();

    assert!(!layout.is_window_minimized(&2));
    assert!(layout.is_grid_overview_open());
    assert_eq!(layout.grid_focused_window_id(), Some(2));
    assert_window_order(&layout, [1, 2, 3]);
    assert_eq!(
        layout
            .active_workspace()
            .unwrap()
            .active_window()
            .map(|w| *w.id()),
        active_before,
        "restoring in place must not steal the activation"
    );
}

#[test]
fn grid_overview_picks_up_config_reloads() {
    // The overview is created once and then lives as long as its workspace, so it used to keep
    // serving the options it was first opened with: colors from an included file (matugen and
    // friends rewrite those live), gap, padding and scales all froze at that point.
    let mut layout = grid_with_minimized_middle(3);

    let highlight = niri_config::GridMinimizedHighlight {
        off: false,
        color: niri_config::Color::from_rgba8_unpremul(1, 2, 3, 255),
        urgent_color: niri_config::Color::from_rgba8_unpremul(4, 5, 6, 255),
        padding: 12.,
        corner_radius: Some(4.),
        dim: 0.5,
    };
    let options = Options {
        grid_overview: niri_config::GridOverview {
            gap: 64.,
            minimized_highlight: highlight,
            ..Default::default()
        },
        ..Default::default()
    };
    layout.update_options(options);
    layout.verify_invariants();

    let go = layout.active_workspace().unwrap().grid_overview().unwrap();
    assert_eq!(
        go.options.grid_overview.minimized_highlight, highlight,
        "the overview must serve the current highlight config"
    );
    assert_eq!(go.layout.gap, 64., "a new gap must re-lay out the grid");
}

#[test]
fn grid_drop_onto_minimized_cell_never_merges_into_it() {
    // Merging a window into the placeholder column would leave the minimized tile inside a
    // visible column, which renders as one cell, so the minimized window would lose its own cell.
    let mut layout = grid_with_minimized_middle(3);
    check_ops_on_layout(&mut layout, [Op::CompleteAnimations]);

    let ws = layout.active_workspace().unwrap();
    let go = ws.grid_overview().unwrap();
    let info = *go.find_grid_info(&2).unwrap();

    // Not right at the middle, where the side comes down to rounding.
    for frac in [0.1, 0.3, 0.45, 0.55, 0.7, 0.9] {
        let pos =
            info.target_pos + Point::from((info.target_size.w * frac, info.target_size.h / 2.));
        let position = ws.grid_insert_position(pos).unwrap();
        let expected = if frac < 0.5 {
            InsertPosition::NewColumn(1)
        } else {
            InsertPosition::NewColumn(2)
        };
        assert_eq!(position, expected, "at x fraction {frac}");
    }
}

#[test]
fn grid_drop_onto_tabbed_minimized_cell_never_merges_into_it() {
    // Placeholder columns take default-column-display.
    let options = Options {
        layout: niri_config::Layout {
            default_column_display: ColumnDisplay::Tabbed,
            ..Default::default()
        },
        ..Default::default()
    };
    let layout = check_ops_with_options(
        options,
        [
            Op::AddOutput(1),
            Op::AddWindow {
                params: TestWindowParams::new(1),
            },
            Op::AddWindow {
                params: TestWindowParams::new(2),
            },
            Op::AddWindow {
                params: TestWindowParams::new(3),
            },
            Op::MinimizeWindow(2),
            Op::ToggleGridOverview,
            Op::CompleteAnimations,
        ],
    );

    let ws = layout.active_workspace().unwrap();
    // A placeholder column in tabbed display shows up as a tab cell.
    assert!(matches!(
        ws.scrolling().grid_item_for_window(&2),
        Some(super::grid_overview::GridItem::Tab { .. })
    ));

    let go = ws.grid_overview().unwrap();
    let info = *go.find_grid_info(&2).unwrap();
    for frac in [0.1, 0.3, 0.7, 0.9] {
        let pos =
            info.target_pos + Point::from((info.target_size.w * frac, info.target_size.h / 2.));
        let position = ws.grid_insert_position(pos).unwrap();
        let expected = if frac < 0.5 {
            InsertPosition::NewColumn(1)
        } else {
            InsertPosition::NewColumn(2)
        };
        assert_eq!(position, expected, "at x fraction {frac}");
    }
    layout.verify_invariants();
}

#[test]
fn workspace_render_geo_at_fractional_scale() {
    let ops = [
        Op::AddScaledOutput {
            id: 1,
            scale: 1.1,
            layout_config: None,
        },
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::FocusWorkspaceDown,
        Op::CompleteAnimations,
    ];

    let layout = check_ops(ops);

    let MonitorSet::Normal { monitors, .. } = &layout.monitor_set else {
        unreachable!()
    };

    let mon = &monitors[0];
    let mut iter = mon.workspaces_with_render_geo();
    let (_ws, geo) = iter.next().unwrap();
    assert!(
        iter.next().is_none(),
        "animations are completed, only one workspace should be visible"
    );
    assert_eq!(
        geo.loc.y, 0.,
        "active workspace must be at y = 0 exactly, \
         otherwise a pointer against the screen edge at y = 0 won't hit it"
    );
}

fn parent_id_causes_loop(layout: &Layout<TestWindow>, id: usize, mut parent_id: usize) -> bool {
    if parent_id == id {
        return true;
    }

    'outer: loop {
        for (_, win) in layout.windows() {
            if win.0.id == parent_id {
                match win.0.parent_id.get() {
                    Some(new_parent_id) => {
                        if new_parent_id == id {
                            // Found a loop.
                            return true;
                        }

                        parent_id = new_parent_id;
                        continue 'outer;
                    }
                    // Reached window with no parent.
                    None => return false,
                }
            }
        }

        // Parent is not in the layout.
        return false;
    }
}

fn arbitrary_spacing() -> impl Strategy<Value = f64> {
    // Give equal weight to:
    // - 0: the element is disabled
    // - 4: some reasonable value
    // - random value, likely unreasonably big
    prop_oneof![Just(0.), Just(4.), ((1.)..=65535.)]
}

fn arbitrary_spacing_neg() -> impl Strategy<Value = f64> {
    // Give equal weight to:
    // - 0: the element is disabled
    // - 4: some reasonable value
    // - -4: some reasonable negative value
    // - random value, likely unreasonably big
    prop_oneof![Just(0.), Just(4.), Just(-4.), ((1.)..=65535.)]
}

fn arbitrary_struts() -> impl Strategy<Value = Struts> {
    (
        arbitrary_spacing_neg(),
        arbitrary_spacing_neg(),
        arbitrary_spacing_neg(),
        arbitrary_spacing_neg(),
    )
        .prop_map(|(left, right, top, bottom)| Struts {
            left: FloatOrInt(left),
            right: FloatOrInt(right),
            top: FloatOrInt(top),
            bottom: FloatOrInt(bottom),
        })
}

fn arbitrary_center_focused_column() -> impl Strategy<Value = CenterFocusedColumn> {
    prop_oneof![
        Just(CenterFocusedColumn::Never),
        Just(CenterFocusedColumn::OnOverflow),
        Just(CenterFocusedColumn::Always),
    ]
}

fn arbitrary_tab_indicator_position() -> impl Strategy<Value = TabIndicatorPosition> {
    prop_oneof![
        Just(TabIndicatorPosition::Left),
        Just(TabIndicatorPosition::Right),
        Just(TabIndicatorPosition::Top),
        Just(TabIndicatorPosition::Bottom),
    ]
}

prop_compose! {
    fn arbitrary_focus_ring()(
        off in any::<bool>(),
        width in prop::option::of(arbitrary_spacing().prop_map(FloatOrInt)),
    ) -> niri_config::BorderRule {
        niri_config::BorderRule {
            off,
            on: !off,
            width,
            ..Default::default()
        }
    }
}

prop_compose! {
    fn arbitrary_border()(
        off in any::<bool>(),
        width in prop::option::of(arbitrary_spacing().prop_map(FloatOrInt)),
    ) -> niri_config::BorderRule {
        niri_config::BorderRule {
            off,
            on: !off,
            width,
            ..Default::default()
        }
    }
}

prop_compose! {
    fn arbitrary_shadow()(
        off in any::<bool>(),
        softness in prop::option::of(arbitrary_spacing().prop_map(FloatOrInt)),
    ) -> niri_config::ShadowRule {
        niri_config::ShadowRule {
            off,
            on: !off,
            softness,
            ..Default::default()
        }
    }
}

prop_compose! {
    fn arbitrary_tab_indicator()(
        off in any::<bool>(),
        hide_when_single_tab in prop::option::of(any::<bool>().prop_map(Flag)),
        place_within_column in prop::option::of(any::<bool>().prop_map(Flag)),
        width in prop::option::of(arbitrary_spacing().prop_map(FloatOrInt)),
        gap in prop::option::of(arbitrary_spacing_neg().prop_map(FloatOrInt)),
        length in prop::option::of((0f64..2f64)
            .prop_map(|x| TabIndicatorLength { total_proportion: Some(x) })),
        position in prop::option::of(arbitrary_tab_indicator_position()),
    ) -> niri_config::TabIndicatorPart {
        niri_config::TabIndicatorPart {
            off,
            on: !off,
            hide_when_single_tab,
            place_within_column,
            width,
            gap,
            length,
            position,
            ..Default::default()
        }
    }
}

prop_compose! {
    fn arbitrary_layout_part()(
        gaps in prop::option::of(arbitrary_spacing().prop_map(FloatOrInt)),
        struts in prop::option::of(arbitrary_struts()),
        focus_ring in prop::option::of(arbitrary_focus_ring()),
        border in prop::option::of(arbitrary_border()),
        shadow in prop::option::of(arbitrary_shadow()),
        tab_indicator in prop::option::of(arbitrary_tab_indicator()),
        center_focused_column in prop::option::of(arbitrary_center_focused_column()),
        always_center_single_column in prop::option::of(any::<bool>().prop_map(Flag)),
        empty_workspace_above_first in prop::option::of(any::<bool>().prop_map(Flag)),
        focused_window_above_floating in prop::option::of(any::<bool>().prop_map(Flag)),
    ) -> niri_config::LayoutPart {
        niri_config::LayoutPart {
            gaps,
            struts,
            center_focused_column,
            always_center_single_column,
            empty_workspace_above_first,
            focused_window_above_floating,
            focus_ring,
            border,
            shadow,
            tab_indicator,
            ..Default::default()
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: if std::env::var_os("RUN_SLOW_TESTS").is_none() {
            eprintln!("ignoring slow test");
            0
        } else {
            ProptestConfig::default().cases
        },
        ..ProptestConfig::default()
    })]

    #[test]
    fn random_operations_dont_panic(
        ops: Vec<Op>,
        layout_config in arbitrary_layout_part(),
    ) {
        // eprintln!("{ops:?}");
        let options = Options {
            layout: niri_config::Layout::from_part(&layout_config),
            ..Default::default()
        };

        check_ops_with_options(options, ops);
    }
}

#[test]
fn grid_moves_a_focused_ignored_window_like_without_the_grid() {
    let mut ignored_floating = TestWindowParams::new(2);
    ignored_floating.is_floating = true;
    ignored_floating.rules = Some(ResolvedWindowRules {
        ignore_grid_overview: Some(true),
        ..ResolvedWindowRules::default()
    });
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: ignored_floating,
        },
        Op::FocusWindow(1),
        Op::ToggleGridOverview,
        Op::CompleteAnimations,
    ]);
    let pos = |layout: &Layout<TestWindow>| {
        let ws = layout.active_workspace().unwrap();
        ws.tiles_with_render_positions()
            .find(|(tile, _, _)| tile.window().id() == &2)
            .map(|(_, pos, _)| pos)
            .unwrap()
    };

    // The user clicks the ignored window, then presses a move bind.
    layout.activate_window(&2);
    let before = pos(&layout);
    layout.move_left();
    check_ops_on_layout(&mut layout, [Op::CompleteAnimations]);

    // It used to move (and activate) the grid's focused cell instead, leaving the window where it
    // was and taking the focus away from it.
    assert!(pos(&layout).x < before.x);
    assert_eq!(layout.focus().map(|win| *win.id()), Some(2));
    assert_eq!(layout.grid_focused_window_id(), Some(1));
    assert!(layout.is_grid_overview_open());
}

#[test]
fn floating_focus_left_goes_to_the_window_alongside() {
    let floating = |id: usize, (w, h): (i32, i32)| {
        let mut params = TestWindowParams::new(id);
        params.is_floating = true;
        params.bbox = Rectangle::from_size(Size::from((w, h)));
        Op::AddWindow { params }
    };
    let place = |id: usize, x: f64, y: f64| Op::MoveFloatingWindow {
        id: Some(id),
        x: PositionChange::SetFixed(x),
        y: PositionChange::SetFixed(y),
        animate: false,
    };
    // A small window top left, a large one top right, and a wide one along the bottom whose
    // center is much closer to the top right one's horizontally.
    let mut layout = check_ops([
        Op::AddOutput(1),
        floating(1, (200, 300)),
        floating(2, (600, 500)),
        floating(3, (800, 300)),
        place(1, 20., 20.),
        place(2, 1000., 20.),
        place(3, 450., 700.),
        Op::FocusWindow(2),
    ]);

    // It used to go by horizontal distance alone, to the window along the bottom.
    layout.focus_left();
    assert_eq!(layout.focus().map(|win| *win.id()), Some(1));

    // And down from there to the one along the bottom.
    layout.focus_down();
    assert_eq!(layout.focus().map(|win| *win.id()), Some(3));
}

#[test]
fn grid_opens_again_the_way_it_closed() {
    let mut floating = TestWindowParams::new(4);
    floating.is_floating = true;
    floating.bbox = Rectangle::from_size(Size::from((900, 700)));
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::ToggleGridOverview,
        Op::CompleteAnimations,
        // A floating window opens while the grid shows: it takes the room that is left.
        Op::AddWindow { params: floating },
        Op::CompleteAnimations,
    ]);
    let cells = |layout: &Layout<TestWindow>| {
        let ws = layout.active_workspace().unwrap();
        let go = ws.grid_overview().unwrap();
        let mut cells: Vec<_> = go
            .layout
            .entries
            .iter()
            .map(|(item, info)| (*item.window_id(), info.rect()))
            .collect();
        cells.sort_by_key(|(id, _)| *id);
        cells
    };
    let shown = cells(&layout);

    // Opened again, everything is where it was, rather than laid out anew, which can put the
    // floating window somewhere else.
    check_ops_on_layout(
        &mut layout,
        [
            Op::ToggleGridOverview,
            Op::CompleteAnimations,
            Op::ToggleGridOverview,
            Op::CompleteAnimations,
        ],
    );
    assert_eq!(cells(&layout), shown);

    // A floating window moved in the meantime takes the spot closest to where it is now.
    check_ops_on_layout(
        &mut layout,
        [
            Op::ToggleGridOverview,
            Op::CompleteAnimations,
            Op::MoveFloatingWindow {
                id: Some(4),
                x: PositionChange::SetFixed(0.),
                y: PositionChange::SetFixed(0.),
                animate: false,
            },
            Op::ToggleGridOverview,
            Op::CompleteAnimations,
        ],
    );
    let moved = cells(&layout);
    let (_, before) = shown.iter().find(|(id, _)| *id == 4).unwrap();
    let (_, after) = moved.iter().find(|(id, _)| *id == 4).unwrap();
    assert_ne!(after, before);
}

#[test]
fn toggle_window_floating_switches_the_focused_cell_in_the_grid() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::FocusWindow(2),
        Op::ToggleGridOverview,
        Op::CompleteAnimations,
    ]);
    // The grid focus goes to window 1, while window 2 stays the active window.
    layout.focus_left();
    assert_eq!(layout.grid_focused_window_id(), Some(1));
    let is_floating = |layout: &Layout<TestWindow>, id: usize| {
        layout.active_workspace().unwrap().is_floating(&id)
    };

    check_ops_on_layout(
        &mut layout,
        [
            Op::ToggleWindowFloating { id: None },
            Op::CompleteAnimations,
        ],
    );
    assert!(is_floating(&layout, 1));
    assert!(!is_floating(&layout, 2));
    assert!(layout.is_grid_overview_open());
    assert_eq!(layout.grid_focused_window_id(), Some(1));
    let ws = layout.active_workspace().unwrap();
    let cell = ws
        .grid_overview()
        .unwrap()
        .find_grid_info(&1)
        .unwrap()
        .rect();
    assert!(ws.working_area().contains_rect(cell), "{cell:?}");

    // And back.
    check_ops_on_layout(
        &mut layout,
        [
            Op::ToggleWindowFloating { id: None },
            Op::CompleteAnimations,
        ],
    );
    assert!(!is_floating(&layout, 1));
    assert!(layout.is_grid_overview_open());
    assert_eq!(layout.grid_focused_window_id(), Some(1));
}

#[test]
fn grid_dropping_a_floating_window_moves_it_on_from_where_it_was_dropped() {
    let mut floating = TestWindowParams::new(3);
    floating.is_floating = true;
    floating.bbox = Rectangle::from_size(Size::from((500, 300)));
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindow { params: floating },
        Op::ToggleGridOverview,
        Op::CompleteAnimations,
    ]);

    // The tiled windows show in the middle and the floating window right of them. Grab the
    // floating window in the middle and drop it into the room on the left, a bit lower.
    let (output, start, _) = grid_window_point(&layout, 3, 0.5, 0.5);
    let before = grid_window_visual_rect(&layout, 3);
    let delta = Point::from((-650., 100.));
    assert!(layout.interactive_move_begin(3, &output, start));
    // Past the threshold the grab turns into a move, then the window follows the pointer.
    let nudge = Point::from((-30., 0.));
    assert!(layout.interactive_move_update(&3, nudge, output.clone(), start + nudge));
    assert!(layout.interactive_move_update(&3, delta - nudge, output.clone(), start + delta));
    let Some(InteractiveMoveState::Moving(move_)) = &layout.interactive_move else {
        panic!("expected an interactive move");
    };
    let let_go = Rectangle::new(
        move_.tile_render_location(1.),
        move_.tile.tile_size().upscale(move_.total_scale(1.)),
    );
    layout.interactive_move_end(&3);

    // It shows where it was let go of right away, rather than wherever its top left corner
    // would put its middle, and lands on the side of the tiled windows it was dropped on.
    let center = |r: Rectangle<f64, Logical>| r.loc + r.size.downscale(2.).to_point();
    let dropped = grid_window_visual_rect(&layout, 3);
    check_ops_on_layout(&mut layout, [Op::CompleteAnimations]);
    let settled = grid_window_visual_rect(&layout, 3);
    let moved = center(settled) - center(before);
    assert!(moved.x < -100., "{before:?} {settled:?}");
    let off = center(dropped) - center(let_go);
    assert!(
        off.x.abs() < 2. && off.y.abs() < 2.,
        "let go at {let_go:?}, but shows at {dropped:?}"
    );
    let tiled_left = grid_window_visual_rect(&layout, 1).loc.x;
    assert!(
        settled.loc.x + settled.size.w <= tiled_left,
        "{settled:?} not left of the tiled windows at {tiled_left}"
    );

    // Dropped onto the tiled windows, it can't stay there: it moves on to the closest free spot
    // from where it was let go of, rather than jumping there.
    let (output, start, _) = grid_window_point(&layout, 3, 0.5, 0.5);
    let tiled = grid_window_visual_rect(&layout, 1);
    let delta = center(tiled) - start;
    let nudge = Point::from((30., 0.));
    assert!(layout.interactive_move_begin(3, &output, start));
    assert!(layout.interactive_move_update(&3, nudge, output.clone(), start + nudge));
    assert!(layout.interactive_move_update(&3, delta - nudge, output.clone(), start + delta));
    let Some(InteractiveMoveState::Moving(move_)) = &layout.interactive_move else {
        panic!("expected an interactive move");
    };
    let let_go = Rectangle::new(
        move_.tile_render_location(1.),
        move_.tile.tile_size().upscale(move_.total_scale(1.)),
    );
    layout.interactive_move_end(&3);
    let dropped = grid_window_visual_rect(&layout, 3);
    check_ops_on_layout(&mut layout, [Op::AdvanceAnimations { msec_delta: 50 }]);
    let moving = grid_window_visual_rect(&layout, 3);
    check_ops_on_layout(&mut layout, [Op::CompleteAnimations]);
    let settled = grid_window_visual_rect(&layout, 3);
    let off = center(dropped) - center(let_go);
    assert!(
        off.x.abs() < 2. && off.y.abs() < 2.,
        "{let_go:?} {dropped:?}"
    );
    let tiled = grid_window_visual_rect(&layout, 1);
    assert!(!tiled.overlaps(settled), "{tiled:?} {settled:?}");
    let way = center(settled) - center(dropped);
    let gone = center(moving) - center(dropped);
    assert!(
        gone.x * way.x + gone.y * way.y > 0. && moving != settled,
        "{dropped:?} {moving:?} {settled:?}"
    );
}

/// A layout like a screenshot of the problem: a few tiled windows, and two small floating windows
/// in the top corners.
fn grid_corner_floats_layout(tiled: usize) -> Layout<TestWindow> {
    let mut ops = vec![Op::AddOutput(1)];
    for id in 1..=tiled {
        ops.push(Op::AddWindow {
            params: half_width_window(id),
        });
    }
    for (id, x) in [(100, 40.), (101, 1000.)] {
        let mut params = TestWindowParams::new(id);
        params.is_floating = true;
        params.bbox = Rectangle::from_size(Size::from((240, 180)));
        ops.push(Op::AddWindow { params });
        ops.push(Op::MoveFloatingWindow {
            id: Some(id),
            x: PositionChange::SetFixed(x),
            y: PositionChange::SetFixed(40.),
            animate: false,
        });
    }
    check_ops(ops)
}

/// A tiled window about half as wide as the output, like a terminal in a half-width column.
fn half_width_window(id: usize) -> TestWindowParams {
    let mut params = TestWindowParams::new(id);
    params.bbox = Rectangle::from_size(Size::from((620, 680)));
    params
}

fn grid_rows(layout: &Layout<TestWindow>) -> usize {
    let go = layout.active_workspace().unwrap().grid_overview().unwrap();
    go.layout
        .entries
        .iter()
        .filter_map(|(_, info)| info.row)
        .max()
        .map_or(0, |row| row + 1)
}

fn grid_scale(layout: &Layout<TestWindow>) -> f64 {
    layout
        .active_workspace()
        .unwrap()
        .grid_overview()
        .unwrap()
        .layout
        .scale
}

#[test]
fn grid_wraps_windows_opened_while_it_was_closed() {
    let mut layout = grid_corner_floats_layout(2);
    check_ops_on_layout(
        &mut layout,
        [
            Op::ToggleGridOverview,
            Op::CompleteAnimations,
            Op::ToggleGridOverview,
            Op::CompleteAnimations,
        ],
    );
    // Many more tiled windows open while the grid is closed.
    for id in 3..=14 {
        check_ops_on_layout(
            &mut layout,
            [Op::AddWindow {
                params: half_width_window(id),
            }],
        );
    }
    check_ops_on_layout(
        &mut layout,
        [Op::ToggleGridOverview, Op::CompleteAnimations],
    );

    // Opening the grid, they wrap into rows as if it opened the first time, rather than all
    // joining the one row the grid showed last time.
    let mut fresh = grid_corner_floats_layout(14);
    check_ops_on_layout(&mut fresh, [Op::ToggleGridOverview, Op::CompleteAnimations]);
    assert!(grid_rows(&layout) > 1);
    assert!(grid_scale(&layout) >= grid_scale(&fresh) * 0.92 - 0.001);
}

#[test]
fn grid_wraps_windows_opening_while_it_shows() {
    let mut layout = grid_corner_floats_layout(2);
    check_ops_on_layout(
        &mut layout,
        [Op::ToggleGridOverview, Op::CompleteAnimations],
    );
    for id in 3..=14 {
        check_ops_on_layout(
            &mut layout,
            [
                Op::AddWindow {
                    params: half_width_window(id),
                },
                Op::CompleteAnimations,
            ],
        );
    }

    // The rows wrap anew once one row gets too long, rather than all windows ending up in one
    // tiny row.
    let mut fresh = grid_corner_floats_layout(14);
    check_ops_on_layout(&mut fresh, [Op::ToggleGridOverview, Op::CompleteAnimations]);
    assert!(grid_rows(&layout) > 1, "one row at {}", grid_scale(&layout));
    assert!(
        grid_scale(&layout) >= grid_scale(&fresh) / 1.15 - 0.001,
        "{} vs {} fresh",
        grid_scale(&layout),
        grid_scale(&fresh)
    );
}

#[test]
fn focused_tiled_windows_stay_above_floating_in_focus_order() {
    let mut layout = focused_window_above_floating_layout(true);
    check_ops_on_layout(
        &mut layout,
        [Op::AddWindow {
            params: TestWindowParams::new(3),
        }],
    );
    let output = layout.outputs().next().unwrap().clone();
    let frame = |layout: &mut Layout<TestWindow>| {
        layout.update_render_elements(Some(&output));
    };

    layout.activate_window(&1);
    frame(&mut layout);
    let pos = tiled_and_floating_overlap(&layout);
    assert_eq!(window_id_under(&layout, pos), Some(1));

    // Focusing another tiled window used to put the floating window back over the first one.
    layout.activate_window(&3);
    frame(&mut layout);
    let pos = tiled_and_floating_overlap(&layout);
    assert_eq!(window_id_under(&layout, pos), Some(1));
    let ws = layout.active_workspace().unwrap();
    let order: Vec<_> = ws
        .tiles_with_render_positions()
        .map(|(tile, _, _)| *tile.window().id())
        .collect();
    assert!(
        order.iter().position(|id| *id == 1) < order.iter().position(|id| *id == 2),
        "{order:?}"
    );

    // Focusing the floating window brings the floating windows back on top of all of them.
    layout.activate_window(&2);
    frame(&mut layout);
    let pos = tiled_and_floating_overlap(&layout);
    assert_eq!(window_id_under(&layout, pos), Some(2));
    layout.activate_window(&3);
    frame(&mut layout);
    let pos = tiled_and_floating_overlap(&layout);
    assert_eq!(window_id_under(&layout, pos), Some(2));
}

#[test]
fn focus_floating_actions_bring_floating_windows_back_on_top() {
    for op in [Op::FocusFloating, Op::SwitchFocusFloatingTiling] {
        let mut layout = focused_window_above_floating_layout(true);
        check_ops_on_layout(
            &mut layout,
            [Op::AddWindow {
                params: TestWindowParams::new(3),
            }],
        );
        let output = layout.outputs().next().unwrap().clone();
        let frame = |layout: &mut Layout<TestWindow>| {
            layout.update_render_elements(Some(&output));
        };

        // Window 1 goes above the floating window, and stays there as window 3 gets the focus.
        layout.activate_window(&1);
        frame(&mut layout);
        layout.activate_window(&3);
        frame(&mut layout);
        let pos = tiled_and_floating_overlap(&layout);
        assert_eq!(window_id_under(&layout, pos), Some(1));

        // Focusing the floating windows by action brings them back on top, like clicking one.
        check_ops_on_layout(&mut layout, [op.clone()]);
        frame(&mut layout);
        let pos = tiled_and_floating_overlap(&layout);
        assert_eq!(window_id_under(&layout, pos), Some(2), "{op:?}");
        let ws = layout.active_workspace().unwrap();
        let order: Vec<_> = ws
            .tiles_with_render_positions()
            .map(|(tile, _, _)| *tile.window().id())
            .collect();
        assert_eq!(order.first(), Some(&2), "{op:?}: {order:?}");
    }
}

/// Like a screenshot of the problem: five tiled windows, three small floating windows along the
/// top, and seven floating file managers that all opened in the middle of the screen, on top of
/// each other.
fn grid_stacked_floats_layout(cascade: bool) -> Layout<TestWindow> {
    let mut ops = vec![Op::AddOutput(1)];
    for id in 1..=5 {
        ops.push(Op::AddWindow {
            params: half_width_window(id),
        });
    }
    for (n, id) in (100..103).enumerate() {
        let mut params = TestWindowParams::new(id);
        params.is_floating = true;
        params.bbox = Rectangle::from_size(Size::from((360, 220)));
        ops.push(Op::AddWindow { params });
        ops.push(Op::MoveFloatingWindow {
            id: Some(id),
            x: PositionChange::SetFixed(200. + 300. * n as f64),
            y: PositionChange::SetFixed(30.),
            animate: false,
        });
    }
    for (n, id) in (200..207).enumerate() {
        let mut params = TestWindowParams::new(id);
        params.is_floating = true;
        params.bbox = Rectangle::from_size(Size::from((450, 320)));
        ops.push(Op::AddWindow { params });
        if cascade {
            ops.push(Op::MoveFloatingWindow {
                id: Some(id),
                x: PositionChange::SetFixed(400. + 20. * n as f64),
                y: PositionChange::SetFixed(220. + 20. * n as f64),
                animate: false,
            });
        }
    }
    ops.push(Op::ToggleGridOverview);
    ops.push(Op::CompleteAnimations);
    check_ops(ops)
}

#[test]
fn grid_spreads_floating_windows_that_opened_on_top_of_each_other() {
    for cascade in [false, true] {
        let layout = grid_stacked_floats_layout(cascade);
        // They spread out rather than into one tall column that everything has to shrink for
        // (they showed at 0.14 and 0.23 that way).
        let go = layout.active_workspace().unwrap().grid_overview().unwrap();
        let shown: Vec<f64> = (200..207)
            .map(|id| go.find_grid_info(&id).unwrap().target_scale)
            .collect();
        assert!(shown.iter().all(|s| *s > 0.3), "{shown:?}");
        // The ones along the top stay in the upper half.
        let go = layout.active_workspace().unwrap().grid_overview().unwrap();
        for id in 100..103 {
            let r = go.find_grid_info(&id).unwrap().rect();
            assert!(r.loc.y + r.size.h / 2. < 360., "{id}: {r:?}");
        }
    }
}

fn grid_gesture_layout() -> Layout<TestWindow> {
    let mut ops = vec![Op::AddOutput(1)];
    for id in 1..=3 {
        ops.push(Op::AddWindow {
            params: half_width_window(id),
        });
    }
    ops.push(Op::FocusColumnLeft);
    check_ops(ops)
}

fn grid_progress(layout: &Layout<TestWindow>) -> Option<f64> {
    let go = layout.active_workspace().unwrap().grid_overview()?;
    go.progress.is_some().then(|| go.progress_value())
}

fn gesture(layout: &mut Layout<TestWindow>, deltas: &[f64]) {
    let mut ops = vec![Op::GridGestureBegin];
    for (n, &delta) in deltas.iter().enumerate() {
        ops.push(Op::GridGestureUpdate {
            delta,
            timestamp: Duration::from_millis(16 * (n as u64 + 1)),
        });
    }
    check_ops_on_layout(layout, ops);
}

#[test]
fn grid_gesture_opens_the_grid_as_far_as_the_fingers_go() {
    let mut layout = grid_gesture_layout();
    let active = layout
        .active_workspace()
        .unwrap()
        .active_window()
        .map(|w| *w.id());

    gesture(&mut layout, &[50., 50., 50.]);
    // Halfway, and it stays there while the fingers rest.
    let p = grid_progress(&layout).unwrap();
    assert!((p - 0.5).abs() < 0.01, "{p}");
    let shown = grid_window_visual_rect(&layout, 2);
    let cell = grid_entry_target_size(&layout, 2);
    assert!(
        cell.w + 1. < shown.size.w && shown.size.w < 620. - 1.,
        "{shown:?} between {cell:?} and full size"
    );
    check_ops_on_layout(&mut layout, [Op::AdvanceAnimations { msec_delta: 500 }]);
    assert_eq!(grid_progress(&layout), Some(p));

    // Further up, then let go: it opens the rest of the way.
    check_ops_on_layout(
        &mut layout,
        [
            Op::GridGestureUpdate {
                delta: 200.,
                timestamp: Duration::from_millis(600),
            },
            Op::GridGestureEnd,
            Op::CompleteAnimations,
        ],
    );
    let ws = layout.active_workspace().unwrap();
    assert!(ws.grid_overview().unwrap().is_fully_open());
    assert_eq!(ws.active_window().map(|w| *w.id()), active);
}

#[test]
fn grid_gesture_that_turns_back_leaves_the_grid_closed() {
    let mut layout = grid_gesture_layout();
    let active = layout
        .active_workspace()
        .unwrap()
        .active_window()
        .map(|w| *w.id());

    gesture(&mut layout, &[60., 60., -60., -60., -30.]);
    check_ops_on_layout(&mut layout, [Op::GridGestureEnd, Op::CompleteAnimations]);
    let ws = layout.active_workspace().unwrap();
    assert!(!ws.is_grid_overview_open());
    assert!(ws.grid_overview().is_none());
    assert_eq!(ws.active_window().map(|w| *w.id()), active);
}

#[test]
fn grid_gesture_closes_an_open_grid() {
    let mut layout = grid_gesture_layout();
    check_ops_on_layout(
        &mut layout,
        [Op::ToggleGridOverview, Op::CompleteAnimations],
    );

    gesture(&mut layout, &[-60., -60.]);
    let p = grid_progress(&layout).unwrap();
    assert!((p - 0.6).abs() < 0.01, "{p}");
    // On its way closed, the grid still shows.
    assert!(layout
        .active_workspace()
        .unwrap()
        .is_grid_overview_animation());

    check_ops_on_layout(
        &mut layout,
        [
            Op::GridGestureUpdate {
                delta: -200.,
                timestamp: Duration::from_millis(100),
            },
            Op::GridGestureEnd,
            Op::CompleteAnimations,
        ],
    );
    let ws = layout.active_workspace().unwrap();
    assert!(!ws.is_grid_overview_open());
    assert!(ws.grid_overview().is_none());
}

#[test]
fn grid_gesture_that_turns_back_keeps_the_grid_open() {
    let mut layout = grid_gesture_layout();
    check_ops_on_layout(
        &mut layout,
        [Op::ToggleGridOverview, Op::CompleteAnimations],
    );
    let cells: Vec<_> = layout
        .active_workspace()
        .unwrap()
        .grid_overview()
        .unwrap()
        .layout
        .entries
        .iter()
        .map(|(_, info)| info.rect())
        .collect();

    gesture(&mut layout, &[-80., -80., 80., 80., 20.]);
    check_ops_on_layout(&mut layout, [Op::GridGestureEnd, Op::CompleteAnimations]);
    let go = layout.active_workspace().unwrap().grid_overview().unwrap();
    assert!(go.is_fully_open());
    let again: Vec<_> = go
        .layout
        .entries
        .iter()
        .map(|(_, info)| info.rect())
        .collect();
    assert_eq!(cells, again);
}

#[test]
fn minimized_window_moved_away_while_the_grid_closes_shows_at_once() {
    let mut params = TestWindowParams::new(2);
    params.bbox = Rectangle::from_size(Size::from((1, 1)));
    check_ops([
        Op::AddOutput(1),
        Op::AddWindow { params },
        Op::MinimizeWindow(2),
        Op::ToggleGridOverview,
        Op::CloseGridOverview,
        Op::MoveWindowToWorkspace {
            window_id: Some(2),
            workspace_idx: 1,
        },
    ]);
}

#[test]
fn grid_gesture_stops_at_closed_and_at_open() {
    let mut layout = grid_gesture_layout();
    let normal = {
        let ws = layout.active_workspace().unwrap();
        ws.tiles_with_render_positions()
            .find(|(tile, _, _)| tile.window().id() == &2)
            .map(|(_, pos, _)| pos)
            .unwrap()
    };

    // Up a little, then down well past where it started.
    gesture(&mut layout, &[60., -200., -200.]);
    assert_eq!(grid_progress(&layout), Some(0.));
    let shown = grid_window_visual_rect(&layout, 2);
    assert!(
        (shown.loc.x - normal.x).abs() < 0.5 && (shown.loc.y - normal.y).abs() < 0.5,
        "{shown:?} vs {normal:?}"
    );
    assert!((shown.size.w - 620.).abs() < 0.5, "{shown:?}");
    check_ops_on_layout(&mut layout, [Op::GridGestureEnd, Op::CompleteAnimations]);

    // Up well past open.
    gesture(&mut layout, &[200., 200., 200.]);
    assert_eq!(grid_progress(&layout), Some(1.));
    let cell = grid_entry_target_size(&layout, 2);
    let shown = grid_window_visual_rect(&layout, 2);
    assert!((shown.size.w - cell.w).abs() < 0.5, "{shown:?} vs {cell:?}");
}

#[test]
fn grid_floating_only_windows_stay_in_their_rows() {
    use super::grid_overview::{GridItem, GridLayout, GridSource};

    let config = niri_config::GridOverview::default();
    let area = Rectangle::from_size(Size::from((1920., 1080.)));
    let item = |id: usize, x: f64, y: f64| {
        (
            GridItem::Floating { window_id: id },
            GridSource {
                size: Size::from((500., 300.)),
                pos: Some(Point::from((x, y))),
            },
        )
    };
    // Three windows at the top, three in the middle, three at the bottom.
    let mut items = Vec::new();
    for (row, y) in [50., 400., 750.].into_iter().enumerate() {
        for (col, x) in [50., 700., 1350.].into_iter().enumerate() {
            items.push(item(row * 3 + col + 1, x, y));
        }
    }
    let middle_y = |layout: &GridLayout<TestWindow>, id: usize| {
        layout
            .entries
            .iter()
            .find(|(item, _)| *item.window_id() == id)
            .map(|(_, info)| info.target_pos.y + info.target_size.h / 2.)
            .unwrap()
    };
    // Pairs of windows in the same row.
    let same_row = |layout: &GridLayout<TestWindow>, ids: &[usize]| {
        let mut pairs = Vec::new();
        for &a in ids {
            for &b in ids {
                if a < b && (middle_y(layout, a) - middle_y(layout, b)).abs() < 0.5 {
                    pairs.push((a, b));
                }
            }
        }
        pairs
    };
    let all: Vec<usize> = (1..=9).collect();
    let before = GridLayout::compute(&items, area, &config, None);
    let rows = same_row(&before, &all);
    assert_eq!(rows.len(), 9, "{rows:?}");

    // One more opens at the top: it joins the top row, and the others stay in theirs.
    items.push(item(10, 400., 60.));
    let opened = GridLayout::compute(&items, area, &config, Some(&before));
    assert_eq!(same_row(&opened, &all), rows);
    assert!((middle_y(&opened, 10) - middle_y(&opened, 1)).abs() < 0.5);

    // One closes in the middle: the others stay in their rows, but for the one that made the
    // top row long, which may take the room.
    items.retain(|(item, _)| *item.window_id() != 5);
    let closed = GridLayout::compute(&items, area, &config, Some(&opened));
    let left: Vec<usize> = (1..=9).filter(|&id| id != 5).collect();
    assert_eq!(same_row(&closed, &left), same_row(&opened, &left));
}
