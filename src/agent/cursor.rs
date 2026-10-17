//! The agent's cursor, drawn on the window it acts on.
//!
//! It's the system arrow, a bit transparent, with a feathered halo in the focus ring colors, and a
//! soft ripple on clicks. It's part of the window's own rendering, so it follows the window
//! wherever niri draws it (scrolling, overview, grid, screencasts) without knowing about the
//! layout. It fades out when the agent stops acting on the window, and agent captures leave it
//! out.

use std::cell::Cell;
use std::time::{Duration, Instant};

use niri_config::FocusRing;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::{
    MemoryRenderBuffer, MemoryRenderBufferRenderElement,
};
use smithay::backend::renderer::element::Kind;
use smithay::utils::{Logical, Point, Scale, Size, Transform};

use crate::render_helpers::renderer::NiriRenderer;

/// Opacity of the arrow.
const ALPHA: f32 = 0.7;

/// Diameter of the halo, in logical pixels.
const HALO_SIZE: f64 = 44.;

/// Opacity at the center of the halo.
const HALO_ALPHA: f32 = 0.55;

/// The cursor stays this long after the last action, then fades out.
const IDLE: Duration = Duration::from_secs(2);
const FADE: Duration = Duration::from_millis(500);

/// Click ripple: the halo grows from `RIPPLE_FROM` to `RIPPLE_TO` while fading out.
const RIPPLE: Duration = Duration::from_millis(500);
const RIPPLE_FROM: f64 = 16.;
const RIPPLE_TO: f64 = 72.;
const RIPPLE_ALPHA: f32 = 0.75;

thread_local! {
    static HIDDEN: Cell<bool> = const { Cell::new(false) };
}

/// Runs `f` with the agent cursor left out of window rendering, for agent captures.
pub fn without_cursor<T>(f: impl FnOnce() -> T) -> T {
    HIDDEN.with(|hidden| hidden.set(true));
    let result = f();
    HIDDEN.with(|hidden| hidden.set(false));
    result
}

#[derive(Debug)]
pub struct AgentCursor {
    /// Tip of the arrow, in window coordinates.
    pos: Point<f64, Logical>,
    arrow: MemoryRenderBuffer,
    hotspot: Point<f64, Logical>,
    arrow_size: Size<f64, Logical>,
    halo: MemoryRenderBuffer,
    active_at: Instant,
    click: Option<(Point<f64, Logical>, Instant)>,
}

impl AgentCursor {
    pub fn new(
        arrow: MemoryRenderBuffer,
        hotspot: Point<f64, Logical>,
        arrow_size: Size<f64, Logical>,
        halo: MemoryRenderBuffer,
    ) -> Self {
        Self {
            pos: Point::default(),
            arrow,
            hotspot,
            arrow_size,
            halo,
            active_at: Instant::now(),
            click: None,
        }
    }

    pub fn move_to(&mut self, pos: Point<f64, Logical>) {
        self.pos = pos;
        self.active_at = Instant::now();
    }

    pub fn click(&mut self) {
        self.active_at = Instant::now();
        self.click = Some((self.pos, self.active_at));
    }

    /// Whether the cursor is still shown.
    pub fn is_visible(&self) -> bool {
        self.active_at.elapsed() < IDLE + FADE
    }

    /// Whether the cursor changes over time right now and needs redraws: during a click
    /// ripple and while fading out, but not while it sits still.
    pub fn is_animating(&self) -> bool {
        let rippling = self.click.is_some_and(|(_, at)| at.elapsed() < RIPPLE);
        let idle = self.active_at.elapsed();
        rippling || (idle >= IDLE && idle < IDLE + FADE)
    }

