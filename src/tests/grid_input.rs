use client::ClientId;
use niri_config::Config;
use smithay::backend::input::{InputEvent, InputTime, KeyState};
use smithay::desktop::Window;
use smithay::reexports::wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1;
use smithay::utils::{Logical, Point};
use wayland_client::protocol::wl_pointer;
use wayland_client::protocol::wl_surface::WlSurface;

use super::*;
use crate::tests::client::KeyboardEvent;
use crate::tests::test_input_backend::{TestInputBackend, TestKeyboardKeyEvent};

const BTN_MIDDLE: u32 = 0x112;
const KEY_ENTER: u32 = 28;

fn add_window(f: &mut Fixture, id: ClientId, title: Option<&str>) -> WlSurface {
    let window = f.client(id).create_window();
    if let Some(title) = title {
        window.set_title(title);
    }
    let surface = window.surface.clone();
    window.commit();
    f.roundtrip(id);

    let window = f.client(id).window(&surface);
    window.attach_new_buffer();
    window.set_size(200, 200);
    window.ack_last_and_commit();
    f.double_roundtrip(id);

    surface
}

struct SetUp {
    f: Fixture,
    id: ClientId,
    pointer: ZwlrVirtualPointerV1,
    tiled: Window,
    tiled_surface: WlSurface,
    ignored: Window,
    ignored_surface: WlSurface,
}

// A tiled window and a floating ignore-grid-overview window, with the grid open.
fn set_up() -> SetUp {
    let config = Config::parse_mem(
        r##"
        window-rule {
            match title="ignored"
            open-floating true
            ignore-grid-overview true
        }
        "##,
    )
    .unwrap();
    let mut f = Fixture::with_config(config);
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let tiled_surface = add_window(&mut f, id, None);
    let ignored_surface = add_window(&mut f, id, Some("ignored"));

    let client = f.client(id);
    let manager = client.state.virtual_pointer_manager.as_ref().unwrap();
    let pointer = manager.create_virtual_pointer(None, &client.qh, ());
    f.roundtrip(id);

    let niri = f.niri();
    let window = |floating: bool| {
        niri.layout
            .windows()
            .find(|(_, mapped)| mapped.is_floating() == floating)
            .map(|(_, mapped)| mapped.window.clone())
            .unwrap()
    };
    let (tiled, ignored) = (window(false), window(true));
    assert!(niri.layout.open_grid_overview());
    assert!(niri.layout.window_is_in_open_grid_overview(&tiled));
    assert!(!niri.layout.window_is_in_open_grid_overview(&ignored));
    niri.clock.set_complete_instantly(true);
    niri.advance_animations();
    niri.clock.set_complete_instantly(false);

    SetUp {
        f,
        id,
        pointer,
        tiled,
        tiled_surface,
        ignored,
        ignored_surface,
    }
}

// Finds a point on the window as it is shown on screen right now.
fn point_on(f: &mut Fixture, window: &Window) -> Point<f64, Logical> {
    let niri = f.niri();
    // Outputs are 1920×1080, laid out left to right.
    for y in (0..1080).step_by(10) {
        for x in (0..1920 * 2).step_by(10) {
            let pos = Point::from((x as f64, y as f64)) + Point::from((20., 20.));
            if niri.window_under(pos).is_some_and(|m| &m.window == window) {
                return pos;
            }
        }
    }
    panic!("window not found on screen");
}

fn press_key(f: &mut Fixture, name: &str, key_state: KeyState) {
    let state = f.niri_state();
    let keyboard = state.niri.seat.get_keyboard().unwrap();
    let code = keyboard.with_xkb_state(state, |xkb| {
        let xkb = xkb.xkb().lock().unwrap();
        let keymap = unsafe { xkb.keymap() };
        keymap.key_by_name(name).unwrap()
    });
    f.niri_state()
        .process_input_event(InputEvent::<TestInputBackend>::Keyboard {
            event: TestKeyboardKeyEvent {
                time: InputTime::from_micros(0),
                code,
                state: key_state,
                count: 1,
            },
        });
}

fn tap_key(f: &mut Fixture, id: ClientId, name: &str) {
    press_key(f, name, KeyState::Pressed);
    press_key(f, name, KeyState::Released);
    f.roundtrip(id);
}

fn middle_click(f: &mut Fixture, id: ClientId, pointer: &ZwlrVirtualPointerV1) {
    pointer.button(0, BTN_MIDDLE, wl_pointer::ButtonState::Pressed);
    pointer.frame();
    pointer.button(0, BTN_MIDDLE, wl_pointer::ButtonState::Released);
    pointer.frame();
    f.double_roundtrip(id);
}

#[test]
fn ignored_window_gets_its_return_key() {
    let SetUp {
        mut f,
        id,
        ignored,
        ignored_surface,
        ..
    } = set_up();

    f.niri().layout.activate_window(&ignored);
    f.niri_state().update_keyboard_focus();
    f.roundtrip(id);
    let _ = f.client(id).state.recent_keyboard_events(&ignored_surface);

    tap_key(&mut f, id, "RTRN");

    // Return used to confirm the grid selection instead of reaching the window.
    assert!(f.niri().layout.is_grid_overview_open());
    let got_return = f
        .client(id)
        .state
        .recent_keyboard_events(&ignored_surface)
        .any(|event| matches!(event, KeyboardEvent::Key { key: KEY_ENTER, .. }));
    assert!(got_return);
}

