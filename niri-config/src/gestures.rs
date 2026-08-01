use crate::utils::MergeWith;
use crate::FloatOrInt;

#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Gestures {
    pub dnd_edge_view_scroll: DndEdgeViewScroll,
    pub dnd_edge_workspace_switch: DndEdgeWorkspaceSwitch,
    pub hot_corners: HotCorners,
}

#[derive(knuffel::Decode, Debug, Default, Clone, Copy, PartialEq)]
pub struct GesturesPart {
    #[knuffel(child)]
    pub dnd_edge_view_scroll: Option<DndEdgeViewScrollPart>,
    #[knuffel(child)]
    pub dnd_edge_workspace_switch: Option<DndEdgeWorkspaceSwitchPart>,
    #[knuffel(child)]
    pub hot_corners: Option<HotCorners>,
}

impl MergeWith<GesturesPart> for Gestures {
    fn merge_with(&mut self, part: &GesturesPart) {
        merge!(
            (self, part),
            dnd_edge_view_scroll,
            dnd_edge_workspace_switch,
        );
        merge_clone!((self, part), hot_corners);
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DndEdgeViewScroll {
    pub trigger_width: f64,
    pub delay_ms: u16,
    pub max_speed: f64,
}

impl Default for DndEdgeViewScroll {
    fn default() -> Self {
        Self {
            trigger_width: 30., // Taken from GTK 4.
            delay_ms: 100,
            max_speed: 1500.,
        }
    }
}

#[derive(knuffel::Decode, Debug, Clone, Copy, PartialEq)]
pub struct DndEdgeViewScrollPart {
    #[knuffel(child, unwrap(argument))]
    pub trigger_width: Option<FloatOrInt<0, 65535>>,
    #[knuffel(child, unwrap(argument))]
    pub delay_ms: Option<u16>,
    #[knuffel(child, unwrap(argument))]
    pub max_speed: Option<FloatOrInt<0, 1_000_000>>,
}

impl MergeWith<DndEdgeViewScrollPart> for DndEdgeViewScroll {
    fn merge_with(&mut self, part: &DndEdgeViewScrollPart) {
        merge!((self, part), trigger_width, max_speed);
        merge_clone!((self, part), delay_ms);
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DndEdgeWorkspaceSwitch {
    pub trigger_height: f64,
    pub delay_ms: u16,
    pub max_speed: f64,
}

impl Default for DndEdgeWorkspaceSwitch {
    fn default() -> Self {
        Self {
            trigger_height: 50.,
            delay_ms: 100,
            max_speed: 1500.,
        }
    }
}

#[derive(knuffel::Decode, Debug, Clone, Copy, PartialEq)]
pub struct DndEdgeWorkspaceSwitchPart {
    #[knuffel(child, unwrap(argument))]
    pub trigger_height: Option<FloatOrInt<0, 65535>>,
    #[knuffel(child, unwrap(argument))]
    pub delay_ms: Option<u16>,
    #[knuffel(child, unwrap(argument))]
    pub max_speed: Option<FloatOrInt<0, 1_000_000>>,
}

impl MergeWith<DndEdgeWorkspaceSwitchPart> for DndEdgeWorkspaceSwitch {
    fn merge_with(&mut self, part: &DndEdgeWorkspaceSwitchPart) {
        merge!((self, part), trigger_height, max_speed);
        merge_clone!((self, part), delay_ms);
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct HotCorners {
    pub off: bool,
    pub top_left: Option<HotCornerAction>,
    pub top_right: Option<HotCornerAction>,
    pub bottom_left: Option<HotCornerAction>,
    pub bottom_right: Option<HotCornerAction>,
}

#[derive(knuffel::DecodeScalar, Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotCornerAction {
    Overview,
    GridOverview,
}

impl HotCorners {
    pub fn has_any_corner(self) -> bool {
        self.top_left.is_some()
            || self.top_right.is_some()
            || self.bottom_left.is_some()
            || self.bottom_right.is_some()
    }

    pub fn with_default_corners(self) -> Self {
        if self.off || self.has_any_corner() {
            return self;
        }

        Self {
            top_left: Some(HotCornerAction::Overview),
            bottom_left: Some(HotCornerAction::GridOverview),
            ..self
        }
    }
}

impl<S> knuffel::Decode<S> for HotCorners
where
    S: knuffel::traits::ErrorSpan,
{
    fn decode_node(
        node: &knuffel::ast::SpannedNode<S>,
        ctx: &mut knuffel::decode::Context<S>,
    ) -> Result<Self, knuffel::errors::DecodeError<S>> {
        if let Some(type_name) = &node.type_name {
            ctx.emit_error(knuffel::errors::DecodeError::unexpected(
                type_name,
                "type name",
                "no type name expected for this node",
            ));
        }

        for val in node.arguments.iter() {
            ctx.emit_error(knuffel::errors::DecodeError::unexpected(
                &val.literal,
                "argument",
                "no arguments expected for this node",
            ));
        }

        for name in node.properties.keys() {
            ctx.emit_error(knuffel::errors::DecodeError::unexpected(
                name,
                "property",
                "no properties expected for this node",
            ));
        }

        let mut rv = HotCorners::default();

        for child in node.children() {
            let action = match &**child.node_name {
                "off" => {
                    if let Some(type_name) = &child.type_name {
                        ctx.emit_error(knuffel::errors::DecodeError::unexpected(
                            type_name,
                            "type name",
                            "no type name expected for this node",
                        ));
                    }
                    for val in child.arguments.iter() {
                        ctx.emit_error(knuffel::errors::DecodeError::unexpected(
                            &val.literal,
                            "argument",
                            "no arguments expected for this node",
                        ));
                    }
                    for name in child.properties.keys() {
                        ctx.emit_error(knuffel::errors::DecodeError::unexpected(
                            name,
                            "property",
                            "no properties expected for this node",
                        ));
                    }
                    for grandchild in child.children() {
                        ctx.emit_error(knuffel::errors::DecodeError::unexpected(
                            grandchild,
                            "node",
                            "no child nodes expected for this node",
                        ));
                    }
                    rv.off = true;
                    continue;
                }
                "top-left" | "top-right" | "bottom-left" | "bottom-right" => {
                    if let Some(type_name) = &child.type_name {
                        ctx.emit_error(knuffel::errors::DecodeError::unexpected(
                            type_name,
                            "type name",
                            "no type name expected for this node",
                        ));
                    }
                    for name in child.properties.keys() {
                        ctx.emit_error(knuffel::errors::DecodeError::unexpected(
                            name,
                            "property",
                            "no properties expected for this node",
                        ));
                    }
                    let mut args = child.arguments.iter();
                    let action = if let Some(arg) = args.next() {
                        let action = knuffel::traits::DecodeScalar::decode(arg, ctx)?;
                        if let Some(arg) = args.next() {
                            ctx.emit_error(knuffel::errors::DecodeError::unexpected(
                                &arg.literal,
                                "argument",
                                "unexpected argument",
                            ));
                        }
                        for grandchild in child.children() {
                            ctx.emit_error(knuffel::errors::DecodeError::unexpected(
                                grandchild,
                                "node",
                                "unexpected node",
                            ));
                        }
                        action
                    } else if let Some(grandchild) = child.children().next() {
                        let action = match &**grandchild.node_name {
                            "overview" => HotCornerAction::Overview,
                            "grid-overview" => HotCornerAction::GridOverview,
                            _ => {
                                ctx.emit_error(knuffel::errors::DecodeError::unexpected(
                                    grandchild,
                                    "node",
                                    format!(
                                        "unexpected hot corner action `{}`",
                                        grandchild.node_name.escape_default()
                                    ),
                                ));
                                HotCornerAction::Overview
                            }
                        };
                        if let Some(type_name) = &grandchild.type_name {
                            ctx.emit_error(knuffel::errors::DecodeError::unexpected(
                                type_name,
                                "type name",
                                "no type name expected for this node",
                            ));
                        }
                        for val in grandchild.arguments.iter() {
                            ctx.emit_error(knuffel::errors::DecodeError::unexpected(
                                &val.literal,
                                "argument",
                                "no arguments expected for this node",
                            ));
                        }
                        for name in grandchild.properties.keys() {
                            ctx.emit_error(knuffel::errors::DecodeError::unexpected(
                                name,
                                "property",
                                "no properties expected for this node",
                            ));
                        }
                        for child in grandchild.children() {
                            ctx.emit_error(knuffel::errors::DecodeError::unexpected(
                                child,
                                "node",
                                "no child nodes expected for this node",
                            ));
                        }
                        for grandchild in child.children().skip(1) {
                            ctx.emit_error(knuffel::errors::DecodeError::unexpected(
                                grandchild,
                                "node",
                                "unexpected node",
                            ));
                        }
                        action
                    } else {
                        HotCornerAction::Overview
                    };
                    action
                }
                _ => {
                    ctx.emit_error(knuffel::errors::DecodeError::unexpected(
                        child,
                        "node",
                        format!(
                            "unexpected hot corner `{}`",
                            child.node_name.escape_default()
                        ),
                    ));
                    continue;
                }
            };

            match &**child.node_name {
                "top-left" => rv.top_left = Some(action),
                "top-right" => rv.top_right = Some(action),
                "bottom-left" => rv.bottom_left = Some(action),
                "bottom-right" => rv.bottom_right = Some(action),
                _ => unreachable!(),
            }
        }

        Ok(rv)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[track_caller]
    fn parse_hot_corners(text: &str) -> HotCorners {
        let part: GesturesPart = knuffel::parse("test.kdl", text)
            .map_err(miette::Report::new)
            .unwrap();
        part.hot_corners.unwrap()
    }

    #[test]
    fn hot_corner_without_action_defaults_to_overview() {
        let hot_corners = parse_hot_corners(
            r#"
            hot-corners {
                top-left
            }
            "#,
        );

        assert_eq!(hot_corners.top_left, Some(HotCornerAction::Overview));
        assert_eq!(hot_corners.bottom_left, None);
    }

    #[test]
    fn hot_corner_can_bind_grid_overview_with_child_action() {
        let hot_corners = parse_hot_corners(
            r#"
            hot-corners {
                bottom-left { grid-overview; }
            }
            "#,
        );

        assert_eq!(hot_corners.top_left, None);
        assert_eq!(hot_corners.bottom_left, Some(HotCornerAction::GridOverview));
    }

    #[test]
    fn hot_corner_can_bind_grid_overview_with_string_argument() {
        let hot_corners = parse_hot_corners(
            r#"
            hot-corners {
                bottom-left "grid-overview"
            }
            "#,
        );

        assert_eq!(hot_corners.top_left, None);
        assert_eq!(hot_corners.bottom_left, Some(HotCornerAction::GridOverview));
    }

    #[test]
    fn hot_corner_defaults_to_top_left_and_bottom_left() {
        let hot_corners = HotCorners::default().with_default_corners();

        assert_eq!(hot_corners.top_left, Some(HotCornerAction::Overview));
        assert_eq!(hot_corners.top_right, None);
        assert_eq!(hot_corners.bottom_left, Some(HotCornerAction::GridOverview));
        assert_eq!(hot_corners.bottom_right, None);
    }

    #[test]
    fn explicit_hot_corner_disables_implicit_defaults() {
        let hot_corners = parse_hot_corners(
            r#"
            hot-corners {
                top-right { overview; }
            }
            "#,
        )
        .with_default_corners();

        assert_eq!(hot_corners.top_left, None);
        assert_eq!(hot_corners.top_right, Some(HotCornerAction::Overview));
        assert_eq!(hot_corners.bottom_left, None);
        assert_eq!(hot_corners.bottom_right, None);
    }
}
