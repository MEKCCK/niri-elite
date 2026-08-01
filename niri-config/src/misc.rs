use crate::appearance::{Color, WorkspaceShadow, WorkspaceShadowPart, DEFAULT_BACKDROP_COLOR};
use crate::utils::{parse_arg_node, Flag, MergeWith};
use crate::FloatOrInt;

#[derive(knuffel::Decode, Debug, Clone, PartialEq, Eq)]
pub struct SpawnAtStartup {
    #[knuffel(arguments)]
    pub command: Vec<String>,
}

#[derive(knuffel::Decode, Debug, Clone, PartialEq, Eq)]
pub struct SpawnShAtStartup {
    #[knuffel(argument)]
    pub command: String,
}

#[derive(Debug, PartialEq)]
pub struct Cursor {
    pub xcursor_theme: String,
    pub xcursor_size: u8,
    pub hide_when_typing: bool,
    pub hide_after_inactive_ms: Option<u32>,
    pub shake_to_enlarge: Option<ShakeToEnlarge>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShakeToEnlarge {
    pub off: bool,
    pub zoom_factor: f64,
    pub hold_duration_ms: u32,
    pub threshold: f64,
    pub grow: bool,
    pub grow_speed: f64,
}

impl Default for ShakeToEnlarge {
    fn default() -> Self {
        Self {
            off: false,
            zoom_factor: 5.0,
            hold_duration_ms: 1500,
            threshold: 2000.0,
            grow: false,
            grow_speed: 0.01,
        }
    }
}

#[derive(knuffel::Decode, Debug, Clone, PartialEq)]
pub struct ShakeToEnlargePart {
    #[knuffel(child)]
    pub off: bool,
    #[knuffel(child)]
    pub on: bool,
    #[knuffel(child, unwrap(argument))]
    pub zoom_factor: Option<FloatOrInt<0, { i32::MAX }>>,
    #[knuffel(child, unwrap(argument))]
    pub hold_duration_ms: Option<u32>,
    #[knuffel(child, unwrap(argument))]
    pub threshold: Option<FloatOrInt<0, { i32::MAX }>>,
    #[knuffel(child)]
    pub grow: Option<Flag>,
    #[knuffel(child, unwrap(argument))]
    pub grow_speed: Option<FloatOrInt<0, { i32::MAX }>>,
}

impl MergeWith<ShakeToEnlargePart> for ShakeToEnlarge {
    fn merge_with(&mut self, part: &ShakeToEnlargePart) {
        self.off |= part.off;
        if part.on {
            self.off = false;
        }

        merge!((self, part), zoom_factor, threshold, grow_speed, grow);
        merge_clone!((self, part), hold_duration_ms);
    }
}

impl Default for Cursor {
    fn default() -> Self {
        Self {
            xcursor_theme: String::from("default"),
            xcursor_size: 24,
            hide_when_typing: false,
            hide_after_inactive_ms: None,
            shake_to_enlarge: Some(ShakeToEnlarge::default()),
        }
    }
}

#[derive(knuffel::Decode, Debug, PartialEq)]
pub struct CursorPart {
    #[knuffel(child, unwrap(argument))]
    pub xcursor_theme: Option<String>,
    #[knuffel(child, unwrap(argument))]
    pub xcursor_size: Option<u8>,
    #[knuffel(child)]
    pub hide_when_typing: Option<Flag>,
    #[knuffel(child, unwrap(argument))]
    pub hide_after_inactive_ms: Option<u32>,
    #[knuffel(child)]
    pub shake_to_enlarge: Option<ShakeToEnlargePart>,
}

impl MergeWith<CursorPart> for Cursor {
    fn merge_with(&mut self, part: &CursorPart) {
        merge_clone!((self, part), xcursor_theme, xcursor_size);
        merge!((self, part), hide_when_typing);
        merge_clone_opt!((self, part), hide_after_inactive_ms);
        if let Some(x) = &part.shake_to_enlarge {
            if let Some(s) = &mut self.shake_to_enlarge {
                s.merge_with(x);
            } else {
                self.shake_to_enlarge = Some(ShakeToEnlarge::from_part(x));
            }
        }
    }
}

#[derive(knuffel::Decode, Debug, Clone, PartialEq)]
pub struct ScreenshotPath(#[knuffel(argument)] pub Option<String>);

impl Default for ScreenshotPath {
    fn default() -> Self {
        Self(Some(String::from(
            "~/Pictures/Screenshots/Screenshot from %Y-%m-%d %H-%M-%S.png",
        )))
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct HotkeyOverlay {
    pub skip_at_startup: bool,
    pub hide_not_bound: bool,
}

#[derive(knuffel::Decode, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct HotkeyOverlayPart {
    #[knuffel(child)]
    pub skip_at_startup: Option<Flag>,
    #[knuffel(child)]
    pub hide_not_bound: Option<Flag>,
}

impl MergeWith<HotkeyOverlayPart> for HotkeyOverlay {
    fn merge_with(&mut self, part: &HotkeyOverlayPart) {
        merge!((self, part), skip_at_startup, hide_not_bound);
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ConfigNotification {
    pub disable_failed: bool,
}

#[derive(knuffel::Decode, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ConfigNotificationPart {
    #[knuffel(child)]
    pub disable_failed: Option<Flag>,
}

impl MergeWith<ConfigNotificationPart> for ConfigNotification {
    fn merge_with(&mut self, part: &ConfigNotificationPart) {
        merge!((self, part), disable_failed);
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Clipboard {
    pub disable_primary: bool,
}

#[derive(knuffel::Decode, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ClipboardPart {
    #[knuffel(child)]
    pub disable_primary: Option<Flag>,
}

impl MergeWith<ClipboardPart> for Clipboard {
    fn merge_with(&mut self, part: &ClipboardPart) {
        merge!((self, part), disable_primary);
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Magnifier {
    pub off: bool,
    pub zoom_factor: f64,
    pub track_cursor: bool,
    pub scale_cursor: bool,
}

impl Default for Magnifier {
    fn default() -> Self {
        Self {
            off: false,
            zoom_factor: 2.0,
            track_cursor: true,
            scale_cursor: true,
        }
    }
}

#[derive(knuffel::Decode, Debug, Clone, PartialEq)]
pub struct MagnifierPart {
    #[knuffel(child)]
    pub off: bool,
    #[knuffel(child)]
    pub on: bool,
    #[knuffel(child, unwrap(argument))]
    pub zoom_factor: Option<FloatOrInt<0, { i32::MAX }>>,
    #[knuffel(child)]
    pub track_cursor: Option<Flag>,
    #[knuffel(child)]
    pub scale_cursor: Option<Flag>,
}

impl MergeWith<MagnifierPart> for Magnifier {
    fn merge_with(&mut self, part: &MagnifierPart) {
        self.off |= part.off;
        if part.on {
            self.off = false;
        }
        merge!((self, part), zoom_factor, track_cursor, scale_cursor);
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Overview {
    pub zoom: f64,
    pub backdrop_color: Color,
    pub workspace_shadow: WorkspaceShadow,
}

impl Default for Overview {
    fn default() -> Self {
        Self {
            zoom: 0.5,
            backdrop_color: DEFAULT_BACKDROP_COLOR,
            workspace_shadow: WorkspaceShadow::default(),
        }
    }
}

#[derive(knuffel::Decode, Debug, Clone, Copy, PartialEq)]
pub struct OverviewPart {
    #[knuffel(child, unwrap(argument))]
    pub zoom: Option<FloatOrInt<0, 1>>,
    #[knuffel(child)]
    pub backdrop_color: Option<Color>,
    #[knuffel(child)]
    pub workspace_shadow: Option<WorkspaceShadowPart>,
}

impl MergeWith<OverviewPart> for Overview {
    fn merge_with(&mut self, part: &OverviewPart) {
        merge!((self, part), zoom, workspace_shadow);
        merge_clone!((self, part), backdrop_color);
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridOverview {
    pub gap: f64,
    pub padding: GridOverviewPadding,
    pub min_scale: f64,
    pub focused_column_scale: f64,
    pub grid_all_monitors: bool,
    pub default_mod_action: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridOverviewPadding {
    pub left: f64,
    pub right: f64,
    pub top: f64,
    pub bottom: f64,
}

impl GridOverviewPadding {
    pub fn uniform(value: f64) -> Self {
        Self {
            left: value,
            right: value,
            top: value,
            bottom: value,
        }
    }
}

impl Default for GridOverviewPadding {
    fn default() -> Self {
        Self::uniform(80.)
    }
}

impl Default for GridOverview {
    fn default() -> Self {
        Self {
            gap: 16.,
            padding: GridOverviewPadding::default(),
            min_scale: 0.08,
            focused_column_scale: 1.04,
            grid_all_monitors: true,
            default_mod_action: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GridOverviewPaddingPart {
    Uniform(f64),
    Sides {
        left: Option<f64>,
        right: Option<f64>,
        top: Option<f64>,
        bottom: Option<f64>,
    },
}

impl GridOverviewPaddingPart {
    fn merge_into(&self, padding: &mut GridOverviewPadding) {
        match *self {
            Self::Uniform(value) => *padding = GridOverviewPadding::uniform(value),
            Self::Sides {
                left,
                right,
                top,
                bottom,
            } => {
                if let Some(left) = left {
                    padding.left = left;
                }
                if let Some(right) = right {
                    padding.right = right;
                }
                if let Some(top) = top {
                    padding.top = top;
                }
                if let Some(bottom) = bottom {
                    padding.bottom = bottom;
                }
            }
        }
    }
}

impl<S: knuffel::traits::ErrorSpan> knuffel::Decode<S> for GridOverviewPaddingPart {
    fn decode_node(
        node: &knuffel::ast::SpannedNode<S>,
        ctx: &mut knuffel::decode::Context<S>,
    ) -> Result<Self, knuffel::errors::DecodeError<S>> {
        let mut iter_args = node.arguments.iter();
        if let Some(val) = iter_args.next() {
            let value: FloatOrInt<0, { i32::MAX }> =
                knuffel::traits::DecodeScalar::decode(val, ctx)?;

            if let Some(val) = iter_args.next() {
                ctx.emit_error(knuffel::errors::DecodeError::unexpected(
                    &val.literal,
                    "argument",
                    "unexpected argument",
                ));
            }
            for child in node.children() {
                ctx.emit_error(knuffel::errors::DecodeError::unexpected(
                    child,
                    "node",
                    "no child nodes expected for `padding` with an argument",
                ));
            }
            for name in node.properties.keys() {
                ctx.emit_error(knuffel::errors::DecodeError::unexpected(
                    name,
                    "property",
                    "no properties expected for this node",
                ));
            }

            return Ok(Self::Uniform(value.0));
        }

        let mut left = None;
        let mut right = None;
        let mut top = None;
        let mut bottom = None;

        for child in node.children() {
            let value: FloatOrInt<0, { i32::MAX }> = match &**child.node_name {
                "left" | "right" | "top" | "bottom" => {
                    parse_arg_node(&child.node_name, child, ctx)?
                }
                name => {
                    ctx.emit_error(knuffel::errors::DecodeError::unexpected(
                        child,
                        "node",
                        format!("unknown padding property `{name}`"),
                    ));
                    continue;
                }
            };

            match &**child.node_name {
                "left" => left = Some(value.0),
                "right" => right = Some(value.0),
                "top" => top = Some(value.0),
                "bottom" => bottom = Some(value.0),
                _ => unreachable!(),
            }
        }

        for name in node.properties.keys() {
            ctx.emit_error(knuffel::errors::DecodeError::unexpected(
                name,
                "property",
                "no properties expected for this node",
            ));
        }

        Ok(Self::Sides {
            left,
            right,
            top,
            bottom,
        })
    }
}

#[derive(knuffel::Decode, Debug, Clone, Copy, PartialEq)]
pub struct GridOverviewPart {
    #[knuffel(child, unwrap(argument))]
    pub gap: Option<FloatOrInt<0, { i32::MAX }>>,
    #[knuffel(child)]
    pub padding: Option<GridOverviewPaddingPart>,
    #[knuffel(child, unwrap(argument))]
    pub min_scale: Option<FloatOrInt<0, 1>>,
    #[knuffel(child, unwrap(argument))]
    pub focused_column_scale: Option<FloatOrInt<1, 2>>,
    #[knuffel(child, unwrap(argument))]
    pub grid_all_monitors: Option<bool>,
    #[knuffel(child, unwrap(argument))]
    pub default_mod_action: Option<bool>,
}

impl MergeWith<GridOverviewPart> for GridOverview {
    fn merge_with(&mut self, part: &GridOverviewPart) {
        if let Some(gap) = &part.gap {
            self.gap = gap.0;
        }
        if let Some(padding) = &part.padding {
            padding.merge_into(&mut self.padding);
        }
        if let Some(min_scale) = &part.min_scale {
            self.min_scale = min_scale.0;
        }
        if let Some(focused_column_scale) = &part.focused_column_scale {
            self.focused_column_scale = focused_column_scale.0;
        }
        if let Some(grid_all_monitors) = part.grid_all_monitors {
            self.grid_all_monitors = grid_all_monitors;
        }
        if let Some(default_mod_action) = part.default_mod_action {
            self.default_mod_action = default_mod_action;
        }
    }
}

#[derive(knuffel::Decode, Debug, Default, Clone, PartialEq, Eq)]
pub struct Environment(#[knuffel(children)] pub Vec<EnvironmentVariable>);

#[derive(knuffel::Decode, Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentVariable {
    #[knuffel(node_name)]
    pub name: String,
    #[knuffel(argument)]
    pub value: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XwaylandSatellite {
    pub off: bool,
    pub path: String,
}

impl Default for XwaylandSatellite {
    fn default() -> Self {
        Self {
            off: false,
            path: String::from("xwayland-satellite"),
        }
    }
}

#[derive(knuffel::Decode, Debug, Clone, PartialEq, Eq)]
pub struct XwaylandSatellitePart {
    #[knuffel(child)]
    pub off: bool,
    #[knuffel(child)]
    pub on: bool,
    #[knuffel(child, unwrap(argument))]
    pub path: Option<String>,
}

impl MergeWith<XwaylandSatellitePart> for XwaylandSatellite {
    fn merge_with(&mut self, part: &XwaylandSatellitePart) {
        self.off |= part.off;
        if part.on {
            self.off = false;
        }

        merge_clone!((self, part), path);
    }
}
