use std::fs::File;
use std::io::BufWriter;
use std::time::Duration;

use anyhow::Context;
use niri::animation::Clock;
use niri::layout::{ActivateWindow, AddWindowTarget, LayoutElement as _, Options, SizingMode};
use niri::render_helpers::{
    copy_framebuffer, create_texture, resources, shaders, RenderCtx, RenderIntent, RenderTarget,
};
use niri_config::{Color, OutputName, PresetSize};
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
use smithay::utils::{Logical, Physical, Rectangle, Scale, Size, Transform};

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

struct Env {
    renderer: GlesRenderer,
    clock: Clock,
    size: Size<i32, Logical>,
    output: Output,
}

fn make_env() -> anyhow::Result<Env> {
    make_sure_egl_is_loaded().context("loading EGL")?;
    let display = unsafe { EGLDisplay::new(EGLSurfacelessDisplay) }
        .context("creating surfaceless EGL display")?;
    let context = EGLContext::new(&display).context("creating EGL context")?;
    unsafe { context.make_current() }.context("making EGL context current")?;
    let mut renderer = unsafe { GlesRenderer::new(context) }.context("creating GlesRenderer")?;
    resources::init(&mut renderer);
    shaders::init(&mut renderer);

    let clock = Clock::with_time(Duration::ZERO);
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
    Ok(Env {
        renderer,
        clock,
        size,
        output,
    })
}

fn make_layout(
    clock: &Clock,
    size: Size<i32, Logical>,
    output: &Output,
) -> niri::layout::Layout<TestWindow> {
    let options = Options {
        layout: niri_config::Layout {
            focus_ring: niri_config::FocusRing {
                off: true,
                ..Default::default()
            },
            border: niri_config::Border {
                off: false,
                width: 4.,
                active_color: Color::from_rgba8_unpremul(255, 163, 72, 255),
                inactive_color: Color::from_rgba8_unpremul(50, 50, 50, 255),
                urgent_color: Color::from_rgba8_unpremul(155, 0, 0, 255),
                active_gradient: None,
                inactive_gradient: None,
                urgent_gradient: None,
            },
            ..Default::default()
        },
        ..Default::default()
    };
    let mut layout = niri::layout::Layout::<TestWindow>::with_options(clock.clone(), options);
    layout.add_output(output.clone(), None);
    layout
}

fn add_window(
    layout: &mut niri::layout::Layout<TestWindow>,
    id: usize,
    color: [f32; 4],
) -> TestWindow {
    let mut window = TestWindow::freeform(id);
    window.set_color(color);
    let ws = layout.active_workspace().unwrap();
    let min_size = window.min_size();
    let max_size = window.max_size();
    window.request_size(
        ws.new_window_size(
            Some(PresetSize::Proportion(0.3)),
            None,
            false,
            window.rules(),
            (min_size, max_size),
        ),
        SizingMode::Normal,
        false,
        None,
    );
    window.communicate();
    layout.add_window(
        window.clone(),
        AddWindowTarget::Auto,
        Some(PresetSize::Proportion(0.3)),
        None,
        false,
        false,
        ActivateWindow::default(),
    );
    window
}

