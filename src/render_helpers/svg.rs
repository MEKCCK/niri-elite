use resvg::tiny_skia::{Pixmap, Transform};
use usvg::{Options, Tree};

/// Renders an SVG cursor at the requested scale.
///
/// Returns `(width, height, pixels_rgba, xhot, yhot)`.
pub fn render_cursor(
    svg_data: &str,
    scale: f64,
    hotspot: Option<(f64, f64)>,
) -> Option<(u32, u32, Vec<u8>, u32, u32)> {
    if !scale.is_finite() || scale <= 0. {
        return None;
    }

    let opts = Options::default();
    let tree = Tree::from_str(svg_data, &opts).ok()?;

    let svg_size = tree.size();
    let width = (svg_size.width() as f64 * scale).ceil() as u32;
    let height = (svg_size.height() as f64 * scale).ceil() as u32;

    let mut pixmap = Pixmap::new(width, height)?;

    let transform = Transform::from_scale(scale as f32, scale as f32);
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    let mut pixels_rgba = pixmap.data().to_vec();
    // tiny-skia gives premultiplied RGBA, while XCursor/ARGB8888 memory is BGRA
    // on little-endian systems. Match the byte order used by parsed XCursor files.
    for pixel in pixels_rgba.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }

    let (xhot, yhot) = hotspot.unwrap_or_else(|| parse_hotspot(svg_data));
    let xhot = (xhot * scale).round().clamp(0., width as f64) as u32;
    let yhot = (yhot * scale).round().clamp(0., height as f64) as u32;

    debug!("rendered SVG cursor: {width}x{height} (scale {scale}), hotspot ({xhot}, {yhot})");

    Some((width, height, pixels_rgba, xhot, yhot))
}

pub fn render_cursor_to_size(
    svg_data: &str,
    target_size: u32,
) -> Option<(u32, u32, Vec<u8>, u32, u32)> {
    let opts = Options::default();
    let tree = Tree::from_str(svg_data, &opts).ok()?;

    let svg_size = tree.size();
    let scale_w = target_size as f64 / svg_size.width() as f64;
    let scale_h = target_size as f64 / svg_size.height() as f64;
    render_cursor(svg_data, scale_w.min(scale_h), None)
}

fn parse_hotspot(svg_data: &str) -> (f64, f64) {
    for line in svg_data.lines() {
        if let Some(pos) = line.find("Hotspot:") {
            let rest = &line[pos + 8..].trim();
            if let Some((x_str, y_str)) = rest.split_once(',') {
                let x: f64 = x_str.trim().parse().ok().unwrap_or(0.);
                let y: f64 = y_str.trim().parse().ok().unwrap_or(0.);
                return (x, y);
            }
        }
    }
    (1., 1.)
}
