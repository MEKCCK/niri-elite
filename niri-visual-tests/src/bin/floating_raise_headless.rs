use std::fs::File;
use std::io::BufWriter;
use std::time::Duration;

use anyhow::Context;
use niri::animation::Clock;
use niri::layout::{ActivateWindow, AddWindowTarget, LayoutElement as _, Options, SizingMode};
use niri::render_helpers::{
    copy_framebuffer, create_texture, resources, shaders, RenderCtx, RenderIntent, RenderTarget,
};
use niri::utils::transaction::{Transaction, TransactionBlocker};
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

fn main() -> anyhow::Result<()> {
    // Headless software GL context.
    make_sure_egl_is_loaded().context("loading EGL")?;
    let display = unsafe { EGLDisplay::new(EGLSurfacelessDisplay) }
        .context("creating surfaceless EGL display")?;
    let context = EGLContext::new(&display).context("creating EGL context")?;
    unsafe { context.make_current() }.context("making EGL context current")?;

    let mut renderer = unsafe { GlesRenderer::new(context) }.context("creating GlesRenderer")?;
    resources::init(&mut renderer);
    shaders::init(&mut renderer);

    let enabled = std::env::args().nth(2).as_deref() != Some("off");

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
            focused_window_above_floating: enabled,
            ..Default::default()
        },
        ..Default::default()
    };

    let mut layout = niri::layout::Layout::<TestWindow>::with_options(clock.clone(), options);
    layout.add_output(output.clone(), None);

    // 0: green tiled, 1: red tiled, 2: blue floating over the right part of the green one.
    let window_colors = [
        [0.15, 0.64, 0.41, 1.],
        [0.88, 0.11, 0.14, 1.],
        [0.10, 0.40, 0.90, 1.],
    ];

    let mut windows = Vec::new();
    for id in 0..3 {
        let is_floating = id == 2;
        let mut window = TestWindow::freeform(id);
        window.set_color(window_colors[id]);
        let ws = layout.active_workspace().unwrap();
        let min_size = window.min_size();
        let max_size = window.max_size();
        let new_size = if is_floating {
            Size::from((400, 300))
        } else {
            ws.new_window_size(
                Some(PresetSize::Proportion(0.5)),
                None,
                false,
                window.rules(),
                (min_size, max_size),
            )
        };
        window.request_size(new_size, SizingMode::Normal, false, None);
        window.communicate();

        layout.add_window(
            window.clone(),
            AddWindowTarget::Auto,
            Some(PresetSize::Proportion(0.5)),
            None,
            false,
            is_floating,
            ActivateWindow::Yes,
        );
        windows.push(window);
    }

    // Overlap of the green tiled window and the blue floating one, in output coordinates.
    let probe = {
        let ws = layout.active_workspace().unwrap();
        let rect_of = |id: usize| {
            ws.tiles_with_render_positions()
                .find_map(|(tile, pos, _)| {
                    (*tile.window().id() == id).then(|| Rectangle::new(pos, tile.tile_size()))
                })
                .unwrap()
        };
        let overlap = rect_of(0).intersection(rect_of(2));
        let overlap = overlap.expect("the floating window must overlap the green one");
        let p: Point<f64, Logical> = overlap.loc + overlap.size.downscale(2.).to_point();
        p.to_i32_round::<i32>()
    };
    eprintln!("probe at {probe:?}");

    let out_dir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/floating-raise-vis".to_owned());
    std::fs::create_dir_all(&out_dir)?;

    type Step = fn(&mut niri::layout::Layout<TestWindow>);
    let steps: &[(u64, &str, Option<Step>)] = &[
        (100, "01_floating_focused", None),
        (200, "02_focus_tiling", Some(|l| l.focus_tiling())),
        (300, "03_focus_floating", Some(|l| l.focus_floating())),
        (
            400,
            "04_ffm_tiled",
            Some(|l| l.activate_window_without_raising(&0)),
        ),
        (500, "05_click_tiled", Some(|l| l.activate_window(&0))),
        (
            600,
            "06_grid_open_start",
            Some(|l| l.toggle_grid_overview()),
        ),
        (700, "07_grid_opening", None),
        (900, "08_grid_opening", None),
        (2500, "09_grid_open", None),
        (
            2600,
            "10_grid_close_start",
            Some(|l| l.toggle_grid_overview()),
        ),
        (2800, "11_grid_closing", None),
        (3000, "12_grid_closing", None),
        (5000, "13_grid_closed", None),
        // Close the focused floating window: focus goes back to the green tiled window, which
        // must not cover the closing animation.
        (5100, "14_focus_floating", Some(|l| l.focus_floating())),
        (5200, "15_close_floating", None),
        (5230, "16_floating_closing", None),
        (5300, "17_floating_closing", None),
        (6500, "18_floating_closed", None),
    ];
    let mut closed_floating = false;

    for (t_ms, name, step) in steps {
        clock.set_unadjusted(Duration::from_millis(*t_ms));
        if let Some(step) = step {
            step(&mut layout);
        }
        if !closed_floating && *t_ms >= 5200 {
            closed_floating = true;
            layout.update_render_elements(Some(&output));
            layout.store_unmap_snapshot(&mut renderer, None, false, &2);
            layout.start_close_animation_for_window(
                &mut renderer,
                &2,
                TransactionBlocker::completed(),
            );
            layout.remove_window(&2, Transaction::new());
        }
        layout.advance_animations();
        layout.update_render_elements(Some(&output));

        let mut elements = Vec::new();
        let ctx = RenderCtx {
            renderer: &mut renderer,
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

        let bytes = render_frame(&mut renderer, size.to_physical(1), elements)?;

        let idx = ((probe.y * size.w + probe.x) * 4) as usize;
        let (b, g, r) = (bytes[idx], bytes[idx + 1], bytes[idx + 2]);
        let what = if b > g && b > r {
            "blue (floating)"
        } else if g > r {
            "green (tiled)"
        } else {
            "other"
        };
        let focus = layout.focus().map(|w| *w.id());
        eprintln!("{name}: focus={focus:?} probe=rgb({r},{g},{b}) -> {what}");

        let path = format!("{out_dir}/{name}.png");
        save_png(&path, size.w as u32, size.h as u32, &bytes)?;
    }

    drop(windows);

    Ok(())
}
