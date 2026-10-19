use std::collections::HashMap;
use std::f64::consts::{FRAC_PI_2, PI, TAU};

use anyhow::Context as _;
use niri_ipc::PickedColor;
use pango::FontDescription;
use pangocairo::cairo::{self, ImageSurface};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::backend::renderer::{ExportMem, Texture as _};
use smithay::output::Output;
use smithay::utils::{Logical, Physical, Point, Size, Transform};

use crate::render_helpers::primary_gpu_texture::PrimaryGpuTextureRenderElement;
use crate::render_helpers::texture::{TextureBuffer, TextureRenderElement};
use crate::render_helpers::RenderTarget;
use crate::ui::screenshot_ui::OutputScreenshot;
use crate::utils::to_physical_precise_round;

/// Diameter of the magnifier, in logical pixels.
const MAGNIFIER_DIAMETER: i32 = 230;
/// Default zoom: physical pixels per screen pixel, same as hyprpicker.
const DEFAULT_ZOOM: f64 = 10.;
const MIN_ZOOM: f64 = 2.;
const MAX_ZOOM: f64 = 60.;
/// Zoom factor applied per scroll wheel notch.
const ZOOM_STEP: f64 = 1.2;
/// Width of the outer ring filled with the picked color.
const RING_WIDTH: f64 = 6.;
const LABEL_PADDING_X: i32 = 6;
const LABEL_PADDING_Y: i32 = 2;
const LABEL_RADIUS: i32 = 6;
const FONT: &str = "monospace bold 13px";

/// Frozen-screen color picker with a magnifier that follows the pointer.
pub enum PickColorUi {
    Closed,
    Open {
        output_data: HashMap<Output, OutputData>,
        magnifier: Option<Magnifier>,
        zoom: f64,
    },
}

pub struct OutputData {
    // Output, screencast, screen capture.
    screenshot: [OutputScreenshot; 3],
    size: Size<i32, Physical>,
    scale: f64,
    /// RGBA8 copy of the output screenshot, used for reading colors.
    pixels: Vec<u8>,
}

pub struct Magnifier {
    output: Output,
    location: Point<f64, Logical>,
    buffer: TextureBuffer<GlesTexture>,
}

impl PickColorUi {
    pub fn new() -> Self {
        Self::Closed
    }

    pub fn is_open(&self) -> bool {
        matches!(self, Self::Open { .. })
    }

    pub fn open(
        &mut self,
        renderer: &mut GlesRenderer,
        screenshots: HashMap<Output, [OutputScreenshot; 3]>,
    ) -> bool {
        let output_data: HashMap<_, _> = screenshots
            .into_iter()
            .filter_map(|(output, screenshot)| {
                let scale = output.current_scale().fractional_scale();
                let size = screenshot[0].texture().size();
                let size = Size::<i32, Physical>::from((size.w, size.h));
                let pixels = match download(renderer, screenshot[0].texture(), size) {
                    Ok(pixels) => pixels,
                    Err(err) => {
                        warn!("error downloading screenshot for color picking: {err:?}");
                        return None;
                    }
                };
                let data = OutputData {
                    screenshot,
                    size,
                    scale,
                    pixels,
                };
                Some((output, data))
            })
            .collect();

        if output_data.is_empty() {
            return false;
        }

        *self = Self::Open {
            output_data,
            magnifier: None,
            zoom: DEFAULT_ZOOM,
        };
        true
    }

    pub fn close(&mut self) -> bool {
        if !self.is_open() {
            return false;
        }
        *self = Self::Closed;
        true
    }

    /// Reads the color at the given output-local position from the frozen screen.
    pub fn color_at(&self, output: &Output, pos: Point<f64, Logical>) -> Option<PickedColor> {
        let Self::Open { output_data, .. } = self else {
            return None;
        };
        let data = output_data.get(output)?;
        let pos = data.pixel_pos(pos);
        let [r, g, b] = data.pixel(pos.x, pos.y);
        Some(PickedColor {
            rgb: [r, g, b].map(|v| f64::from(v) / 255.),
        })
    }

    /// Zooms the magnifier in (positive steps) or out (negative steps).
    pub fn zoom(&mut self, steps: f64) {
        if let Self::Open { zoom, .. } = self {
            *zoom = (*zoom * ZOOM_STEP.powf(steps)).clamp(MIN_ZOOM, MAX_ZOOM);
        }
    }