#[test]
fn middle_click_on_ignored_window_doesnt_close_it() {
    let SetUp {
        mut f,
        id,
        pointer,
        ignored,
        ignored_surface,
        ..
    } = set_up();

    let pos = point_on(&mut f, &ignored);
    f.niri_state().move_cursor(pos);
    middle_click(&mut f, id, &pointer);

    assert!(!f.client(id).window(&ignored_surface).close_requested);
}

#[test]
fn middle_click_on_grid_cell_still_closes_it() {
    let SetUp {
        mut f,
        id,
        pointer,
        tiled,
        tiled_surface,
        ..
    } = set_up();

    let pos = point_on(&mut f, &tiled);
    f.niri_state().move_cursor(pos);
    middle_click(&mut f, id, &pointer);

    assert!(f.client(id).window(&tiled_surface).close_requested);
}

#[test]
fn wheel_over_ignored_window_doesnt_switch_workspace() {
    let SetUp {
        mut f,
        id,
        pointer,
        ignored,
        ..
    } = set_up();

    let pos = point_on(&mut f, &ignored);
    let ws_before = f.niri().layout.active_workspace().unwrap().id();
    f.niri_state().move_cursor(pos);
    pointer.axis_discrete(0, wl_pointer::Axis::VerticalScroll, 15., 1);
    pointer.axis_source(wl_pointer::AxisSource::Wheel);
    pointer.frame();
    f.roundtrip(id);

    assert_eq!(f.niri().layout.active_workspace().unwrap().id(), ws_before);
}

// Two outputs side by side, each with a window, and the grid open only on the first one.
fn set_up_per_monitor() -> (Fixture, ClientId, ZwlrVirtualPointerV1, Window, WlSurface) {
    let config = Config::parse_mem(
        r##"
        grid-overview {
            grid-all-monitors false
        }
        "##,
    )
    .unwrap();
    let mut f = Fixture::with_config(config);
    f.add_output(1, (1920, 1080));
    f.add_output(2, (1920, 1080));
    let id = f.add_client();
    f.niri_focus_output(1);
    add_window(&mut f, id, None);
    f.niri_focus_output(2);
    let second_surface = add_window(&mut f, id, None);

    let client = f.client(id);
    let manager = client.state.virtual_pointer_manager.as_ref().unwrap();
    let pointer = manager.create_virtual_pointer(None, &client.qh, ());
    f.roundtrip(id);

    f.niri_focus_output(1);
    let output2 = f.niri_output(2);
    let niri = f.niri();
    assert!(niri.layout.open_grid_overview());
    assert!(!niri.layout.is_grid_overview_open_on(&output2));
    let second = niri
        .layout
        .windows()
        .find(|(mon, _)| mon.is_some_and(|mon| mon.output() == &output2))
        .map(|(_, mapped)| mapped.window.clone())
        .unwrap();

    (f, id, pointer, second, second_surface)
}

#[test]
fn middle_click_on_monitor_without_grid_doesnt_close() {
    let (mut f, id, pointer, second, second_surface) = set_up_per_monitor();

    // Used to close the window, because the grid is open on the active monitor.
    let pos = point_on(&mut f, &second);
    f.niri_state().move_cursor(pos);
    middle_click(&mut f, id, &pointer);

    assert!(!f.client(id).window(&second_surface).close_requested);
}

#[test]
fn wheel_on_monitor_without_grid_doesnt_switch_workspace() {
    let (mut f, id, pointer, second, _) = set_up_per_monitor();

    let pos = point_on(&mut f, &second);
    let output2 = f.niri_output(2);
    let ws_before = f
        .niri()
        .layout
        .monitor_for_output(&output2)
        .unwrap()
        .active_workspace_ref()
        .id();
    f.niri_state().move_cursor(pos);
    pointer.axis_discrete(0, wl_pointer::Axis::VerticalScroll, 15., 1);
    pointer.axis_source(wl_pointer::AxisSource::Wheel);
    pointer.frame();
    f.roundtrip(id);

    let ws_after = f
        .niri()
        .layout
        .monitor_for_output(&output2)
        .unwrap()
        .active_workspace_ref()
        .id();
    assert_eq!(ws_after, ws_before);
}

#[test]
fn grid_hot_corner_toggles_its_own_monitor() {
    let (mut f, _id, _pointer, _second, _) = set_up_per_monitor();
    let output2 = f.niri_output(2);

    f.niri().trigger_hot_corner(
        niri_config::gestures::HotCornerAction::GridOverview,
        Some(&output2),
    );

    let output1 = f.niri_output(1);
    let layout = &f.niri().layout;
    assert!(layout.is_grid_overview_open_on(&output1));
    assert!(layout.is_grid_overview_open_on(&output2));
}
