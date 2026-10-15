use std::fs::File;
use std::io::BufWriter;
use std::time::Duration;

use anyhow::Context;
use niri::animation::Clock;
use niri::layout::grid_overview::GridItem;
use niri::layout::{ActivateWindow, AddWindowTarget, LayoutElement as _, Options, SizingMode};
use niri::render_helpers::{
    copy_framebuffer, create_texture, resources, shaders, RenderCtx, RenderIntent, RenderTarget,
};
use niri::utils::transaction::Transaction;
use niri_config::{Color, OutputName, PresetSize};
use niri_ipc::PositionChange;
use niri_visual_tests::test_window::TestWindow;
use smithay::backend::egl::ffi::make_sure_egl_is_loaded;
use smithay::backend::egl::native::EGLSurfacelessDisplay;
use smithay::backend::egl::{EGLContext, EGLDisplay};
use smithay::backend::renderer::element::RenderElement;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::{Bind, Color32F, ExportMem, Frame, Renderer};
use smithay::output::{Mode, Output, PhysicalProperties, Subpixel};
use smithay::reexports::gbm::Format as Fourcc;
use smithay::utils::user_data::UserDataMap;
use smithay::utils::{Logical, Physical, Point, Rectangle, Scale, Size, Transform};

fn save_png(path: &str, width: u32, height: u32, xrgb: &[u8]) -> anyhow::Result<()> {
    let mut rgba = Vec::with_capacity(xrgb.len());
    for chunk in xrgb.chunks_exact(4) {
        rgba.push(chunk[2]);
        rgba.push(chunk[1]);
        rgba.push(chunk[0]);
        rgba.push(255);
    }

    let file = File::create(path)?;
    let w = BufWriter::new(file);
    let mut encoder = png::Encoder::new(w, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(&rgba)?;
    Ok(())
}

fn render_frame(
    renderer: &mut GlesRenderer,
    size: Size<i32, Physical>,
    elements: Vec<Box<dyn RenderElement<GlesRenderer>>>,
) -> anyhow::Result<Vec<u8>> {
    let mut texture =
        create_texture(renderer, size, Fourcc::Xrgb8888).context("creating texture")?;
    let mut target = renderer.bind(&mut texture).context("binding texture")?;

    let mut frame = renderer
        .render(&mut target, size, Transform::Normal)
        .context("starting frame")?;

    let rect: Rectangle<i32, Physical> = Rectangle::from_size(size);
    frame
        .clear(Color32F::from([0.15, 0.15, 0.15, 1.]), &[rect])
        .context("clearing frame")?;

    for element in elements.iter().rev() {
        let src = element.src();
        let dst = element.geometry(Scale::from(1.));

        if let Some(mut damage) = rect.intersection(dst) {
            damage.loc -= dst.loc;

            let cache = UserDataMap::new();
            if element.is_framebuffer_effect() {
                element
                    .capture_framebuffer(&mut frame, src, dst, &cache)
                    .context("capture_framebuffer")?;
            }
            element
                .draw(&mut frame, src, dst, &[damage], &[], Some(&cache))
                .context("drawing element")?;
        }
    }

    frame.finish().context("finishing frame")?;

    let mapping =
        copy_framebuffer(renderer, &target, Fourcc::Xrgb8888).context("copying framebuffer")?;
    let bytes = renderer.map_texture(&mapping).context("mapping texture")?;
    Ok(bytes.to_vec())
}

/// A window to add: tiled with a column width, or floating at a position with a size.
enum Spec {
    Tiled(f64),
    Floating((i32, i32), (i32, i32)),
}

/// Grid overview layout scenarios, one PNG per step plus the cell geometry on stderr.
///
/// Usage: grid-layout-headless OUT_DIR [mixed|five|allfloat|wechat|tiled] [drag|close|add]
///
/// `drag` (the default) drags windows into and out of tiling; `close` closes the tiled windows
/// one after another, and `add` opens floating windows one after another, printing how far every
/// floating window moves.
fn main() -> anyhow::Result<()> {
    make_sure_egl_is_loaded().context("loading EGL")?;
    let display = unsafe { EGLDisplay::new(EGLSurfacelessDisplay) }
        .context("creating surfaceless EGL display")?;
    let context = EGLContext::new(&display).context("creating EGL context")?;
    unsafe { context.make_current() }.context("making EGL context current")?;

    let mut renderer = unsafe { GlesRenderer::new(context) }.context("creating GlesRenderer")?;
    resources::init(&mut renderer);
    shaders::init(&mut renderer);

    let out_dir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/grid-layout-vis".to_owned());
    let scenario = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "mixed".to_owned());
    let mode = std::env::args().nth(3).unwrap_or_else(|| "drag".to_owned());
    std::fs::create_dir_all(&out_dir)?;

    let mut clock = Clock::with_time(Duration::ZERO);
    let size: Size<i32, Logical> = Size::from((1280, 720));

    let output = Output::new(
        String::new(),
        PhysicalProperties {
            size: Size::from((size.w, size.h)),
            subpixel: Subpixel::Unknown,
            make: String::new(),
            model: String::new(),
            serial_number: String::new(),
        },
    );
    output.change_current_state(
        Some(Mode {
            size: size.to_physical(1),
            refresh: 60000,
        }),
        None,
        None,
        None,
    );
    output.user_data().insert_if_missing(|| OutputName {
        connector: String::new(),
        make: None,
        model: None,
        serial: None,
    });

    let options = Options {
        layout: niri_config::Layout {
            focus_ring: niri_config::FocusRing {
                off: false,
                width: 4.,
                active_color: Color::from_rgba8_unpremul(255, 163, 72, 255),
                inactive_color: Color::from_rgba8_unpremul(50, 50, 50, 255),
                ..Default::default()
            },
            gaps: 8.,
            ..Default::default()
        },
        grid_overview: niri_config::GridOverview {
            gap: 12.,
            padding: niri_config::GridOverviewPadding::uniform(40.),
            ..Default::default()
        },
        ..Default::default()
    };

    let mut layout = niri::layout::Layout::<TestWindow>::with_options(clock.clone(), options);
    layout.add_output(output.clone(), None);

    let specs: Vec<Spec> = match scenario.as_str() {
        "five" => vec![
            Spec::Tiled(0.5),
            Spec::Tiled(0.5),
            Spec::Tiled(1. / 3.),
            Spec::Floating((60, 60), (300, 220)),
            Spec::Floating((900, 50), (330, 260)),
            Spec::Floating((480, 250), (340, 280)),
            Spec::Floating((80, 470), (300, 200)),
            Spec::Floating((880, 440), (340, 240)),
        ],
        "allfloat" => vec![
            Spec::Floating((60, 80), (520, 400)),
            Spec::Floating((640, 60), (460, 380)),
            Spec::Floating((380, 260), (620, 420)),
            Spec::Floating((60, 420), (340, 260)),
            Spec::Floating((880, 380), (380, 300)),
            Spec::Floating((560, 40), (260, 190)),
            Spec::Floating((420, 520), (380, 180)),
        ],
        // Five tiled windows and a small floating one in the middle of the screen.
        "wechat" => vec![
            Spec::Tiled(0.5),
            Spec::Tiled(0.5),
            Spec::Tiled(0.5),
            Spec::Tiled(0.5),
            Spec::Tiled(0.5),
            Spec::Floating((569, 264), (142, 192)),
        ],
        "tiled" => vec![
            Spec::Tiled(0.5),
            Spec::Tiled(1. / 3.),
            Spec::Tiled(2. / 3.),
            Spec::Tiled(0.5),
            Spec::Tiled(0.5),
            Spec::Tiled(1. / 3.),
        ],
        // The scene of the screenshot: four columns, one of them with three tabs, and three
        // floating windows around the screen.
        _ => vec![
            Spec::Tiled(0.5),
            Spec::Tiled(1. / 3.),
            Spec::Tiled(0.5),
            Spec::Tiled(0.5),
            Spec::Tiled(0.5),
            Spec::Tiled(2. / 3.),
            Spec::Floating((880, 60), (360, 260)),
            Spec::Floating((420, 300), (440, 330)),
            Spec::Floating((80, 470), (300, 200)),
        ],
    };

    let colors = [
        [0.15, 0.64, 0.41, 1.],
        [0.88, 0.11, 0.14, 1.],
        [0.10, 0.40, 0.90, 1.],
        [0.55, 0.20, 0.70, 1.],
        [0.20, 0.55, 0.60, 1.],
        [0.60, 0.60, 0.20, 1.],
        [0.90, 0.50, 0.10, 1.],
        [0.85, 0.35, 0.60, 1.],
        [0.35, 0.75, 0.85, 1.],
        [0.75, 0.75, 0.75, 1.],
    ];

    let mut windows = Vec::new();
    for (id, spec) in specs.iter().enumerate() {
        let mut window = TestWindow::freeform(id);
        window.set_color(colors[id % colors.len()]);
        let ws = layout.active_workspace().unwrap();
        let (min_size, max_size) = (window.min_size(), window.max_size());
        let (is_floating, width, new_size) = match spec {
            Spec::Tiled(frac) => {
                let width = PresetSize::Proportion(*frac);
                let size = ws.new_window_size(
                    Some(width),
                    None,
                    false,
                    window.rules(),
                    (min_size, max_size),
                );
                (false, Some(width), size)
            }
            Spec::Floating(_, (w, h)) => (true, None, Size::from((*w, *h))),
        };
        window.request_size(new_size, SizingMode::Normal, false, None);
        window.communicate();
        layout.add_window(
            window.clone(),
            AddWindowTarget::Auto,
            width,
            None,
            false,
            is_floating,
            ActivateWindow::Yes,
        );
        if let Spec::Floating((x, y), _) = spec {
            layout.move_floating_window(
                Some(&id),
                PositionChange::SetFixed(*x as f64),
                PositionChange::SetFixed(*y as f64),
                false,
            );
        }
        windows.push(window);
    }

    // In the mixed scene, windows 2, 3 and 4 become the tabs of one column.
    if scenario == "mixed" {
        layout.activate_window(&3);
        layout.consume_or_expel_window_left(None);
        layout.activate_window(&4);
        layout.consume_or_expel_window_left(None);
        layout.toggle_column_tabbed_display();
    }
    layout.activate_window(&0);

    let commit_all = |layout: &mut niri::layout::Layout<TestWindow>| {
        for window in &windows {
            if window.communicate() {
                layout.update_window(window.id(), None);
            }
        }
    };
    commit_all(&mut layout);

    let first_floating = specs
        .iter()
        .position(|spec| matches!(spec, Spec::Floating(..)));
    let last_tiled = specs
        .iter()
        .rposition(|spec| matches!(spec, Spec::Tiled(..)));

    let mut t = 100;
    let mut last_rects: Vec<(usize, Rectangle<f64, Logical>)> = Vec::new();
    let mut step = |layout: &mut niri::layout::Layout<TestWindow>,
                    renderer: &mut GlesRenderer,
                    name: &str,
                    action: &mut dyn FnMut(&mut niri::layout::Layout<TestWindow>)|
     -> anyhow::Result<()> {
        t += 100;
        clock.set_unadjusted(Duration::from_millis(t));
        action(layout);
        commit_all(layout);
        // Let everything settle, updating render elements on the way like every frame does, so
        // that transitions that start there (the focus ring) run to their end too.
        for _ in 0..40 {
            t += 100;
            clock.set_unadjusted(Duration::from_millis(t));
            layout.advance_animations();
            commit_all(layout);
            layout.update_render_elements(Some(&output));
        }

        let ws = layout.active_workspace().unwrap();
        if let Some(go) = ws.grid_overview() {
            eprintln!(
                "== {name}: scale {:.3}, wrap {}, {:?}, grid focus {:?}, active {:?}",
                go.layout.scale,
                go.layout.wrap,
                (go.layout.floating_order, go.layout.block_place),
                layout.grid_focused_window_id(),
                ws.active_window().map(|win| win.id()),
            );
            let mut covered = 0.;
            for (item, info) in &go.layout.entries {
                let r = info.rect();
                covered += r.size.w * r.size.h;
                let kind = match item {
                    GridItem::Floating { .. } => "floating",
                    _ => "tiled",
                };
                eprintln!(
                    "   window {} {kind:8} row {:?} at ({:.0}, {:.0}) size {:.0}x{:.0}",
                    item.window_id(),
                    info.row,
                    r.loc.x,
                    r.loc.y,
                    r.size.w,
                    r.size.h
                );
            }
            let area = ws.working_area();
            eprintln!(
                "   covered {:.0}% of the working area",
                100. * covered / (area.size.w * area.size.h)
            );
            let mut moves = Vec::new();
            for (item, info) in &go.layout.entries {
                if !matches!(item, GridItem::Floating { .. }) {
                    continue;
                }
                let id = *item.window_id();
                let r = info.rect();
                if let Some((_, old)) = last_rects.iter().find(|(old_id, _)| *old_id == id) {
                    let dx = (r.loc.x + r.size.w / 2.) - (old.loc.x + old.size.w / 2.);
                    let dy = (r.loc.y + r.size.h / 2.) - (old.loc.y + old.size.h / 2.);
                    moves.push(format!("{id}: {:.0}", dx.hypot(dy)));
                }
            }
            if !moves.is_empty() {
                eprintln!("   floating windows moved by {}", moves.join(", "));
            }
            last_rects = go
                .layout
                .entries
                .iter()
                .map(|(item, info)| (*item.window_id(), info.rect()))
                .collect();
        }

        let mut elements = Vec::new();
        let ctx = RenderCtx {
            renderer,
            target: RenderTarget::Output,
            intent: RenderIntent::Normal,
            xray: None,
        };
        layout
            .monitor_for_output(&output)
            .unwrap()
            .render_workspaces(ctx, true, &mut |elem| {
                elements.push(Box::new(elem) as Box<dyn RenderElement<GlesRenderer>>);
            });
        let bytes = render_frame(renderer, size.to_physical(1), elements)?;
        let path = format!("{out_dir}/{scenario}_{name}.png");
        save_png(&path, size.w as u32, size.h as u32, &bytes)?;
        eprintln!("wrote {path}");
        Ok(())
    };

    // Where the grid shows a window, in output coordinates.
    let cell = |layout: &niri::layout::Layout<TestWindow>, id: usize| {
        let go = layout.active_workspace().unwrap().grid_overview().unwrap();
        let (_, info) = go
            .layout
            .entries
            .iter()
            .find(|(item, _)| *item.window_id() == id)
            .unwrap();
        info.rect()
    };
    // Drags window `id` from the middle of its cell to `to`, switching it between floating and
    // tiling on the way like a right click would.
    let drag = |layout: &mut niri::layout::Layout<TestWindow>,
                id: usize,
                to: Point<f64, Logical>,
                switch: bool| {
        let from = cell(layout, id);
        let from = from.loc + Point::from((from.size.w / 2., from.size.h / 2.));
        assert!(layout.interactive_move_begin(id, &output, from));
        let halfway = Point::from(((from.x + to.x) / 2., (from.y + to.y) / 2.));
        layout.interactive_move_update(&id, halfway - from, output.clone(), halfway);
        if switch {
            layout.toggle_window_floating(Some(&id));
        }
        layout.interactive_move_update(&id, to - halfway, output.clone(), to);
        layout.interactive_move_end(&id);
    };

    step(&mut layout, &mut renderer, "0_desktop", &mut |_| ())?;
    step(&mut layout, &mut renderer, "1_grid", &mut |layout| {
        layout.toggle_grid_overview()
    })?;
    if mode == "add" {
        let spots = [
            ((700, 120), (420, 300)),
            ((150, 380), (360, 260)),
            ((900, 420), (300, 240)),
            ((450, 200), (500, 360)),
            ((80, 60), (280, 200)),
            ((1000, 80), (240, 180)),
        ];
        for (n, &((x, y), (w, h))) in spots.iter().enumerate() {
            let id = specs.len() + n;
            let mut window = TestWindow::freeform(id);
            window.set_color(colors[id % colors.len()]);
            window.request_size(Size::from((w, h)), SizingMode::Normal, false, None);
            window.communicate();
            step(
                &mut layout,
                &mut renderer,
                &format!("add_{id}"),
                &mut |layout| {
                    layout.add_window(
                        window.clone(),
                        AddWindowTarget::Auto,
                        None,
                        None,
                        false,
                        true,
                        ActivateWindow::Yes,
                    );
                    layout.move_floating_window(
                        Some(&id),
                        PositionChange::SetFixed(x as f64),
                        PositionChange::SetFixed(y as f64),
                        false,
                    );
                },
            )?;
        }
        return Ok(());
    }

    if mode == "close" {
        // The first tiled window, then the last, then whatever is left, one after another.
        let mut tiled: Vec<usize> = specs
            .iter()
            .enumerate()
            .filter(|(_, spec)| matches!(spec, Spec::Tiled(..)))
            .map(|(id, _)| id)
            .collect();
        let mut order = Vec::new();
        while !tiled.is_empty() {
            order.push(tiled.remove(0));
            if let Some(last) = tiled.pop() {
                order.push(last);
            }
        }
        for id in order {
            step(
                &mut layout,
                &mut renderer,
                &format!("close_{id}"),
                &mut |layout| {
                    layout.remove_window(&id, Transaction::new());
                },
            )?;
        }
        return Ok(());
    }

    if let (Some(floating), Some(tiled)) = (first_floating, last_tiled) {
        step(
            &mut layout,
            &mut renderer,
            "2_floating_dragged_into_tiling",
            &mut |layout| {
                // Onto the left edge of the last tiled window's cell.
                let target = cell(layout, tiled);
                let to = target.loc + Point::from((target.size.w * 0.1, target.size.h / 2.));
                drag(layout, floating, to, true);
            },
        )?;
        step(
            &mut layout,
            &mut renderer,
            "3_tiled_dragged_out_to_floating",
            &mut |layout| {
                // Down into the bottom right corner.
                drag(layout, tiled, Point::from((1150., 600.)), true);
            },
        )?;
    }
    if let Some(floating) = first_floating
        .map(|id| id + 1)
        .filter(|id| *id < specs.len())
    {
        step(
            &mut layout,
            &mut renderer,
            "4_floating_dragged_elsewhere",
            &mut |layout| {
                drag(layout, floating, Point::from((200., 150.)), false);
            },
        )?;
    }

    Ok(())
}
