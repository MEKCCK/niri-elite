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

    // Optional grid focus-ring-width override, in logical pixels.
    // Usage: grid-focus-ring-headless OUT_DIR [none|grow|slide] [GRID_RING_WIDTH]
    let transition = match std::env::args().nth(2).as_deref() {
        Some("none") => niri_config::GridFocusRingTransition::None,
        Some("slide") => niri_config::GridFocusRingTransition::Slide,
        _ => niri_config::GridFocusRingTransition::Grow,
    };
    let grid_width: Option<f64> = std::env::args().nth(3).and_then(|w| w.parse().ok());

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
            border: niri_config::Border {
                off: true,
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
        grid_overview: niri_config::GridOverview {
            focus_ring_width: grid_width,
            focus_ring_transition: transition,
            ..Default::default()
        },
        ..Default::default()
    };

    let mut layout = niri::layout::Layout::<TestWindow>::with_options(clock.clone(), options);
    layout.add_output(output.clone(), None);

    let window_colors = [
        [0.15, 0.64, 0.41, 1.],
        [0.88, 0.11, 0.14, 1.],
        [0.10, 0.40, 0.90, 1.],
        [0.55, 0.20, 0.70, 1.],
        [0.20, 0.55, 0.60, 1.],
        [0.60, 0.60, 0.20, 1.],
    ];

    let mut windows = Vec::new();
    for id in 0..6 {
        let is_floating = false;
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

    layout.activate_window(&0);

    let out_dir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/grid-focus-ring-vis".to_owned());
    std::fs::create_dir_all(&out_dir)?;

    type Step = fn(&mut niri::layout::Layout<TestWindow>);
    let steps: &[(u64, &str, Option<Step>)] = &[
        (100, "01_normal", None),
        (200, "02_open_start", Some(|l| l.toggle_grid_overview())),
        (300, "03_opening", None),
        (2000, "04_open", None),
        // One focus move, frame by frame (the default animation is 150 ms).
        (2100, "05_move", Some(|l| l.focus_right())),
        (2120, "06_move", None),
        (2140, "07_move", None),
        (2170, "08_move", None),
        (2200, "09_move", None),
        (2250, "10_move", None),
        (2400, "11_moved", None),
        // Two quick moves back, the second one interrupting the first.
        (2500, "12_back", Some(|l| l.focus_left())),
        (2540, "13_back", None),
        (2560, "14_forth", Some(|l| l.focus_right())),
        (2600, "15_forth", None),
        (2650, "16_forth", None),
        (2900, "17_settled", None),
        (3000, "18_close_start", Some(|l| l.toggle_grid_overview())),
        (3100, "19_closing", None),
        (5000, "20_closed", None),
        // Outside the grid: the rings of the normal layout grow and shrink too.
        (5100, "21_layout_left", Some(|l| l.focus_left())),
        (5150, "22_layout", None),
        (5250, "23_layout", None),
        (5600, "24_layout_settled", None),
    ];
    for (t_ms, name, step) in steps {
        clock.set_unadjusted(Duration::from_millis(*t_ms));
        if let Some(step) = step {
            step(&mut layout);
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

        // Measure the rings: orange pixels on the row through the middle of the first row of
        // cells, counted per cell (both sides of a ring, so twice its width), and where the
        // orange starts and ends on that row.
        let orange = |x: i32, y: i32| {
            let i = ((y * size.w + x) * 4) as usize;
            let (b, g, r) = (bytes[i], bytes[i + 1], bytes[i + 2]);
            r > 200 && (130..200).contains(&g) && b < 110
        };
        // The cells of windows 0 (green) and 1 (red), found by their colors.
        let bbox = |rgb: (u8, u8, u8)| {
            let mut bb: Option<(i32, i32, i32, i32)> = None;
            for y in (0..size.h).step_by(2) {
                for x in (0..size.w).step_by(2) {
                    let i = ((y * size.w + x) * 4) as usize;
                    let (b, g, r) = (bytes[i], bytes[i + 1], bytes[i + 2]);
                    let close = |a: u8, b: u8| (a as i32 - b as i32).abs() < 12;
                    if close(r, rgb.0) && close(g, rgb.1) && close(b, rgb.2) {
                        bb = Some(match bb {
                            None => (x, y, x, y),
                            Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
                        });
                    }
                }
            }
            bb
        };
        let mut line = String::new();
        let cells = [bbox((38, 163, 105)), bbox((224, 28, 36))];
        if let [Some(c0), Some(_)] = cells {
            let y = (c0.1 + c0.3) / 2;
            for (id, cell) in cells.iter().enumerate() {
                let (x0, _, x1, _) = cell.unwrap();
                let n = ((x0 - 14).max(0)..(x1 + 14).min(size.w))
                    .filter(|&x| orange(x, y))
                    .count();
                line += &format!(" cell{id}={:>4.1}px", n as f64 / 2.);
            }
            let xs: Vec<i32> = (0..size.w).filter(|&x| orange(x, y)).collect();
            if let (Some(a), Some(b)) = (xs.first(), xs.last()) {
                line += &format!("  orange x {a}..{b}");
            }
        }
        let focus = layout.focus().map(|w| *w.id());
        eprintln!("{name}: focus={focus:?}{line}");

        let path = format!("{out_dir}/{name}.png");
        save_png(&path, size.w as u32, size.h as u32, &bytes)?;
    }

    drop(windows);

    Ok(())
}
