use std::cell::{Cell, OnceCell, RefCell};

use niri_config::utils::{Flag, MergeWith as _};
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
        #[proptest(strategy = "prop::option::of(1..=5usize)")]
        output_name: Option<usize>,
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
                output_name,
                layout_config,
            } => {
                layout.ensure_named_workspace(&WorkspaceConfig {
                    name: WorkspaceName(format!("ws{ws_name}")),
                    open_on_output: output_name.map(|name| format!("output{name}")),
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
            output_name: Some(1),
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
            output_name: Some(1),
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
            output_name: None,
            layout_config: None,
        },
        Op::AddNamedWorkspace {
            ws_name: 2,
            output_name: None,
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
            output_name: None,
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
            output_name: Some(2),
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
            output_name: Some(2),
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
fn grid_overview_floating_uses_blended_tiling_scale_when_mixed() {
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
    let fullscreen_scale = fullscreen.target_scale;
    let floating_scale = floating.target_scale;

    assert!(
        floating_scale > fullscreen_scale,
        "mixed floating grid preview should be allowed to grow above the tiling scale: fullscreen={fullscreen_scale}, floating={floating_scale}"
    );
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

    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow { params: floating },
        Op::ToggleGridOverview,
        Op::CompleteAnimations,
    ]);
    let go = layout
        .active_workspace()
        .and_then(|ws| ws.grid_overview())
        .unwrap();
    let (item, info) = go.layout.entries.first().unwrap();

    approx::assert_abs_diff_eq!(info.target_scale, 1., epsilon = 0.001);
    assert!(go.entry_focus_boost(item, info) > 1.);
}

#[test]
fn grid_overview_does_not_upscale_mixed_floating_window() {
    let mut floating = TestWindowParams::new(2);
    floating.is_floating = true;

    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow { params: floating },
        Op::FocusWindow(2),
        Op::ToggleGridOverview,
        Op::CompleteAnimations,
    ]);
    let go = layout
        .active_workspace()
        .and_then(|ws| ws.grid_overview())
        .unwrap();
    let (item, info) = go
        .layout
        .entries
        .iter()
        .find(|(item, _)| item.window_id() == &2)
        .unwrap();

    assert!(info.target_scale <= 1.);
    assert!(go.entry_focus_boost(item, info) > 1.);
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
    check_ops([
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
    ])
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
fn grid_move_column_keeps_focus_boost_stationary() {
    let mut layout = tabbed_column_and_column_grid();
    assert!(layout
        .active_workspace_mut()
        .unwrap()
        .set_grid_focus_for_window(&1));
    check_ops_on_layout(&mut layout, [Op::CompleteAnimations]);

    layout.move_right();
    layout.verify_invariants();

    let go = layout
        .active_workspace()
        .and_then(|ws| ws.grid_overview())
        .unwrap();
    let (focused_item, focused_info) = go
        .layout
        .entries
        .iter()
        .find(|(item, _)| item.window_id() == &1)
        .unwrap();
    let (swapped_item, swapped_info) = go
        .layout
        .entries
        .iter()
        .find(|(item, _)| item.window_id() == &3)
        .unwrap();

    assert_eq!(go.focused_id(), Some(1));
    assert!(go.focus_boost_anim.is_none());
    assert!(go.entry_focus_boost(focused_item, focused_info) > 1.);
    approx::assert_abs_diff_eq!(go.entry_focus_boost(swapped_item, swapped_info), 1.);
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

    // While the grab is ongoing, the focus belongs to the grabbed window itself, so that no
    // other cell gets a spurious focus boost that would animate away on drop.
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

    assert_ne!(after_communicate, initial_size);
    assert_eq!(after_communicate, after_reopen);
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
    ) -> niri_config::LayoutPart {
        niri_config::LayoutPart {
            gaps,
            struts,
            center_focused_column,
            always_center_single_column,
            empty_workspace_above_first,
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
