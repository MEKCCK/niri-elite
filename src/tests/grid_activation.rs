use client::ClientId;
use wayland_client::protocol::wl_surface::WlSurface;

use super::*;

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

// Sets up three windows on one output with the grid overview open on the last one, and returns
// the niri window of the first one.
fn set_up() -> (Fixture, smithay::desktop::Window) {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    for _ in 0..3 {
        add_window(&mut f, id);
    }

    let niri = f.niri();
    let first = niri.layout.windows().next().unwrap().1.window.clone();
    assert!(niri.layout.open_grid_overview());
    (f, first)
}

#[test]
fn focusing_a_window_in_the_grid_closes_it() {
    let (mut f, first) = set_up();
    assert_ne!(
        f.niri().layout.focus().map(|m| m.window.clone()),
        Some(first.clone())
    );

    // What focus-window and focus-window-previous do.
    f.niri_state().focus_window(&first);

    let niri = f.niri();
    assert!(!niri.layout.is_grid_overview_open());
    assert_eq!(
        niri.layout.focus().map(|m| m.window.clone()),
        Some(first.clone())
    );
}

#[test]
fn focusing_a_minimized_window_in_the_grid_restores_it() {
    let (mut f, first) = set_up();
    let niri = f.niri();
    niri.layout.close_grid_overview();
    niri.layout.set_window_minimized(&first, true);
    assert!(niri.layout.open_grid_overview());

    f.niri_state().focus_window(&first);

    let niri = f.niri();
    assert!(!niri.layout.is_grid_overview_open());
    assert!(!niri.layout.is_window_minimized(&first));
    assert_eq!(niri.layout.focus().map(|m| m.window.clone()), Some(first));
}
