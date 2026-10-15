//! Using the grid overview the way people do, through the whole compositor: real clients that
//! resize to what they are configured to, floating windows from window rules, windows opening and
//! closing while the grid shows and while it's closed.
//!
//! Set `NIRI_GRID_FLOW_DUMP` to a directory to get the grid's cells at every step as text, one file
//! per step, for looking at them.

use client::ClientId;
use niri_config::{Action, Config};
use niri_ipc::PositionChange;
use smithay::desktop::Window;
use smithay::utils::{Logical, Rectangle};
use wayland_client::protocol::wl_surface::WlSurface;

use super::*;
use crate::utils::with_toplevel_role;

fn title(window: &Window) -> Option<String> {
    with_toplevel_role(window.toplevel()?, |role| role.title.clone())
}

/// A cell of the grid: the label of its window, where it is, and whether it is floating.
type Cell = (String, Rectangle<f64, Logical>, bool);

struct Flow {
    f: Fixture,
    id: ClientId,
    /// Every window opened, with its label and the size it takes when it can pick one.
    windows: Vec<(WlSurface, String, (u16, u16))>,
    step: usize,
}

impl Flow {
    fn new() -> Self {
        let config = Config::parse_mem(
            r##"
            window-rule {
                match title="^float"
                open-floating true
            }
            grid-overview {
                gap 20
                padding 100
            }
            "##,
        )
        .unwrap();
        let mut f = Fixture::with_config(config);
        f.add_output(1, (2560, 1440));
        let id = f.add_client();
        Self {
            f,
            id,
            windows: Vec::new(),
            step: 0,
        }
    }

    /// Opens a window, which takes the size it's configured to, or `size` if it may pick one.
    fn open(&mut self, label: &str, size: (u16, u16)) -> WlSurface {
        let window = self.f.client(self.id).create_window();
        window.set_title(label);
        let surface = window.surface.clone();
        window.commit();
        self.f.roundtrip(self.id);
        self.f.client(self.id).window(&surface).attach_new_buffer();
        self.windows.push((surface.clone(), label.to_owned(), size));
        self.settle();
        surface
    }

    /// Lets every window answer the configures it got, like clients do, then lets every animation
    /// finish.
    fn settle(&mut self) {
        for _ in 0..4 {
            let mut answered = false;
            for (surface, _, size) in &self.windows {
                let window = self.f.client(self.id).window(surface);
                let Some(configure) = window.recent_configures().last().cloned() else {
                    continue;
                };
                let (w, h) = match configure.size {
                    (0, _) | (_, 0) => *size,
                    (w, h) => (w as u16, h as u16),
                };
                window.set_size(w, h);
                window.ack_last_and_commit();
                answered = true;
            }
            self.f.double_roundtrip(self.id);
            if !answered {
                break;
            }
        }
        self.f.niri_complete_animations();
        self.f.double_roundtrip(self.id);
    }

    fn mapped(&mut self, label: &str) -> Window {
        self.f
            .niri()
            .layout
            .windows()
            .find(|(_, mapped)| title(&mapped.window).as_deref() == Some(label))
            .map(|(_, mapped)| mapped.window.clone())
            .unwrap()
    }

    fn place_floating(&mut self, label: &str, x: f64, y: f64) {
        let window = self.mapped(label);
        self.f.niri().layout.move_floating_window(
            Some(&window),
            PositionChange::SetFixed(x),
            PositionChange::SetFixed(y),
            false,
        );
        self.settle();
    }

    fn close(&mut self, label: &str) {
        let idx = self
            .windows
            .iter()
            .position(|(_, l, _)| l == label)
            .unwrap();
        let (surface, _, _) = self.windows.remove(idx);
        let window = self.f.client(self.id).window(&surface);
        window.attach_null();
        window.commit();
        self.settle();
    }

    fn toggle_grid(&mut self) {
        self.f
            .niri_state()
            .do_action(Action::ToggleGridOverview, false);
        self.settle();
    }