    /// Moves the magnifier to the given output-local pointer position.
    pub fn update_pointer(
        &mut self,
        renderer: &mut GlesRenderer,
        output: Option<(&Output, Point<f64, Logical>)>,
    ) {
        let Self::Open {
            output_data,
            magnifier,
            zoom,
        } = self
        else {
            return;
        };

        let Some((output, pos)) = output else {
            *magnifier = None;
            return;
        };
        let Some(data) = output_data.get(output) else {
            *magnifier = None;
            return;
        };

        match render_magnifier(renderer, data, pos, *zoom) {
            Ok((buffer, center)) => {
                let location = pos - center.to_f64().to_logical(data.scale);
                *magnifier = Some(Magnifier {
                    output: output.clone(),
                    location,
                    buffer,
                });
            }
            Err(err) => {
                warn!("error rendering color picker magnifier: {err:?}");
                *magnifier = None;
            }
        }
    }

    pub fn render_output(
        &self,
        output: &Output,
        target: RenderTarget,
        push: &mut dyn FnMut(PrimaryGpuTextureRenderElement),
    ) {
        let Self::Open {
            output_data,
            magnifier,
            ..
        } = self
        else {
            return;
        };

        if let Some(magnifier) = magnifier {
            if magnifier.output == *output {
                push(PrimaryGpuTextureRenderElement(
                    TextureRenderElement::from_texture_buffer(
                        magnifier.buffer.clone(),
                        magnifier.location,
                        1.,
                        None,
                        None,
                        Kind::Unspecified,
                    ),
                ));
            }
        }

        let Some(data) = output_data.get(output) else {
            return;
        };
        let index = match target {
            RenderTarget::Output => 0,
            RenderTarget::Screencast => 1,
            RenderTarget::ScreenCapture => 2,
        };
        push(data.screenshot[index].buffer().clone());
    }
}

impl Default for PickColorUi {
    fn default() -> Self {
        Self::new()
    }
}

impl OutputData {
    fn pixel_pos(&self, pos: Point<f64, Logical>) -> Point<i32, Physical> {
        let pos: Point<i32, Physical> = pos.to_physical_precise_floor(self.scale);
        Point::from((
            pos.x.clamp(0, self.size.w - 1),
            pos.y.clamp(0, self.size.h - 1),
        ))
    }

    fn pixel(&self, x: i32, y: i32) -> [u8; 3] {
        let idx = (y as usize * self.size.w as usize + x as usize) * 4;
        match self.pixels.get(idx..idx + 3) {
            Some(px) => [px[0], px[1], px[2]],
            None => [0, 0, 0],
        }
    }

    /// Returns the pixel color, or `None` if outside the screen.
    fn pixel_checked(&self, x: i32, y: i32) -> Option<[u8; 3]> {
        if x < 0 || y < 0 || x >= self.size.w || y >= self.size.h {
            return None;
        }
        Some(self.pixel(x, y))
    }
}

fn download(
    renderer: &mut GlesRenderer,
    texture: &GlesTexture,
    size: Size<i32, Physical>,
) -> anyhow::Result<Vec<u8>> {
    let rect = smithay::utils::Rectangle::from_size(size)
        .to_logical(1)
        .to_buffer(1, Transform::Normal, &Size::from((1, 1)));
    let mapping = renderer
        .copy_texture(texture, rect, Fourcc::Abgr8888)
        .context("error copying texture")?;
    let pixels = renderer
        .map_texture(&mapping)
        .context("error mapping texture")?;
    Ok(pixels.to_vec())
}

