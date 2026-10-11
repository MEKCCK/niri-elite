use client::ClientId;
use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::Layer;
use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::Anchor;
use smithay::reexports::wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1;
use smithay::utils::Point;
use wayland_client::protocol::wl_pointer;

use super::*;
use crate::tests::client::LayerConfigureProps;

// Sets up a fixture with one output, a window, a 50 px tall bar on the top layer, the grid
// overview open, and a virtual pointer.
fn set_up() -> (Fixture, ClientId, ZwlrVirtualPointerV1) {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();

    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.roundtrip(id);

    let window = f.client(id).window(&surface);
    window.attach_new_buffer();
    window.set_size(100, 100);
    window.ack_last_and_commit();
    f.double_roundtrip(id);

    let layer = f.client(id).create_layer(None, Layer::Top, "bar");
    let surface = layer.surface.clone();
    layer.set_configure_props(LayerConfigureProps {
        anchor: Some(Anchor::Left | Anchor::Right | Anchor::Top),
        size: Some((0, 50)),
        exclusive_zone: Some(50),
        ..Default::default()
    });
    layer.commit();
    f.roundtrip(id);

    let layer = f.client(id).layer(&surface);
    layer.attach_new_buffer();
    layer.set_size(1920, 50);
    layer.ack_last_and_commit();
    f.double_roundtrip(id);

    assert!(f.niri().layout.open_grid_overview());
    f.niri_complete_animations();

    let client = f.client(id);
    let manager = client.state.virtual_pointer_manager.as_ref().unwrap();
    let pointer = manager.create_virtual_pointer(None, &client.qh, ());
    f.roundtrip(id);

    (f, id, pointer)
}

fn active_workspace_idx(f: &mut Fixture) -> usize {
    let layout = &f.niri().layout;
    let id = layout.active_workspace().unwrap().id();
    layout.find_workspace_by_id(id).unwrap().0
}

// Scrolls the mouse wheel one notch down at the given position.
fn wheel_down_at(f: &mut Fixture, id: ClientId, pointer: &ZwlrVirtualPointerV1, pos: (f64, f64)) {
    f.niri_state().move_cursor(Point::from(pos));

    pointer.axis_discrete(0, wl_pointer::Axis::VerticalScroll, 15., 1);
    pointer.axis_source(wl_pointer::AxisSource::Wheel);
    pointer.frame();
    f.roundtrip(id);
}

#[test]
fn wheel_over_bar_does_not_switch_workspace() {
    let (mut f, id, pointer) = set_up();
    assert_eq!(active_workspace_idx(&mut f), 0);

    wheel_down_at(&mut f, id, &pointer, (960., 25.));
    assert_eq!(active_workspace_idx(&mut f), 0);
}

#[test]
fn wheel_over_grid_switches_workspace() {
    let (mut f, id, pointer) = set_up();
    assert_eq!(active_workspace_idx(&mut f), 0);

    wheel_down_at(&mut f, id, &pointer, (960., 600.));
    assert_eq!(active_workspace_idx(&mut f), 1);
}
