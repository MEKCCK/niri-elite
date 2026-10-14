use std::collections::HashSet;

use knuffel::errors::DecodeError;

use crate::appearance::{Color, WorkspaceShadow, WorkspaceShadowPart, DEFAULT_BACKDROP_COLOR};
use crate::utils::{expect_only_children, parse_arg_node, Flag, MergeWith};
use crate::{Action, Bind, FloatOrInt, Key, Trigger};

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

#[derive(Debug, Clone, PartialEq)]
pub struct Magnifier {
    pub off: bool,
    pub zoom_factor: f64,
    pub track_cursor: bool,
    pub scale_cursor: bool,
    /// Fraction of the visible area that one move-magnifier step moves the view by.
    pub move_step: f64,
    /// Binds that take precedence over the regular binds while the magnifier is on.
    pub binds: Vec<Bind>,
}

impl Default for Magnifier {
    fn default() -> Self {
        Self {
            off: false,
            zoom_factor: 2.0,
            track_cursor: true,
            scale_cursor: true,
            move_step: 0.05,
            binds: Vec::new(),
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
    #[knuffel(child, unwrap(argument))]
    pub move_step: Option<FloatOrInt<0, 1>>,
    #[knuffel(child)]
    pub binds: Option<MagnifierBinds>,
}

impl MergeWith<MagnifierPart> for Magnifier {
    fn merge_with(&mut self, part: &MagnifierPart) {
        self.off |= part.off;
        if part.on {
            self.off = false;
        }
        merge!(
            (self, part),
            zoom_factor,
            track_cursor,
            scale_cursor,
            move_step
        );

        if let Some(part) = &part.binds {
            // Remove existing binds matching any new bind.
            self.binds
                .retain(|bind| !part.0.iter().any(|new| new.key == bind.key));
            // Add all new binds.
            self.binds.extend(part.0.iter().cloned().map(Bind::from));
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MagnifierBind {
    pub key: Key,
    pub action: MagnifierAction,
    /// Explicit `repeat`; the default depends on the action.
    pub repeat: Option<bool>,
    pub allow_inhibiting: bool,
}

impl From<MagnifierBind> for Bind {
    fn from(x: MagnifierBind) -> Self {
        Self {
            key: x.key,
            action: Action::from(x.action),
            // Key repeat replays the bind, so a repeating toggle would turn the magnifier back on.
            repeat: x
                .repeat
                .unwrap_or(x.action != MagnifierAction::ToggleMagnifier),
            cooldown: None,
            allow_when_locked: false,
            allow_inhibiting: x.allow_inhibiting,
            hotkey_overlay_title: None,
        }
    }
}

/// Actions allowed in the magnifier binds.
#[derive(knuffel::Decode, Debug, Clone, Copy, PartialEq, Eq)]
pub enum MagnifierAction {
    MoveMagnifierLeft,
    MoveMagnifierRight,
    MoveMagnifierUp,
    MoveMagnifierDown,
    ToggleMagnifier,
}

impl From<MagnifierAction> for Action {
    fn from(x: MagnifierAction) -> Self {
        match x {
            MagnifierAction::MoveMagnifierLeft => Self::MoveMagnifierLeft,
            MagnifierAction::MoveMagnifierRight => Self::MoveMagnifierRight,
            MagnifierAction::MoveMagnifierUp => Self::MoveMagnifierUp,
            MagnifierAction::MoveMagnifierDown => Self::MoveMagnifierDown,
            MagnifierAction::ToggleMagnifier => Self::ToggleMagnifier,
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct MagnifierBinds(pub Vec<MagnifierBind>);

impl<S> knuffel::Decode<S> for MagnifierBinds
where
    S: knuffel::traits::ErrorSpan,
{
    fn decode_node(
        node: &knuffel::ast::SpannedNode<S>,
        ctx: &mut knuffel::decode::Context<S>,
    ) -> Result<Self, DecodeError<S>> {
        expect_only_children(node, ctx);

        let mut seen_keys = HashSet::new();

        let mut binds = Vec::new();

        for child in node.children() {
            match MagnifierBind::decode_node(child, ctx) {
                Ok(bind) => {
                    if !seen_keys.insert(bind.key) {
                        ctx.emit_error(DecodeError::unexpected(
                            &child.node_name,
                            "keybind",
                            "duplicate keybind",
                        ));
                        continue;
                    }

                    binds.push(bind);
                }
                Err(e) => {
                    ctx.emit_error(e);
                }
            }
        }

        Ok(Self(binds))
    }
}

impl<S> knuffel::Decode<S> for MagnifierBind
where
    S: knuffel::traits::ErrorSpan,
{
    fn decode_node(
        node: &knuffel::ast::SpannedNode<S>,
        ctx: &mut knuffel::decode::Context<S>,
    ) -> Result<Self, DecodeError<S>> {
        if let Some(type_name) = &node.type_name {
            ctx.emit_error(DecodeError::unexpected(
                type_name,
                "type name",
                "no type name expected for this node",
            ));
        }

        for val in node.arguments.iter() {
            ctx.emit_error(DecodeError::unexpected(
                &val.literal,
                "argument",
                "no arguments expected for this node",
            ));
        }

        let key = node
            .node_name
            .parse::<Key>()
            .map_err(|e| DecodeError::conversion(&node.node_name, e.wrap_err("invalid keybind")))?;

        // FIXME: To support this, all the mods_with_mouse_binds()/mods_with_wheel_binds()/etc.
        // will need to learn about magnifier bindings.
        if !matches!(key.trigger, Trigger::Keysym(_)) {
            ctx.emit_error(DecodeError::unexpected(
                &node.node_name,
                "key",
                "key must be a keyboard key (others are unsupported here for now)",
            ));
        }

        let mut repeat = None;
        let mut allow_inhibiting = true;
        for (name, val) in &node.properties {
            match &***name {
                "repeat" => {
                    repeat = Some(knuffel::traits::DecodeScalar::decode(val, ctx)?);
                }
                "allow-inhibiting" => {
                    allow_inhibiting = knuffel::traits::DecodeScalar::decode(val, ctx)?;
                }
                name_str => {
                    ctx.emit_error(DecodeError::unexpected(
                        name,
                        "property",
                        format!("unexpected property `{}`", name_str.escape_default()),
                    ));
                }
            }
        }

        let mut children = node.children();

        // If the action is invalid but the key is fine, we still want to return something.
        // That way, the parent can handle the existence of duplicate keybinds,
        // even if their contents are not valid.
        let dummy = Self {
            key,
            action: MagnifierAction::ToggleMagnifier,
            repeat: None,
            allow_inhibiting: true,
        };

        if let Some(child) = children.next() {
            for unwanted_child in children {
                ctx.emit_error(DecodeError::unexpected(
                    unwanted_child,
                    "node",
                    "only one action is allowed per keybind",
                ));
            }
            match MagnifierAction::decode_node(child, ctx) {
                Ok(action) => Ok(Self {
                    key,
                    action,
                    repeat,
                    allow_inhibiting,
                }),
                Err(e) => {
                    ctx.emit_error(e);
                    Ok(dummy)
                }
            }
        } else {
            ctx.emit_error(DecodeError::missing(
                node,
                "expected an action for this keybind",
            ));
            Ok(dummy)
        }
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
    /// On-screen width of the focus ring around the focused cell, in logical pixels.
    ///
    /// `None` keeps the width of the window's own focus ring. Either way, the width doesn't shrink
    /// with the cells.
    pub focus_ring_width: Option<f64>,
    pub focus_ring_transition: GridFocusRingTransition,
    pub focus_follows_mouse: Option<GridFocusFollowsMouse>,
    pub grid_all_monitors: bool,
    pub default_mod_action: bool,
    pub minimized_highlight: GridMinimizedHighlight,
}

/// How the focus ring goes from one grid cell to another.
///
/// Timing comes from `animations { grid-focus-ring }`.
#[derive(knuffel::DecodeScalar, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum GridFocusRingTransition {
    /// The ring appears on the new cell right away.
    None,
    /// The new cell's ring grows out from its edge while the old one shrinks back and fades.
    #[default]
    Grow,
    /// One ring slides from the old cell to the new one.
    Slide,
}

/// Moving the mouse moves the grid focus to the cell under the pointer.
#[derive(knuffel::Decode, Debug, Clone, Copy, PartialEq)]
pub struct GridFocusFollowsMouse {
    /// How far the pointer has to move after the focus was last set some other way, in logical
    /// pixels, before the focus follows it.
    #[knuffel(property)]
    pub min_distance: Option<FloatOrInt<0, 65535>>,
}

impl GridFocusFollowsMouse {
    pub fn min_distance(&self) -> f64 {
        self.min_distance.map_or(8., |d| d.0)
    }
}

/// Highlight drawn behind the grid cell of a minimized window.
///
/// Minimized windows only exist in the Grid Overview, where they otherwise look exactly like
/// normal windows. The highlight extends past the cell on every side, so it reads as a colored
/// frame around the thumbnail without touching the window's own opacity.
///
/// Mirrors the recent-windows [`MruHighlight`](crate::recent_windows::MruHighlight), values
/// included.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridMinimizedHighlight {
    pub off: bool,
    pub color: Color,
    pub urgent_color: Color,
    /// How far the highlight extends past the cell, in logical pixels.
    pub padding: f64,
    pub corner_radius: f64,
}

impl Default for GridMinimizedHighlight {
    fn default() -> Self {
        Self {
            off: false,
            // Recent-windows' grey, at 70% alpha so the frame blends into the backdrop.
            color: Color::from_rgba8_unpremul(153, 153, 153, 179),
            urgent_color: Color::new_unpremul(1., 0.6, 0.6, 1.),
            // Recent-windows uses 30, which is sized for full-screen previews. Grid cells sit a
            // `gap` apart (16 by default), so the frame has to stay inside half of that.
            padding: 6.,
            corner_radius: 0.,
        }
    }
}

#[derive(knuffel::Decode, Debug, Default, Clone, Copy, PartialEq)]
pub struct GridMinimizedHighlightPart {
    #[knuffel(child)]
    pub off: bool,
    #[knuffel(child)]
    pub on: bool,
    #[knuffel(child)]
    pub color: Option<Color>,
    #[knuffel(child)]
    pub urgent_color: Option<Color>,
    #[knuffel(child, unwrap(argument))]
    pub padding: Option<FloatOrInt<0, 65535>>,
    #[knuffel(child, unwrap(argument))]
    pub corner_radius: Option<FloatOrInt<0, 65535>>,
}

impl MergeWith<GridMinimizedHighlightPart> for GridMinimizedHighlight {
    fn merge_with(&mut self, part: &GridMinimizedHighlightPart) {
        self.off |= part.off;
        if part.on {
            self.off = false;
        }
        merge_clone!((self, part), color, urgent_color);
        merge!((self, part), padding, corner_radius);
    }
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
            // The focus ring marks the focused cell; scaling it up would misrepresent the window's
            // size next to the others.
            focused_column_scale: 1.,
            focus_ring_width: None,
            focus_ring_transition: GridFocusRingTransition::default(),
            focus_follows_mouse: None,
            grid_all_monitors: true,
            default_mod_action: true,
            minimized_highlight: GridMinimizedHighlight::default(),
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
    pub focus_ring_width: Option<FloatOrInt<0, 65535>>,
    #[knuffel(child, unwrap(argument))]
    pub focus_ring_transition: Option<GridFocusRingTransition>,
    #[knuffel(child)]
    pub focus_follows_mouse: Option<GridFocusFollowsMouse>,
    #[knuffel(child, unwrap(argument))]
    pub grid_all_monitors: Option<bool>,
    #[knuffel(child, unwrap(argument))]
    pub default_mod_action: Option<bool>,
    #[knuffel(child)]
    pub minimized_highlight: Option<GridMinimizedHighlightPart>,
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
        if let Some(focus_ring_width) = &part.focus_ring_width {
            self.focus_ring_width = Some(focus_ring_width.0);
        }
        if let Some(focus_ring_transition) = part.focus_ring_transition {
            self.focus_ring_transition = focus_ring_transition;
        }
        if let Some(focus_follows_mouse) = part.focus_follows_mouse {
            self.focus_follows_mouse = Some(focus_follows_mouse);
        }
        if let Some(grid_all_monitors) = part.grid_all_monitors {
            self.grid_all_monitors = grid_all_monitors;
        }
        if let Some(default_mod_action) = part.default_mod_action {
            self.default_mod_action = default_mod_action;
        }
        merge!((self, part), minimized_highlight);
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