    pub fn render<R: NiriRenderer>(
        &self,
        renderer: &mut R,
        location: Point<f64, Logical>,
        scale: Scale<f64>,
        alpha: f32,
        push: &mut dyn FnMut(MemoryRenderBufferRenderElement<R>),
    ) {
        if HIDDEN.with(Cell::get) {
            return;
        }

        let idle = self.active_at.elapsed();
        let fade = if idle < IDLE {
            1.
        } else {
            1. - ((idle - IDLE).as_secs_f32() / FADE.as_secs_f32()).min(1.)
        };
        let alpha = alpha * fade;
        if alpha <= 0. {
            return;
        }

        let mut push_buffer = |buffer: &MemoryRenderBuffer,
                               loc: Point<f64, Logical>,
                               size: Option<Size<f64, Logical>>,
                               alpha: f32| {
            let loc = (location + loc).to_physical_precise_round(scale);
            let size = size.map(|size| size.to_i32_round());
            match MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                loc,
                buffer,
                Some(alpha),
                None,
                size,
                Kind::Unspecified,
            ) {
                Ok(elem) => push(elem),
                Err(err) => warn!("error importing the agent cursor: {err:?}"),
            }
        };

        // Front to back: arrow, halo, ripple.
        let arrow_loc = self.pos - self.hotspot;
        push_buffer(&self.arrow, arrow_loc, None, alpha * ALPHA);

        // Center the halo on the arrow's body rather than on its tip.
        let body = arrow_loc + Point::from((self.arrow_size.w * 0.3, self.arrow_size.h * 0.4));
        let halo = Point::from((HALO_SIZE / 2., HALO_SIZE / 2.));
        push_buffer(&self.halo, body - halo, None, alpha);

        if let Some((at, clicked_at)) = self.click {
            let t = clicked_at.elapsed().as_secs_f64() / RIPPLE.as_secs_f64();
            if t < 1. {
                let size = RIPPLE_FROM + (RIPPLE_TO - RIPPLE_FROM) * t;
                let loc = at - Point::from((size / 2., size / 2.));
                let ripple_alpha = alpha * RIPPLE_ALPHA * (1. - t as f32) / HALO_ALPHA;
                push_buffer(
                    &self.halo,
                    loc,
                    Some(Size::from((size, size))),
                    ripple_alpha.min(1.),
                );
            }
        }
    }
}

/// Renders the halo: a feathered disc, `HALO_SIZE` logical pixels wide, colored like the active
/// focus ring: its gradient if it has one, laid across the disc at the same angle.
pub fn halo_buffer(focus_ring: &FocusRing, scale: i32) -> MemoryRenderBuffer {
    let (from, to, angle) = match focus_ring.active_gradient {
        Some(gradient) => (gradient.from, gradient.to, f64::from(gradient.angle)),
        None => (focus_ring.active_color, focus_ring.active_color, 0.),
    };
    // CSS angles: 0 points up, 90 points right.
    let (dir_x, dir_y) = (angle.to_radians().sin(), -angle.to_radians().cos());

    let size = (HALO_SIZE * f64::from(scale)).round() as i32;
    let radius = f64::from(size) / 2.;

    let mut pixels = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let dx = f64::from(x) + 0.5 - radius;
            let dy = f64::from(y) + 0.5 - radius;
            let t = ((dx * dx + dy * dy).sqrt() / radius).min(1.);

            // Position along the gradient, 0 at its start and 1 at its end.
            let g = (((dx * dir_x + dy * dir_y) / radius + 1.) / 2.).clamp(0., 1.) as f32;
            let mix = |a: f32, b: f32| a + (b - a) * g;
            let (r, gr, b) = (mix(from.r, to.r), mix(from.g, to.g), mix(from.b, to.b));
            let color_a = mix(from.a, to.a);

            // Gaussian-like falloff that reaches exactly zero at the edge.
            let a = f64::from(HALO_ALPHA * color_a) * (-2. * t * t).exp() * (1. - t * t);

            // Premultiplied ARGB8888, which is B, G, R, A in memory.
            let channel = |c: f32| (f64::from(c) * a * 255.).round() as u8;
            pixels.extend_from_slice(&[
                channel(b),
                channel(gr),
                channel(r),
                (a * 255.).round() as u8,
            ]);
        }
    }

    MemoryRenderBuffer::from_slice(
        &pixels,
        Fourcc::Argb8888,
        (size, size),
        scale,
        Transform::Normal,
        None,
    )
}
