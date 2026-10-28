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

    // Layout setup, mirroring the visual test cases.
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
            ..Default::default()
        },
        ..Default::default()
    };

    let mut layout = niri::layout::Layout::<TestWindow>::with_options(clock.clone(), options);
    layout.add_output(output.clone(), None);

    // Distinct colors so that the flying window can be told apart from the source grid's
    // windows when checking the render z-order.
    let window_colors = [
        [0.15, 0.64, 0.41, 1.],
        [0.88, 0.11, 0.14, 1.],
        [0.10, 0.40, 0.90, 1.],
        [0.85, 0.60, 0.10, 1.],
    ];

    let mut windows = Vec::new();
    for id in 0..4 {
        let mut window = TestWindow::freeform(id);
        window.set_color(window_colors[id]);
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
        windows.push(window);
    }
    layout.activate_window(&2);

    // Keep workspace 1 populated so it survives workspace clean-up during the later moves.
    layout.move_to_workspace(Some(&3), 1, ActivateWindow::No);
    layout.activate_window(&2);

    layout.toggle_grid_overview();

    let out_dir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/grid-vis".to_owned());
    std::fs::create_dir_all(&out_dir)?;

    // Capture frames around the moves:
    // - t = 1000ms: move the focused window down to workspace 1 (focus follows).
    // - t = 5000ms: move it back up to workspace 0 (focus follows).
    // - t = 9000ms: move it down again WITHOUT following focus, so the view stays on the source
    //   grid while the blue window flies down across it. This lets the z-order check below see
    //   whether the flying window draws above the source grid's windows.
    let mut moved_down = false;
    let mut moved_up = false;
    let mut moved_no_focus = false;
    let mut moved_column = false;
    let frames: &[(u64, &str)] = &[
        (300, "01_grid_open"),
        (900, "02_before_move"),
        (1050, "03_move_start"),
        (1200, "04_flying"),
        (1500, "05_mid_flight"),
        (2000, "06_late_flight"),
        (3500, "07_settled"),
        (4900, "08_before_up"),
        (5050, "09_up_start"),
        (5200, "10_up_flying"),
        (5500, "11_up_mid"),
        (6500, "12_up_settled"),
        (8850, "13_before_no_focus"),
        (9100, "14_no_focus_fly1"),
        (9130, "15a_no_focus_dense1"),
        (9160, "15b_no_focus_dense2"),
        (9190, "15c_no_focus_dense3"),
        (9220, "15d_no_focus_dense4"),
        (9250, "15_no_focus_fly2"),
        (9400, "16_no_focus_fly3"),
        (9600, "17_no_focus_fly4"),
        (10500, "18_no_focus_settled"),
        (12900, "19_before_column_move"),
        (13050, "20_column_move_start"),
        (13150, "21_column_fly1"),
        (13250, "22_column_fly2"),
        (13400, "23_column_fly3"),
        (14200, "24_column_settled"),
    ];

    for (t_ms, name) in frames {
        let now = Duration::from_millis(*t_ms);
        clock.set_unadjusted(now);
        layout.advance_animations();

        if !moved_down && *t_ms >= 1000 {
            moved_down = true;
            layout.move_to_workspace(None, 1, ActivateWindow::Yes);
        }
        if !moved_up && *t_ms >= 5000 {
            moved_up = true;
            layout.move_to_workspace(None, 0, ActivateWindow::Yes);
        }
        if !moved_no_focus && *t_ms >= 9000 {
            moved_no_focus = true;
            layout.move_to_workspace(None, 1, ActivateWindow::No);
        }
        if !moved_column && *t_ms >= 13000 {
            moved_column = true;
            layout.move_column_to_workspace(1, true);
        }

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

        let path = format!("{out_dir}/{name}.png");
        save_png(&path, size.w as u32, size.h as u32, &bytes)?;
        eprintln!("wrote {path}");
    }

    drop(windows);

    Ok(())
}
