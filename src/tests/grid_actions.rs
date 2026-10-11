use client::ClientId;
use niri_config::Action;
use niri_ipc::SizeChange;
use smithay::desktop::Window;

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

// Two quarter-width columns, the first one active, and the grid overview open.
fn set_up() -> (Fixture, Window, Window) {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    add_window(&mut f, id);
    add_window(&mut f, id);

    let state = f.niri_state();
    let windows: Vec<_> = state
        .niri
        .layout
        .windows()
        .map(|(_, m)| m.window.clone())
        .collect();
    for window in &windows {
        state.niri.layout.activate_window(window);
        state
            .niri
            .layout
            .set_column_width(SizeChange::SetProportion(25.));
    }
    state.niri.layout.activate_window(&windows[0]);
    assert!(state.niri.layout.open_grid_overview());

    (f, windows[0].clone(), windows[1].clone())
}

#[test]
fn expand_column_in_grid_expands_the_selected_column() {
    let (mut f, first, second) = set_up();
    f.niri().layout.focus_right();
    assert_eq!(
        f.niri().layout.grid_focused_window_id(),
        Some(second.clone())
    );
    let first_width = *f
        .niri()
        .layout
        .active_workspace()
        .unwrap()
        .scrolling()
        .columns()
        .next()
        .unwrap()
        .column_width();

    f.niri_state()
        .do_action(Action::ExpandColumnToAvailableWidth, false);

    // It used to expand the column of the window active before the grid opened, then activate
    // the selected one.
    let layout = &f.niri().layout;
    assert!(!layout.is_grid_overview_open());
    assert_eq!(
        layout.focus().map(|m| m.window.clone()),
        Some(second.clone())
    );
    let ws = layout.active_workspace().unwrap();
    let mut columns = ws.scrolling().columns();
    let first_col = columns.next().unwrap();
    let second_col = columns.next().unwrap();
    assert!(first_col.contains(&first));
    assert_eq!(first_col.column_width(), &first_width);
    assert_ne!(second_col.column_width(), &first_width);
}

#[test]
fn by_id_actions_in_grid_leave_the_grid_alone() {
    let (mut f, first, second) = set_up();
    assert_eq!(
        f.niri().layout.grid_focused_window_id(),
        Some(first.clone())
    );
    let id = f
        .niri()
        .layout
        .windows()
        .find(|(_, m)| m.window == second)
        .map(|(_, m)| m.id().get())
        .unwrap();

    f.niri_state()
        .do_action(Action::FullscreenWindowById(id), false);
    f.niri_state().do_action(
        Action::SetWindowWidthById {
            id,
            change: SizeChange::SetProportion(30.),
        },
        false,
    );

    // These used to close the grid and activate its selection, which wasn't the window named
    // by the id.
    let layout = &f.niri().layout;
    assert!(layout.is_grid_overview_open());
    assert_eq!(layout.grid_focused_window_id(), Some(first));
}
