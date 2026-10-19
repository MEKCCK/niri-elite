use std::cell::Cell;
use std::f64::consts::{FRAC_PI_2, PI, TAU};

use niri_ipc::PickedColor;
use pango::FontDescription;
use pangocairo::cairo::{self, ImageSurface};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::output::Output;
use smithay::utils::{Logical, Physical, Point, Size, Transform};

use crate::render_helpers::primary_gpu_texture::PrimaryGpuTextureRenderElement;
use crate::render_helpers::texture::{TextureBuffer, TextureRenderElement};
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

/// Color picker with a magnifier that follows the pointer over the live screen.
///
/// The magnifier shows a small region around the pointer, captured from the output without the
/// magnifier itself. The region is captured again on pointer motion and after every frame, so the
/// magnifier follows changing contents; it is redrawn only when the captured pixels change.
#[allow(clippy::large_enum_variant)]
pub enum PickColorUi {
    Closed,
    Open {
        region: Option<Region>,
        magnifier: Option<Magnifier>,
        zoom: f64,
        /// Whether an output was rendered since the region was last captured.
        rendered: Cell<bool>,
    },
}

/// Screen pixels around the pointer.
pub struct Region {
    pub output: Output,
    pub scale: f64,
    /// Output-local pointer position.
    pub pos: Point<f64, Logical>,
    pub output_size: Size<i32, Physical>,
    /// Top-left corner of the region within the output.
    pub origin: Point<i32, Physical>,
    /// Width and height of the region.
    pub side: i32,
    /// RGBA8 pixels.
    pub pixels: Vec<u8>,
}

pub struct Magnifier {
    output: Output,
    location: Point<f64, Logical>,
    zoom: f64,
    buffer: TextureBuffer<GlesTexture>,
}

impl PickColorUi {
    pub fn new() -> Self {
        Self::Closed
    }

    pub fn is_open(&self) -> bool {
        matches!(self, Self::Open { .. })
    }

    pub fn open(&mut self) {
        *self = Self::Open {
            region: None,
            magnifier: None,
            zoom: DEFAULT_ZOOM,
            rendered: Cell::new(false),
        };
    }

    pub fn close(&mut self) -> bool {
        if !self.is_open() {
            return false;
        }
        *self = Self::Closed;
        true
    }

    /// Reads the color under the pointer from the last captured region.
    pub fn color_at(&self, output: &Output, pos: Point<f64, Logical>) -> Option<PickedColor> {
        let Self::Open {
            region: Some(region),
            ..
        } = self
        else {
            return None;
        };
        if region.output != *output {
            return None;
        }
        let pos = region.pixel_pos(pos);
        let [r, g, b] = region.pixel_checked(pos.x, pos.y)?;
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

    /// Number of screen pixels the magnifier needs on each side of the center pixel.
    pub fn capture_half(&self, scale: f64) -> i32 {
        let zoom = match self {
            Self::Open { zoom, .. } => *zoom,
            Self::Closed => DEFAULT_ZOOM,
        };
        magnifier_half(scale, zoom)
    }

    /// Returns whether an output was rendered since the last call.
    pub fn take_rendered(&self) -> bool {
        match self {
            Self::Open { rendered, .. } => rendered.replace(false),
            Self::Closed => false,
        }
    }

    /// Hides the magnifier, returning whether it was visible.
    pub fn clear(&mut self) -> bool {
        let Self::Open {
            region, magnifier, ..
        } = self
        else {
            return false;
        };
        *region = None;
        magnifier.take().is_some()
    }

    /// Updates the magnifier from a newly captured region.
    ///
    /// Returns whether the magnifier changed and needs a redraw.
    pub fn set_region(&mut self, renderer: &mut GlesRenderer, new: Region) -> bool {
        let Self::Open {
            region,
            magnifier,
            zoom,
            ..
        } = self
        else {
            return false;
        };

        let unchanged = region.as_ref().is_some_and(|old| {
            old.output == new.output
                && old.pos == new.pos
                && old.origin == new.origin
                && old.side == new.side
                && old.pixels == new.pixels
        }) && magnifier.as_ref().is_some_and(|m| m.zoom == *zoom);
        if unchanged {
            return false;
        }

        match render_magnifier(renderer, &new, *zoom) {
            Ok((buffer, center)) => {
                let location = new.pos - center.to_f64().to_logical(new.scale);
                *magnifier = Some(Magnifier {
                    output: new.output.clone(),
                    location,
                    zoom: *zoom,
                    buffer,
                });
            }
            Err(err) => {
                warn!("error rendering color picker magnifier: {err:?}");
                *magnifier = None;
            }
        }
        *region = Some(new);
        true
    }

    pub fn render_output(
        &self,
        output: &Output,
        push: &mut dyn FnMut(PrimaryGpuTextureRenderElement),
    ) {
        let Self::Open {
            magnifier,
            rendered,
            ..
        } = self
        else {
            return;
        };

        rendered.set(true);

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
    }
}

impl Default for PickColorUi {
    fn default() -> Self {
        Self::new()
    }
}

impl Region {
    fn pixel_pos(&self, pos: Point<f64, Logical>) -> Point<i32, Physical> {
        clamp_to_output(pos.to_physical_precise_floor(self.scale), self.output_size)
    }

    /// Returns the pixel color, or `None` if outside the screen or the captured region.
    fn pixel_checked(&self, x: i32, y: i32) -> Option<[u8; 3]> {
        if x < 0 || y < 0 || x >= self.output_size.w || y >= self.output_size.h {
            return None;
        }
        let (x, y) = (x - self.origin.x, y - self.origin.y);
        if x < 0 || y < 0 || x >= self.side || y >= self.side {
            return None;
        }
        let idx = (y as usize * self.side as usize + x as usize) * 4;
        let px = self.pixels.get(idx..idx + 3)?;
        Some([px[0], px[1], px[2]])
    }
}

pub fn clamp_to_output(
    pos: Point<i32, Physical>,
    output_size: Size<i32, Physical>,
) -> Point<i32, Physical> {
    Point::from((
        pos.x.clamp(0, output_size.w - 1),
        pos.y.clamp(0, output_size.h - 1),
    ))
}

fn magnifier_half(scale: f64, zoom: f64) -> i32 {
    let diameter: i32 = to_physical_precise_round(scale, MAGNIFIER_DIAMETER);
    (f64::from(diameter) / 2. / zoom).ceil() as i32 + 1
}

/// Renders the magnifier: magnified pixels inside a ring filled with the picked color, and the
/// hex value in the lower part of the lens.
///
/// Returns the buffer and the physical position of the magnifier center within it.
fn render_magnifier(
    renderer: &mut GlesRenderer,
    data: &Region,
    zoom: f64,
) -> anyhow::Result<(TextureBuffer<GlesTexture>, Point<i32, Physical>)> {
    let _span = tracy_client::span!("pick_color_ui::render_magnifier");

    let scale = data.scale;
    let center = data.pixel_pos(data.pos);
    let [r, g, b] = data.pixel_checked(center.x, center.y).unwrap_or_default();
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
    let half = magnifier_half(scale, zoom);
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
