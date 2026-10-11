use client::ClientId;
use niri_config::Config;
use smithay::reexports::wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1;
use smithay::desktop::Window;
use smithay::utils::Point;
use wayland_client::protocol::wl_surface::WlSurface;

use super::*;

// Sets up a fixture with two outputs side by side, a 100×100 window on the first one, and a
// virtual pointer in the middle of the first output.
fn set_up(confine: bool) -> (Fixture, ClientId, WlSurface, ZwlrVirtualPointerV1) {
    let mut config = Config::default();
    config.input.confine_mouse_to_fullscreen = confine;
    let mut f = Fixture::with_config(config);
    f.add_output(1, (1920, 1080));
    f.add_output(2, (1280, 720));

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

    let client = f.client(id);
    let manager = client.state.virtual_pointer_manager.as_ref().unwrap();
    let pointer = manager.create_virtual_pointer(None, &client.qh, ());
    f.roundtrip(id);

    f.niri_state().move_cursor(Point::from((960., 540.)));

    (f, id, surface, pointer)
}

fn add_window(f: &mut Fixture, id: ClientId) -> WlSurface {
    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.roundtrip(id);

    let window = f.client(id).window(&surface);
    window.attach_new_buffer();
    window.set_size(100, 100);
    window.ack_last_and_commit();
    f.double_roundtrip(id);

    surface
}

fn ack_fullscreen_size(f: &mut Fixture, id: ClientId, surface: &WlSurface) {
    let window = f.client(id).window(surface);
    window.set_size(1920, 1080);
    window.ack_last_and_commit();
    f.double_roundtrip(id);

    f.niri_complete_animations();
}

fn fullscreen(f: &mut Fixture, id: ClientId, surface: &WlSurface) {
    let window = f.client(id).window(surface);
    window.set_fullscreen(None);
    f.double_roundtrip(id);

    ack_fullscreen_size(f, id, surface);
}

fn focused_window(f: &mut Fixture) -> Window {
    f.niri().layout.focus().unwrap().window.clone()
}

fn move_pointer(f: &mut Fixture, id: ClientId, pointer: &ZwlrVirtualPointerV1, dx: f64) -> f64 {
    pointer.motion(0, dx, 0.);
    pointer.frame();
    f.roundtrip(id);

    f.niri().seat.get_pointer().unwrap().current_location().x
}

#[test]
fn stays_on_fullscreen_output() {
    let (mut f, id, surface, pointer) = set_up(true);
    fullscreen(&mut f, id, &surface);

    // Without the setting, this would land on the second output at 1960.
    assert_eq!(move_pointer(&mut f, id, &pointer, 1000.), 1919.);
    // Moving within the output still works.
    assert_eq!(move_pointer(&mut f, id, &pointer, -919.), 1000.);
}

#[test]
fn fullscreen_window_action_confines_too() {
    let (mut f, id, surface, pointer) = set_up(true);

    // What the fullscreen-window bind does, as opposed to the client asking for fullscreen.
    let window = focused_window(&mut f);
    f.niri().layout.toggle_fullscreen(&window);
    f.double_roundtrip(id);
    ack_fullscreen_size(&mut f, id, &surface);

    assert_eq!(move_pointer(&mut f, id, &pointer, 1000.), 1919.);
}

#[test]
fn windowed_fullscreen_lets_pointer_leave() {
    let (mut f, id, surface, pointer) = set_up(true);

    let window = focused_window(&mut f);
    f.niri().layout.toggle_windowed_fullscreen(&window);
    f.double_roundtrip(id);
    f.client(id).window(&surface).ack_last_and_commit();
    f.double_roundtrip(id);
    f.niri_complete_animations();

    assert_eq!(move_pointer(&mut f, id, &pointer, 1000.), 1960.);
}

#[test]
fn focusing_another_column_lets_pointer_leave() {
    let (mut f, id, surface, pointer) = set_up(true);
    add_window(&mut f, id);

    f.niri().layout.focus_left();
    fullscreen(&mut f, id, &surface);
    assert_eq!(move_pointer(&mut f, id, &pointer, 1000.), 1919.);

    // Scroll over to the other window.
    f.niri().layout.focus_right();
    f.niri_complete_animations();
    assert_eq!(move_pointer(&mut f, id, &pointer, 100.), 2019.);

    // Coming back to the fullscreen window on the first output confines again.
    f.niri_state().move_cursor(Point::from((960., 540.)));
    f.niri_focus_output(1);
    f.niri().layout.focus_left();
    f.niri_complete_animations();
    assert_eq!(move_pointer(&mut f, id, &pointer, 1000.), 1919.);
}

#[test]
fn focused_floating_window_lets_pointer_leave() {
    let (mut f, id, surface, pointer) = set_up(true);
    add_window(&mut f, id);

    let floating = focused_window(&mut f);
    f.niri().layout.toggle_window_floating(Some(&floating));
    f.niri().layout.focus_tiling();
    fullscreen(&mut f, id, &surface);
    assert_eq!(move_pointer(&mut f, id, &pointer, 1000.), 1919.);

    f.niri().layout.focus_floating();
    f.niri_complete_animations();
    assert!(f.niri().layout.focus().unwrap().window == floating);
    assert_eq!(move_pointer(&mut f, id, &pointer, 100.), 2019.);
}

#[test]
fn setting_off_lets_pointer_leave() {
    let (mut f, id, surface, pointer) = set_up(false);
    fullscreen(&mut f, id, &surface);

    assert_eq!(move_pointer(&mut f, id, &pointer, 1000.), 1960.);
}

#[test]
fn not_fullscreen_lets_pointer_leave() {
    let (mut f, id, _surface, pointer) = set_up(true);

    assert_eq!(move_pointer(&mut f, id, &pointer, 1000.), 1960.);
}

#[test]
fn unfocused_fullscreen_output_lets_pointer_leave() {
    let (mut f, id, surface, pointer) = set_up(true);
    fullscreen(&mut f, id, &surface);

    // The pointer is still on the first output, but the focus went to the second one.
    f.niri_focus_output(2);

    assert_eq!(move_pointer(&mut f, id, &pointer, 1000.), 1960.);
}

#[test]
fn overview_lets_pointer_leave() {
    let (mut f, id, surface, pointer) = set_up(true);
    fullscreen(&mut f, id, &surface);

    f.niri().layout.toggle_overview();
    f.niri_complete_animations();

    assert_eq!(move_pointer(&mut f, id, &pointer, 1000.), 1960.);
}

#[test]
fn unfullscreen_lets_pointer_leave() {
    let (mut f, id, surface, pointer) = set_up(true);
    fullscreen(&mut f, id, &surface);
    assert_eq!(move_pointer(&mut f, id, &pointer, 1000.), 1919.);

    let window = f.client(id).window(&surface);
    window.unset_fullscreen();
    f.double_roundtrip(id);

    let window = f.client(id).window(&surface);
    window.set_size(100, 100);
    window.ack_last_and_commit();
    f.double_roundtrip(id);
    f.niri_complete_animations();

    assert_eq!(move_pointer(&mut f, id, &pointer, 100.), 2019.);
}

#[test]
fn grid_overview_lets_pointer_leave() {
    let (mut f, id, surface, pointer) = set_up(true);
    fullscreen(&mut f, id, &surface);

    assert!(f.niri().layout.open_grid_overview());
    f.niri_complete_animations();

    assert_eq!(move_pointer(&mut f, id, &pointer, 1000.), 1960.);
}