/// Renders the magnifier: magnified pixels inside a ring filled with the picked color, and the
/// hex value in the lower part of the lens.
///
/// Returns the buffer and the physical position of the magnifier center within it.
fn render_magnifier(
    renderer: &mut GlesRenderer,
    data: &OutputData,
    pos: Point<f64, Logical>,
    zoom: f64,
) -> anyhow::Result<(TextureBuffer<GlesTexture>, Point<i32, Physical>)> {
    let _span = tracy_client::span!("pick_color_ui::render_magnifier");

    let scale = data.scale;
    let center = data.pixel_pos(pos);
    let [r, g, b] = data.pixel(center.x, center.y);
    let (rf, gf, bf) = (
        f64::from(r) / 255.,
        f64::from(g) / 255.,
        f64::from(b) / 255.,
    );
    let hex = format!("#{r:02X}{g:02X}{b:02X}");

    let cell = zoom;
    let diameter: i32 = to_physical_precise_round(scale, MAGNIFIER_DIAMETER);
    let ring: f64 = to_physical_precise_round(scale, RING_WIDTH);
    let edge: f64 = to_physical_precise_round(scale, 1.);
    let padding_x: i32 = to_physical_precise_round(scale, LABEL_PADDING_X);
    let padding_y: i32 = to_physical_precise_round(scale, LABEL_PADDING_Y);
    let label_radius: f64 = to_physical_precise_round(scale, LABEL_RADIUS);

    let mut font = FontDescription::from_string(FONT);
    font.set_absolute_size(to_physical_precise_round(scale, font.size()));

    // Leave room for the ring outside the magnified area.
    let margin = ring.ceil() as i32 + 1;
    let size = diameter + margin * 2;

    let surface = ImageSurface::create(cairo::Format::ARgb32, size, size)?;
    let cr = cairo::Context::new(&surface)?;

    let c = f64::from(size) / 2.;
    let radius = f64::from(diameter) / 2.;

    // Copy the screen pixels covered by the lens, centered on the picked pixel.
    let half = (radius / cell).ceil() as i32 + 1;
    let count = half * 2 + 1;
    let mut region = ImageSurface::create(cairo::Format::Rgb24, count, count)?;
    {
        let stride = region.stride() as usize;
        let mut buf = region.data()?;
        for y in 0..count {
            for x in 0..count {
                let [pr, pg, pb] = data
                    .pixel_checked(center.x - half + x, center.y - half + y)
                    .unwrap_or([26, 26, 26]);
                let i = y as usize * stride + x as usize * 4;
                // Rgb24 is native-endian xRGB.
                buf[i..i + 4].copy_from_slice(&u32::from_be_bytes([0, pr, pg, pb]).to_ne_bytes());
            }
        }
    }
    // Top-left corner of the copied region in lens coordinates.
    let origin = c - cell / 2. - f64::from(half) * cell;

    // Magnified pixels, clipped to a circle.
    cr.save()?;
    cr.arc(c, c, radius, 0., TAU);
    cr.clip();
    let pattern = cairo::SurfacePattern::create(&region);
    pattern.set_filter(cairo::Filter::Nearest);
    let mut matrix = cairo::Matrix::identity();
    matrix.scale(1. / cell, 1. / cell);
    matrix.translate(-origin, -origin);
    pattern.set_matrix(matrix);
    cr.set_source(&pattern)?;
    cr.paint()?;

    // Outline the center pixel, contrasting with its color.
    let luma = 0.299 * f64::from(r) + 0.587 * f64::from(g) + 0.114 * f64::from(b);
    let outline = if luma > 140. { 0. } else { 1. };
    if cell >= 4. {
        cr.set_source_rgb(outline, outline, outline);
        cr.set_line_width(edge);
        cr.rectangle(
            c - cell / 2. - edge / 2.,
            c - cell / 2. - edge / 2.,
            cell + edge,
            cell + edge,
        );
        cr.stroke()?;
    }

    // The hex value in the lower part of the lens.
    let layout = pangocairo::functions::create_layout(&cr);
    layout.set_font_description(Some(&font));
    layout.set_text(&hex);
    let (text_w, text_h) = layout.pixel_size();
    let label_w = f64::from(text_w + padding_x * 2);
    let label_h = f64::from(text_h + padding_y * 2);
    let lx = c - label_w / 2.;
    let ly = c + radius * 0.6 - label_h / 2.;
    rounded_rect(&cr, lx, ly, label_w, label_h, label_radius);
    cr.set_source_rgba(0., 0., 0., 0.65);
    cr.fill()?;
    cr.move_to(lx + f64::from(padding_x), ly + f64::from(padding_y));
    cr.set_source_rgb(1., 1., 1.);
    pangocairo::functions::show_layout(&cr, &layout);
    cr.restore()?;
    // Drop the current point left by the text, or the ring arc would start with a line from it.
    cr.new_path();

    // Ring filled with the picked color.
    // Overlap the lens edge by a pixel to hide the antialiasing seam between them.
    cr.set_line_width(ring + 1.);
    cr.arc(c, c, radius + (ring - 1.) / 2., 0., TAU);
    cr.set_source_rgb(rf, gf, bf);
    cr.stroke()?;
    drop(cr);

    let data = surface.take_data().unwrap();
    let buffer = TextureBuffer::from_memory(
        renderer,
        &data,
        Fourcc::Argb8888,
        (size, size),
        false,
        scale,
        Transform::Normal,
        Vec::new(),
    )?;

    // The lens is centered on the picked pixel.
    let center = c.round() as i32;
    Ok((buffer, Point::from((center, center))))
}

fn rounded_rect(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    let r = r.min(w / 2.).min(h / 2.);
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -FRAC_PI_2, 0.);
    cr.arc(x + w - r, y + h - r, r, 0., FRAC_PI_2);
    cr.arc(x + r, y + h - r, r, FRAC_PI_2, PI);
    cr.arc(x + r, y + r, r, PI, PI + FRAC_PI_2);
    cr.close_path();
}