    /// The grid's cells by window label, with the scale, or `None` with the grid closed.
    fn cells(&mut self) -> Option<(f64, Vec<Cell>)> {
        let niri = self.f.niri();
        let ws = niri.layout.active_workspace()?;
        let go = ws.grid_overview().filter(|go| go.open)?;
        let cells = go
            .layout
            .entries
            .iter()
            .map(|(item, info)| {
                let label = title(item.window_id()).unwrap_or_else(|| "?".to_owned());
                (label, info.rect(), info.row.is_none())
            })
            .collect();
        Some((go.layout.scale, cells))
    }

    /// Writes the grid's cells for looking at them, if asked to.
    fn dump(&mut self, name: &str) {
        self.step += 1;
        let Some(dir) = std::env::var_os("NIRI_GRID_FLOW_DUMP") else {
            return;
        };
        let mut text = String::new();
        if let Some((scale, cells)) = self.cells() {
            text += &format!("scale {scale}\n");
            for (label, r, floating) in cells {
                text += &format!(
                    "{label} {} {} {} {} {}\n",
                    r.loc.x,
                    r.loc.y,
                    r.size.w,
                    r.size.h,
                    if floating { "floating" } else { "tiled" }
                );
            }
        }
        let path = std::path::Path::new(&dir).join(format!("{:02}_{name}.txt", self.step));
        std::fs::write(path, text).unwrap();
    }

    fn rows(&mut self) -> usize {
        let niri = self.f.niri();
        let ws = niri.layout.active_workspace().unwrap();
        let go = ws.grid_overview().unwrap();
        go.layout
            .entries
            .iter()
            .filter_map(|(_, info)| info.row)
            .max()
            .map_or(0, |row| row + 1)
    }
}

#[test]
fn grid_flow() {
    let mut flow = Flow::new();
    flow.open("t1", (800, 600));
    flow.open("t2", (800, 600));
    flow.open("float-files", (900, 640));
    flow.place_floating("float-files", 80., 80.);
    flow.open("float-term", (900, 600));
    flow.place_floating("float-term", 1580., 60.);
    flow.dump("desktop");

    flow.toggle_grid();
    flow.dump("grid");
    let first = flow.cells().unwrap();
    flow.toggle_grid();

    // Many windows open while the grid is closed.
    for n in 3..=14 {
        flow.open(&format!("t{n}"), (800, 600));
    }
    flow.toggle_grid();
    flow.dump("grid_after_opening_12");
    assert!(flow.rows() > 1, "all windows in one row");
    let again = flow.cells().unwrap();
    assert!(again.0 > first.0 * 0.3);

    // Closed and opened again with nothing changed, it shows the same.
    flow.toggle_grid();
    flow.toggle_grid();
    flow.dump("grid_unchanged");
    assert_eq!(flow.cells().unwrap().1, again.1);

    // Windows opening while it shows.
    for n in 15..=17 {
        flow.open(&format!("t{n}"), (800, 600));
        flow.dump(&format!("opened_t{n}"));
    }
    assert!(flow.rows() > 1);

    // Windows closing while it shows: once there are few left, they show larger.
    let before = flow.cells().unwrap().0;
    for n in [3, 5, 7, 9, 11, 13, 15, 17, 4, 6] {
        flow.close(&format!("t{n}"));
        flow.dump(&format!("closed_t{n}"));
    }
    let after = flow.cells().unwrap().0;
    assert!(after > before * 1.15, "{before} -> {after}");

    // A floating window opening while it shows, then the grid closed and opened again.
    flow.open("float-new", (1000, 700));
    flow.place_floating("float-new", 1400., 700.);
    flow.dump("opened_float");
    let shown = flow.cells().unwrap();
    flow.toggle_grid();
    flow.toggle_grid();
    flow.dump("grid_unchanged_again");
    assert_eq!(flow.cells().unwrap().1, shown.1);
}
