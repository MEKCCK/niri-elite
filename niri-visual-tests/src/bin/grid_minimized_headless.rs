//! Headless captures of how minimized windows look and behave in the grid overview.
//!
//! Scenario A: the minimized cell's highlight, while the grid opens, sits open, and closes.
//! Scenario B: moving a visible window over a minimized cell steps one cell at a time.
//! Scenario C: restoring a minimized window from the grid must not make the neighbors bounce.

use std::fs::File;
use std::io::BufWriter;
use std::time::Duration;

use anyhow::Context;
use niri::animation::Clock;
use niri::layout::{ActivateWindow, AddWindowTarget, LayoutElement as _, Options, SizingMode};
use niri::render_helpers::{
    copy_framebuffer, create_texture, resources, shaders, RenderCtx, RenderIntent, RenderTarget,
};
use niri_config::{OutputName, PresetSize};
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
    let _ = frame.finish().context("finishing frame")?;

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

/// Builds a layout with the default appearance, so the only colored frame on screen is the
/// minimized highlight. To compare highlight styles, override
/// `options.grid_overview.minimized_highlight` here.
fn make_layout(
    clock: &Clock,
    _size: Size<i32, Logical>,
    output: &Output,
) -> niri::layout::Layout<TestWindow> {
    let mut layout =
        niri::layout::Layout::<TestWindow>::with_options(clock.clone(), Options::default());
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

/// Renders a frame and, as a numeric trace, the x of the widest element in each screen third.
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

    let mut geos: Vec<Rectangle<i32, Physical>> = elements
        .iter()
        .map(|elem| elem.geometry(Scale::from(1.)))
        .filter(|geo| geo.size.w > 40 && geo.size.h > 40)
        .collect();
    geos.sort_by_key(|geo| geo.loc.x);
    let trace: Vec<String> = geos
        .iter()
        .map(|geo| format!("{}+{}x{}", geo.loc.x, geo.size.w, geo.size.h))
        .collect();
    eprintln!(
        "{t_ms:>5}ms {}: {}",
        path.rsplit('/').next().unwrap(),
        trace.join(" ")
    );

    let bytes = render_frame(&mut env.renderer, env.size.to_physical(1), elements)?;
    save_png(path, env.size.w as u32, env.size.h as u32, &bytes)?;
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let env_filter = tracing_subscriber::EnvFilter::builder()
        .parse_lossy(std::env::var("RUST_LOG").unwrap_or_default());
    tracing_subscriber::fmt().with_env_filter(env_filter).init();

    let out_dir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/grid-minimized".to_owned());
    std::fs::create_dir_all(&out_dir)?;

    let colors = [
        [0.15, 0.64, 0.41, 1.],
        [0.10, 0.40, 0.90, 1.],
        [0.55, 0.20, 0.55, 1.],
    ];

    // Scenario A: the minimized window's cell is marked while the grid is open.
    {
        let mut env = make_env()?;
        let mut layout = make_layout(&env.clock, env.size, &env.output);
        let mut windows = Vec::new();
        for id in 0..3 {
            windows.push(add_window(&mut layout, id, colors[id]));
        }
        layout.activate_window(&2);
        layout.set_window_minimized(&1, true);
        layout.toggle_grid_overview();

        for (t, name) in [
            (1060, "a1_open_early"),
            (1200, "a2_open_mid"),
            (2000, "a3_open"),
        ] {
            capture(&mut env, &mut layout, t, &format!("{out_dir}/{name}.png"))?;
        }

        layout.close_grid_overview();
        for (t, name) in [
            (2060, "a4_close_early"),
            (2200, "a5_close_mid"),
            (3000, "a6_closed"),
        ] {
            capture(&mut env, &mut layout, t, &format!("{out_dir}/{name}.png"))?;
        }
        drop(windows);
    }

    // Scenario B: move the rightmost window left: it must land on the minimized cell's spot.
    {
        let mut env = make_env()?;
        let mut layout = make_layout(&env.clock, env.size, &env.output);
        let mut windows = Vec::new();
        for id in 0..3 {
            windows.push(add_window(&mut layout, id, colors[id]));
        }
        layout.activate_window(&2);
        layout.set_window_minimized(&1, true);
        layout.toggle_grid_overview();

        capture(
            &mut env,
            &mut layout,
            2000,
            &format!("{out_dir}/b1_open.png"),
        )?;
        layout.move_left();
        for (t, name) in [
            (2080, "b2_move_early"),
            (2200, "b3_move_mid"),
            (3000, "b4_moved"),
        ] {
            capture(&mut env, &mut layout, t, &format!("{out_dir}/{name}.png"))?;
        }
        drop(windows);
    }

    // Scenario C: restore the minimized window from the grid. The other cells must fly straight
    // to their new places instead of starting towards the freed spot and turning back.
    {
        let mut env = make_env()?;
        let mut layout = make_layout(&env.clock, env.size, &env.output);
        let mut windows = Vec::new();
        for id in 0..3 {
            windows.push(add_window(&mut layout, id, colors[id]));
        }
        layout.activate_window(&2);
        layout.set_window_minimized(&1, true);
        layout.toggle_grid_overview();

        capture(
            &mut env,
            &mut layout,
            2000,
            &format!("{out_dir}/c1_open.png"),
        )?;
        // Focus the minimized cell, then confirm it, which restores and closes the grid.
        layout.focus_left();
        eprintln!("C: grid focus = {:?}", layout.grid_focused_window_id());
        capture(
            &mut env,
            &mut layout,
            2100,
            &format!("{out_dir}/c2_focused.png"),
        )?;
        layout.confirm_grid_selection();
        for (t, name) in [
            (2130, "c3_restore_early"),
            (2180, "c4_restore_mid"),
            (2260, "c5_restore_late"),
            (2400, "c6_restore_end"),
            (3200, "c7_settled"),
        ] {
            capture(&mut env, &mut layout, t, &format!("{out_dir}/{name}.png"))?;
        }
        drop(windows);
    }

    // Scenario D: minimizing an already-minimized cell restores it in place, with the grid
    // staying open on it and its highlight going away.
    {
        let mut env = make_env()?;
        let mut layout = make_layout(&env.clock, env.size, &env.output);
        let mut windows = Vec::new();
        for id in 0..3 {
            windows.push(add_window(&mut layout, id, colors[id]));
        }
        layout.activate_window(&2);
        layout.set_window_minimized(&1, true);
        layout.toggle_grid_overview();

        capture(
            &mut env,
            &mut layout,
            2000,
            &format!("{out_dir}/d1_minimized.png"),
        )?;
        layout.focus_left();
        capture(
            &mut env,
            &mut layout,
            2100,
            &format!("{out_dir}/d2_focused.png"),
        )?;
        layout.unminimize_window(&1, false);
        for (t, name) in [
            (2130, "d3_fadeout_early"),
            (2180, "d4_fadeout_mid"),
            (2260, "d5_fadeout_late"),
            (3000, "d6_restored"),
        ] {
            capture(&mut env, &mut layout, t, &format!("{out_dir}/{name}.png"))?;
        }

        // And back: minimizing while the grid is up fades the frame in.
        layout.set_window_minimized(&1, true);
        for (t, name) in [
            (3030, "d7_fadein_early"),
            (3080, "d8_fadein_mid"),
            (3160, "d9_fadein_late"),
            (4000, "d10_minimized"),
        ] {
            capture(&mut env, &mut layout, t, &format!("{out_dir}/{name}.png"))?;
        }
        eprintln!(
            "D: minimized={} grid_open={} grid_focus={:?}",
            layout.is_window_minimized(&1),
            layout.is_grid_overview_open(),
            layout.grid_focused_window_id(),
        );
        drop(windows);
    }

    Ok(())
}