fn capture(
    env: &mut Env,
    layout: &mut niri::layout::Layout<TestWindow>,
    t_ms: u64,
    path: &str,
) -> anyhow::Result<()> {
    let now = Duration::from_millis(t_ms);
    env.clock.set_unadjusted(now);
    layout.advance_animations();
    layout.update_render_elements(Some(&env.output));

    let mut elements = Vec::new();
    let ctx = RenderCtx {
        renderer: &mut env.renderer,
        target: RenderTarget::Output,
        intent: RenderIntent::Normal,
        xray: None,
    };
    layout
        .monitor_for_output(&env.output)
        .unwrap()
        .render_workspaces(ctx, true, &mut |elem| {
            elements.push(Box::new(elem) as Box<dyn RenderElement<GlesRenderer>>);
        });

    let bytes = render_frame(&mut env.renderer, env.size.to_physical(1), elements)?;
    save_png(path, env.size.w as u32, env.size.h as u32, &bytes)?;
    eprintln!("wrote {path}");
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let env_filter = tracing_subscriber::EnvFilter::builder()
        .parse_lossy(std::env::var("RUST_LOG").unwrap_or_default());
    tracing_subscriber::fmt().with_env_filter(env_filter).init();

    let out_dir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/grid-issues".to_owned());
    std::fs::create_dir_all(&out_dir)?;

    // Scenario A: crowded workspaces, move the focused window down (focus follows) and check
    // whether the flying window is covered by the destination grid's other cells.
    {
        let mut env = make_env()?;
        let mut layout = make_layout(&env.clock, env.size, &env.output);
        let colors = [
            [0.15, 0.64, 0.41, 1.],
            [0.10, 0.40, 0.90, 1.],
            [0.88, 0.11, 0.14, 1.],
            [0.85, 0.60, 0.10, 1.],
            [0.55, 0.20, 0.55, 1.],
            [0.20, 0.60, 0.60, 1.],
        ];
        let mut windows = Vec::new();
        for id in 0..6 {
            windows.push(add_window(&mut layout, id, colors[id]));
        }
        // Move three windows to workspace 1 so both grids are crowded.
        layout.move_to_workspace(Some(&3), 1, ActivateWindow::No);
        layout.move_to_workspace(Some(&4), 1, ActivateWindow::No);
        layout.move_to_workspace(Some(&5), 1, ActivateWindow::No);
        // Focus the middle-left window: its flight path to the end of the destination grid
        // crosses the destination grid's other cells.
        layout.activate_window(&1);
        layout.toggle_grid_overview();

        for (t, name) in [
            (700, "a1_before"),
            (1050, "a2_move_start"),
            (1150, "a3_fly1"),
            (1250, "a4_fly2"),
            (1400, "a5_fly3"),
            (2000, "a6_settled"),
        ] {
            if t == 1050 {
                let now = Duration::from_millis(1050);
                env.clock.set_unadjusted(now);
                layout.advance_animations();
                layout.move_to_workspace(None, 1, ActivateWindow::Yes);
            }
            capture(&mut env, &mut layout, t, &format!("{out_dir}/{name}.png"))?;
        }
        drop(windows);
    }

    // Scenario B: minimize a window, then open and close the grid, tracking its thumbnail.
    {
        let mut env = make_env()?;
        let mut layout = make_layout(&env.clock, env.size, &env.output);
        let mut windows = Vec::new();
        for id in 0..3 {
            windows.push(add_window(&mut layout, id, [0.15, 0.64, 0.41, 1.]));
        }
        layout.activate_window(&2);

        let mut step = |layout: &mut niri::layout::Layout<TestWindow>,
                        env: &mut Env,
                        t: u64,
                        name: &str|
         -> anyhow::Result<()> {
            let now = Duration::from_millis(t);
            env.clock.set_unadjusted(now);
            layout.advance_animations();
            match name {
                "b1_minimize" => {
                    layout.set_window_minimized(&0, true);
                }
                "b2_open_grid" => {
                    layout.open_grid_overview();
                }
                "b6_close_grid" => {
                    layout.close_grid_overview();
                }
                _ => {}
            }
            capture(env, layout, t, &format!("{out_dir}/{name}.png"))
        };

        step(&mut layout, &mut env, 400, "b0_grid_closed")?;
        step(&mut layout, &mut env, 600, "b1_minimize")?;
        step(&mut layout, &mut env, 1000, "b2_open_grid")?;
        for (t, name) in [
            (1080, "b3_open_early"),
            (1150, "b4_open_mid"),
            (1400, "b5_open_done"),
        ] {
            let now = Duration::from_millis(t);
            env.clock.set_unadjusted(now);
            layout.advance_animations();
            capture(&mut env, &mut layout, t, &format!("{out_dir}/{name}.png"))?;
        }
        step(&mut layout, &mut env, 2000, "b6_close_grid")?;
        for (t, name) in [
            (2080, "b7_close_early"),
            (2150, "b8_close_mid"),
            (2500, "b9_close_done"),
        ] {
            let now = Duration::from_millis(t);
            env.clock.set_unadjusted(now);
            layout.advance_animations();
            capture(&mut env, &mut layout, t, &format!("{out_dir}/{name}.png"))?;
        }
        drop(windows);
    }

    // Scenario D: a second move before the first fly-in finishes should retarget smoothly
    // instead of teleporting.
    {
        let mut env = make_env()?;
        let mut layout = make_layout(&env.clock, env.size, &env.output);
        let mut windows = Vec::new();
        for id in 0..4 {
            windows.push(add_window(&mut layout, id, [0.15, 0.64, 0.41, 1.]));
        }
        windows[2].set_color([0.10, 0.40, 0.90, 1.]);
        layout.activate_window(&2);
        // Keep workspace 1 populated.
        layout.move_to_workspace(Some(&3), 1, ActivateWindow::No);
        layout.activate_window(&2);
        layout.toggle_grid_overview();

        let mut moved1 = false;
        let mut moved2 = false;
        for (t, name) in [
            (700, "d1_before"),
            (1050, "d2_move1"),
            (1150, "d3_fly1"),
            (1190, "d4_pre_move2"),
            (1210, "d5_move2"),
            (1250, "d6_fly2"),
            (1350, "d7_fly2_mid"),
            (1600, "d8_settled"),
        ] {
            let now = Duration::from_millis(t);
            env.clock.set_unadjusted(now);
            layout.advance_animations();
            if !moved1 && t >= 1050 {
                moved1 = true;
                layout.move_to_workspace(None, 1, ActivateWindow::Yes);
            }
            if !moved2 && t >= 1200 {
                moved2 = true;
                layout.move_to_workspace(None, 0, ActivateWindow::Yes);
            }
            capture(&mut env, &mut layout, t, &format!("{out_dir}/{name}.png"))?;
        }
        drop(windows);
    }

    // Scenario C: grab a minimized window in the grid should un-minimize it and start the move.
    {
        let mut env = make_env()?;
        let mut layout = make_layout(&env.clock, env.size, &env.output);
        let mut windows = Vec::new();
        for id in 0..3 {
            windows.push(add_window(&mut layout, id, [0.15, 0.64, 0.41, 1.]));
        }
        layout.activate_window(&2);
        layout.set_window_minimized(&0, true);
        eprintln!(
            "C: minimized after minimize = {}",
            layout.is_window_minimized(&0)
        );
        layout.toggle_grid_overview();
        eprintln!(
            "C: minimized after grid open = {}",
            layout.is_window_minimized(&0)
        );

        let now = Duration::from_millis(800);
        env.clock.set_unadjusted(now);
        layout.advance_animations();

        // Find the minimized window's grid cell by hit-testing (the same path the pointer
        // input uses) and try to grab it.
        let mon = layout.monitor_for_output(&env.output).unwrap();
        let mut grab_pos = None;
        'outer: for y in (20..700).step_by(20) {
            for x in (20..1260).step_by(20) {
                let pos = smithay::utils::Point::<f64, Logical>::from((x as f64, y as f64));
                if let Some((win, _)) = mon.window_under(pos) {
                    if win.id() == &0 {
                        grab_pos = Some(pos);
                        break 'outer;
                    }
                }
            }
        }
        let grab_pos = grab_pos.expect("minimized window should be hit-testable in the grid");
        eprintln!("C: hit-test found minimized window at {grab_pos:?}");

        let began = layout.interactive_move_begin(0, &env.output, grab_pos);
        eprintln!("C: interactive_move_begin on minimized window = {began}");
        eprintln!(
            "C: minimized after grab = {}",
            layout.is_window_minimized(&0)
        );
        drop(windows);
    }

    Ok(())
}
