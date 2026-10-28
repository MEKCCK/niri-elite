use client::ClientId;
use niri_config::Config;
use smithay::desktop::Window;
use smithay::reexports::wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1;
use smithay::utils::{Logical, Point};

use super::*;

fn add_window(f: &mut Fixture, id: ClientId) {
    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.roundtrip(id);

    let window = f.client(id).window(&surface);
    window.attach_new_buffer();
    window.set_size(100, 100);
    window.ack_last_and_commit();
    f.double_roundtrip(id);
}

struct SetUp {
    f: Fixture,
    id: ClientId,
    pointer: ZwlrVirtualPointerV1,
    windows: Vec<Window>,
}

// Three columns, the first one active, and the grid overview open.
fn set_up(config: &str) -> SetUp {
    let config = Config::parse_mem(config).unwrap();
    let mut f = Fixture::with_config(config);
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    for _ in 0..3 {
        add_window(&mut f, id);
    }

    let client = f.client(id);
    let manager = client.state.virtual_pointer_manager.as_ref().unwrap();
    let pointer = manager.create_virtual_pointer(None, &client.qh, ());
    f.roundtrip(id);

    let niri = f.niri();
    let windows: Vec<_> = niri
        .layout
        .windows()
        .map(|(_, m)| m.window.clone())
        .collect();
    niri.layout.activate_window(&windows[0]);
    open_grid(&mut f);

    SetUp {
        f,
        id,
        pointer,
        windows,
    }
}

fn open_grid(f: &mut Fixture) {
    let niri = f.niri();
    assert!(niri.layout.open_grid_overview());
    niri.clock.set_complete_instantly(true);
    niri.advance_animations();
    niri.clock.set_complete_instantly(false);
}

fn cell_center(f: &mut Fixture, window: &Window) -> Point<f64, Logical> {
    let ws = f.niri().layout.active_workspace().unwrap();
    let info = *ws.grid_overview().unwrap().find_grid_info(window).unwrap();
    info.target_pos + Point::from((info.target_size.w / 2., info.target_size.h / 2.))
}

fn move_pointer(
    f: &mut Fixture,
    id: ClientId,
    pointer: &ZwlrVirtualPointerV1,
    delta: Point<f64, Logical>,
) {
    pointer.motion(0, delta.x, delta.y);
    pointer.frame();
    f.roundtrip(id);
}

fn grid_focus(f: &mut Fixture) -> Option<Window> {
    f.niri().layout.grid_focused_window_id()
}

const FFM: &str = "grid-overview { focus-follows-mouse; }";

#[test]
fn focus_follows_pointer_into_another_cell() {
    let SetUp {
        mut f,
        id,
        pointer,
        windows,
    } = set_up(FFM);
    let [first, second, _] = [0, 1, 2].map(|i| windows[i].clone());

    let start = cell_center(&mut f, &first);
    f.niri_state().move_cursor(start);

    let target = cell_center(&mut f, &second);
    move_pointer(&mut f, id, &pointer, target - start);
    assert_eq!(grid_focus(&mut f), Some(second));

    // The window under the pointer didn't change: nothing to do.
    move_pointer(&mut f, id, &pointer, Point::from((1., 0.)));
    assert!(f.niri().layout.is_grid_overview_open());
}

#[test]
fn small_motion_does_not_take_focus() {
    let SetUp {
        mut f,
        id,
        pointer,
        windows,
    } = set_up(FFM);
    let [first, second, _] = [0, 1, 2].map(|i| windows[i].clone());

    // Start at the edge of the second cell, so a few pixels already cross into it.
    let ws = f.niri().layout.active_workspace().unwrap();
    let info = *ws.grid_overview().unwrap().find_grid_info(&second).unwrap();
    let start = info.target_pos + Point::from((-3., info.target_size.h / 2.));
    f.niri_state().move_cursor(start);
    assert_eq!(grid_focus(&mut f), Some(first.clone()));

    // 6 px is under the default 8 px threshold.
    move_pointer(&mut f, id, &pointer, Point::from((6., 0.)));
    assert!(f
        .niri()
        .window_under(start + Point::from((6., 0.)))
        .is_some());
    assert_eq!(grid_focus(&mut f), Some(first));

    // Past the threshold, the focus follows.
    move_pointer(&mut f, id, &pointer, Point::from((6., 0.)));
    assert_eq!(grid_focus(&mut f), Some(second));
}

#[test]
fn keyboard_navigation_wins_until_pointer_moves_far_enough() {
    let SetUp {
        mut f,
        id,
        pointer,
        windows,
    } = set_up(FFM);
    let [first, second, third] = [0, 1, 2].map(|i| windows[i].clone());

    let start = cell_center(&mut f, &first);
    f.niri_state().move_cursor(start);
    let target = cell_center(&mut f, &second);
    move_pointer(&mut f, id, &pointer, target - start);
    assert_eq!(grid_focus(&mut f), Some(second.clone()));

    // Keyboard navigation moves the focus away from the cell under the pointer.
    f.niri().layout.focus_right();
    assert_eq!(grid_focus(&mut f), Some(third.clone()));

    // A twitch of the pointer doesn't take it back.
    move_pointer(&mut f, id, &pointer, Point::from((3., 3.)));
    assert_eq!(grid_focus(&mut f), Some(third));

    // A real motion does.
    move_pointer(&mut f, id, &pointer, Point::from((10., 0.)));
    assert_eq!(grid_focus(&mut f), Some(second));
}

#[test]
fn reopened_grid_needs_pointer_motion_again() {
    let SetUp {
        mut f,
        id,
        pointer,
        windows,
    } = set_up(FFM);
    let [first, second, _] = [0, 1, 2].map(|i| windows[i].clone());

    let start = cell_center(&mut f, &first);
    f.niri_state().move_cursor(start);
    let target = cell_center(&mut f, &second);
    move_pointer(&mut f, id, &pointer, target - start);
    assert_eq!(grid_focus(&mut f), Some(second.clone()));

    // Close the grid without confirming the hovered cell, then reopen it.
    let niri = f.niri();
    niri.layout.activate_window(&first);
    assert!(niri.layout.dismiss_grid_overview());
    niri.clock.set_complete_instantly(true);
    niri.advance_animations();
    niri.clock.set_complete_instantly(false);
    open_grid(&mut f);
    assert_eq!(grid_focus(&mut f), Some(first.clone()));

    // The pointer now sits over some cell; a twitch there doesn't take the focus.
    move_pointer(&mut f, id, &pointer, Point::from((2., 0.)));
    assert_eq!(grid_focus(&mut f), Some(first));
}

#[test]
fn pointer_does_not_move_focus_by_default() {
    let SetUp {
        mut f,
        id,
        pointer,
        windows,
    } = set_up("");
    let [first, second, _] = [0, 1, 2].map(|i| windows[i].clone());

    let start = cell_center(&mut f, &first);
    f.niri_state().move_cursor(start);
    let target = cell_center(&mut f, &second);
    move_pointer(&mut f, id, &pointer, target - start);
    assert_eq!(grid_focus(&mut f), Some(first));
}
