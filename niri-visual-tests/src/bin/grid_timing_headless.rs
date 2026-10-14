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

    // Open the grid at 100 ms and close it at 2100 ms, sampling every 8 ms.
    let mut times: Vec<u64> = (100..1300).step_by(8).collect();
    times.extend((2100..3300).step_by(8));
    for t_ms in times {
        clock.set_unadjusted(Duration::from_millis(t_ms));
        if t_ms == 100 || t_ms == 2100 {
            layout.toggle_grid_overview();
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

        // Left edge and width of the green window.
        let mut x0 = i32::MAX;
        let mut x1 = i32::MIN;
        let y = size.h / 2;
        for yy in [y - 100, y, y + 100] {
            for x in 0..size.w {
                let i = ((yy * size.w + x) * 4) as usize;
                let (b, g, r) = (bytes[i], bytes[i + 1], bytes[i + 2]);
                if (r as i32 - 38).abs() < 14
                    && (g as i32 - 163).abs() < 14
                    && (b as i32 - 105).abs() < 14
                {
                    x0 = x0.min(x);
                    x1 = x1.max(x);
                }
            }
        }
        println!("{t_ms} {x0} {}", x1 - x0);
    }

    drop(windows);

    Ok(())
}
