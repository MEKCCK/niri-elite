//! Where the windows of a grid with tiled windows go: the tiled windows in a block in the
//! middle, and the floating windows in bands around it, on the side of the screen they are on.
//!
//! The more the windows shrink, the less small windows shrink than big ones, the way window
//! overviews of other desktops keep small windows readable: with room to spare, every window
//! shows at the same scale, and only as it gets tight do sizes squeeze towards each other. How a
//! window shrinks depends on its own size and the scale only, so windows coming and going don't
//! change it but by the scale. The tiled windows shrink alike, as they are alike in height.

use std::ops::Range;

use smithay::utils::{Logical, Point, Rectangle, Size};

use super::{rect_center, GridItem, GridSource, EPSILON};
use crate::layout::LayoutElement;

/// How much less small windows shrink: with everything at `scale`, a window with a fraction `a`
/// of the screen's area shows at `scale ^ (a ^ SQUEEZE)`.
const SQUEEZE: f64 = 0.25;

/// Where in thirds of the screen a floating window has to be, across or up and down, to count as
/// on that side.
const SIDE: f64 = 1. / 3.;

/// How floating windows go around the block of tiled windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Arrangement {
    /// Above and below the block, windows in the corners too.
    Above,
    /// Beside the block for windows in the corners, above and below it for the others.
    Corners,
    /// Beside the block for every window, on the side of the screen it is on: windows from above
    /// go at the top of the columns and windows from below at the bottom, and the block can
    /// wrap into more rows.
    Beside,
}

/// How a window of `size` on a screen of `area` shrinks: see [`SQUEEZE`].
pub(super) fn squeeze_exponent(area: Rectangle<f64, Logical>, size: Size<f64, Logical>) -> f64 {
    let screen = (area.size.w * area.size.h).max(1.);
    let own = (size.w * size.h).max(1.);
    (own / screen).clamp(0.000001, 1.).powf(SQUEEZE)
}

/// The scale a window with `exponent` shows at with everything at `scale`.
pub(super) fn squeezed_scale(scale: f64, exponent: f64) -> f64 {
    scale.min(1.).powf(exponent)
}

pub(super) const ARRANGEMENTS: [Arrangement; 3] = [
    Arrangement::Above,
    Arrangement::Corners,
    Arrangement::Beside,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Side {
    Top,
    Bottom,
    Left,
    Right,
}

/// The band a floating window is in, and its line in it, counting out from the block.
#[derive(Debug)]
pub(super) struct BandLine<W: LayoutElement> {
    pub id: W::Id,
    pub side: Side,
    pub line: usize,
}

impl<W: LayoutElement> Clone for BandLine<W> {
    fn clone(&self) -> Self {
        Self {
            id: self.id.clone(),
            side: self.side,
            line: self.line,
        }
    }
}

/// Where every item goes, as the place it has, with the row of every tiled one, and the lines of
/// the floating windows in the bands.
pub(super) struct Placed<W: LayoutElement> {
    pub slots: Vec<(Rectangle<f64, Logical>, Option<usize>)>,
    pub lines: Vec<BandLine<W>>,
}

struct Floating {
    /// Index of the item.
    idx: usize,
    /// Where it is on the workspace.
    center: Point<f64, Logical>,
    /// The side of the screen it's on up and down, and across.
    up: Option<Side>,
    across: Option<Side>,
}

struct Row {
    width: f64,
    height: f64,
    top: f64,
}

struct Block {
    rows: Vec<Row>,
    /// Item index, row and place of every tiled window, from the block's top left.
    cells: Vec<(usize, usize, Rectangle<f64, Logical>)>,
    size: Size<f64, Logical>,
}

/// A window in a line of a band, by its index in `Bands::floating`, with its length along the
/// line, its thickness across it, and where along it it is on the workspace.
#[derive(Clone, Copy)]
struct LineItem {
    float: usize,
    len: f64,
    thick: f64,
    at: f64,
}

struct Line {
    items: Vec<LineItem>,
    len: f64,
    thick: f64,
}

/// A pocket: the room beside a short row, inside the block.
struct Pocket {
    end: Side,
    /// The side of the block the row is on, if it's the first or the last; `None` for both.
    edge: Option<Option<Side>>,
    x: Range<f64>,
    y: Range<f64>,
    used: f64,
}

pub(super) struct Bands<'a, W: LayoutElement> {
    items: &'a [(GridItem<W>, GridSource)],
    content: Rectangle<f64, Logical>,
    gap: f64,
    groups: &'a [Range<usize>],
    /// How every item shrinks with the scale: see [`SQUEEZE`].
    exponents: Vec<f64>,
    floating: Vec<Floating>,
    /// The middle of the screen.
    middle: Point<f64, Logical>,
}

impl<'a, W: LayoutElement> Bands<'a, W> {
    pub fn new(
        items: &'a [(GridItem<W>, GridSource)],
        area: Rectangle<f64, Logical>,
        content: Rectangle<f64, Logical>,
        gap: f64,
        groups: &'a [Range<usize>],
        floating: &[usize],
    ) -> Self {
        // The tiled windows shrink alike, by their average size, so that they stay alike in
        // height.
        let tiled: Vec<usize> = groups.iter().flat_map(|g| g.clone()).collect();
        let tiled_size = if tiled.is_empty() {
            Size::from((1., 1.))
        } else {
            let area_sum: f64 = tiled
                .iter()
                .map(|&idx| items[idx].1.size.w * items[idx].1.size.h)
                .sum();
            let side = (area_sum / tiled.len() as f64).sqrt();
            Size::from((side, side))
        };
        let exponents = items
            .iter()
            .map(|(item, source)| {
                if item.col_idx().is_some() {
                    squeeze_exponent(area, tiled_size)
                } else {
                    squeeze_exponent(area, source.size)
                }
            })
            .collect();

        // Which third of the screen a window is in.
        let middle = rect_center(area);
        let floating = floating
            .iter()
            .map(|&idx| {
                let source = items[idx].1;
                let center = source.pos.map_or(middle, |pos| {
                    pos + Point::from((source.size.w / 2., source.size.h / 2.))
                });
                let nx = (center.x - middle.x) / (area.size.w / 2.);
                let ny = (center.y - middle.y) / (area.size.h / 2.);
                let across = if nx < -SIDE {
                    Some(Side::Left)
                } else if nx > SIDE {
                    Some(Side::Right)
                } else {
                    None
                };
                let up = if ny < -SIDE {
                    Some(Side::Top)
                } else if ny > SIDE {
                    Some(Side::Bottom)
                } else {
                    None
                };
                Floating {
                    idx,
                    center,
                    up,
                    across,
                }
            })
            .collect();

        Self {
            items,
            content,
            gap,
            groups,
            exponents,
            floating,
            middle,
        }
    }

    /// The scale item `idx` shows at with everything at `scale`.
    pub fn shown_scale(&self, idx: usize, scale: f64) -> f64 {
        squeezed_scale(scale, self.exponents[idx])
    }

    /// The size item `idx` shows at with everything at `scale`.
    fn size(&self, idx: usize, scale: f64) -> Size<f64, Logical> {
        self.items[idx].1.size.upscale(self.shown_scale(idx, scale))
    }

    /// The side of the block a floating window goes to.
    fn side_of(&self, f: &Floating, arrangement: Arrangement) -> Side {
        match arrangement {
            Arrangement::Beside => f.across.unwrap_or(if f.center.x < self.middle.x {
                Side::Left
            } else {
                Side::Right
            }),
            Arrangement::Above => f.up.or(f.across).unwrap_or(Side::Bottom),
            Arrangement::Corners => match (f.up, f.across) {
                (_, Some(across)) => across,
                (Some(up), None) => up,
                (None, None) => Side::Bottom,
            },
        }
    }

    /// The tiled windows in rows wrapping at `wrap`, every row centered, at `scale`.
    fn block(&self, wrap: f64, scale: f64) -> Block {
        let gap = self.gap;
        let size = |idx: usize| self.size(idx, scale);
        let wrap_width = self.content.size.w * wrap;

        // (groups, width, height) of every row. A column's tabs stay together.
        let mut rows: Vec<(Range<usize>, f64, f64)> = Vec::new();
        for (group_idx, group) in self.groups.iter().enumerate() {
            let width =
                group.clone().map(|idx| size(idx).w).sum::<f64>() + gap * (group.len() - 1) as f64;
            let height = group.clone().map(|idx| size(idx).h).fold(0., f64::max);
            match rows.last_mut() {
                Some((groups, row_width, row_height))
                    if *row_width + gap + width <= wrap_width + EPSILON =>
                {
                    groups.end = group_idx + 1;
                    *row_width += gap + width;
                    *row_height = row_height.max(height);
                }
                _ => rows.push((group_idx..group_idx + 1, width, height)),
            }
        }
        let block_width = rows.iter().map(|(_, width, _)| *width).fold(0., f64::max);
        let block_height = rows.iter().map(|(_, _, height)| *height).sum::<f64>()
            + gap * rows.len().saturating_sub(1) as f64;

        let mut cells = Vec::new();
        let mut out = Vec::new();
        let mut y = 0.;
        for (row_idx, (groups, width, height)) in rows.into_iter().enumerate() {
            let mut x = (block_width - width) / 2.;
            for idx in groups.flat_map(|group_idx| self.groups[group_idx].clone()) {
                let size = size(idx);
                let pos = Point::from((x, y + (height - size.h) / 2.));
                cells.push((idx, row_idx, Rectangle::new(pos, size)));
                x += size.w + gap;
            }
            out.push(Row {
                width,
                height,
                top: y,
            });
            y += height + gap;
        }

        Block {
            rows: out,
            cells,
            size: Size::from((block_width, block_height)),
        }
    }

    /// Where everything goes with the rows wrapping at `wrap`, the floating windows arranged
    /// with `arrangement`, and everything at `scale`. Floating windows stay in the lines `kept`
    /// says they were in, as long as those have room for them.
    ///
    /// `None` if it doesn't fit. With `force` it's placed anyway, overlapping where it must.
    pub fn place(
        &self,
        wrap: f64,
        scale: f64,
        arrangement: Arrangement,
        kept: Option<&[BandLine<W>]>,
        force: bool,
    ) -> Option<Placed<W>> {
        let content = self.content;
        let gap = self.gap;
        let middle = rect_center(content);
        let block = self.block(wrap, scale);
        let bw = block.size.w;
        let bh = block.size.h;
        if !force && (bw > content.size.w + EPSILON || bh > content.size.h + EPSILON) {
            return None;
        }

        let size = |f: &Floating| self.size(f.idx, scale);
        if !force
            && self.floating.iter().any(|f| {
                let size = size(f);
                size.w > content.size.w + EPSILON || size.h > content.size.h + EPSILON
            })
        {
            return None;
        }

        // Pockets beside short rows. A floating window goes into one on its side across, and for
        // one in the top or bottom third of the screen, beside the first or the last row, in
        // the order the windows come in, so that a new one never pushes an older one out.
        let rows = block.rows.len();
        let mut pockets = Vec::new();
        for (row_idx, row) in block.rows.iter().enumerate() {
            let free = (bw - row.width) / 2. - gap;
            if free <= 0. {
                continue;
            }
            let edge = match (row_idx == 0, row_idx + 1 == rows) {
                (true, true) => None,
                (true, false) => Some(Some(Side::Top)),
                (false, true) => Some(Some(Side::Bottom)),
                (false, false) => Some(None),
            };
            let y = row.top..row.top + row.height;
            pockets.push(Pocket {
                end: Side::Left,
                edge,
                x: 0.0..free,
                y: y.clone(),
                used: 0.,
            });
            pockets.push(Pocket {
                end: Side::Right,
                edge,
                x: bw - free..bw,
                y,
                used: 0.,
            });
        }

        let block_top_guess = middle.y - bh / 2.;
        let mut in_pockets = Vec::new();
        let mut sides: [Vec<usize>; 4] = Default::default();
        let side_idx = |side: Side| match side {
            Side::Top => 0,
            Side::Bottom => 1,
            Side::Left => 2,
            Side::Right => 3,
        };
        // With every window beside the block, one in the middle across goes to the side with
        // less on it, or the side it was on before.
        let mut beside_area = [0., 0.];
        for (float, f) in self.floating.iter().enumerate() {
            let size = size(f);
            let side = if arrangement == Arrangement::Beside && f.across.is_none() {
                let id = self.items[f.idx].0.window_id();
                let kept_side = kept.and_then(|kept| {
                    kept.iter()
                        .find(|l| &l.id == id && matches!(l.side, Side::Left | Side::Right))
                        .map(|l| l.side)
                });
                kept_side.unwrap_or(if beside_area[0] < beside_area[1] - EPSILON {
                    Side::Left
                } else if beside_area[1] < beside_area[0] - EPSILON {
                    Side::Right
                } else {
                    self.side_of(f, arrangement)
                })
            } else {
                self.side_of(f, arrangement)
            };
            match side {
                Side::Left => beside_area[0] += size.w * size.h,
                Side::Right => beside_area[1] += size.w * size.h,
                _ => (),
            }
            let mut best: Option<(usize, (f64, f64, bool))> = None;
            for (pocket_idx, p) in pockets.iter().enumerate() {
                let row_ok = match (f.up, p.edge) {
                    (None, _) | (_, None) => true,
                    (Some(up), Some(edge)) => edge == Some(up),
                };
                let ok = row_ok && (side == p.end || f.across == Some(p.end));
                let room = p.x.end - p.x.start - p.used;
                if !ok || room < size.w - EPSILON || p.y.end - p.y.start < size.h - EPSILON {
                    continue;
                }
                // Up and down, the closest row; across, the most room, then the right one.
                let key = (
                    (block_top_guess + (p.y.start + p.y.end) / 2. - f.center.y).abs(),
                    -room,
                    p.end != Side::Right,
                );
                let better = best.is_none_or(|(_, b)| {
                    key.0 < b.0 - EPSILON
                        || ((key.0 - b.0).abs() <= EPSILON
                            && (key.1 < b.1 - EPSILON
                                || ((key.1 - b.1).abs() <= EPSILON && !key.2 && b.2)))
                });
                if better {
                    best = Some((pocket_idx, key));
                }
            }
            if let Some((pocket_idx, _)) = best {
                let p = &mut pockets[pocket_idx];
                let x = if p.end == Side::Left {
                    p.x.end - p.used - size.w
                } else {
                    p.x.start + p.used
                };
                let y = match side {
                    Side::Top => p.y.start,
                    Side::Bottom => p.y.end - size.h,
                    _ => (p.y.start + p.y.end - size.h) / 2.,
                };
                in_pockets.push((f.idx, Rectangle::new(Point::from((x, y)), size)));
                p.used += size.w + gap;
            } else {
                sides[side_idx(side)].push(float);
            }
        }

        // The lines of every band.
        let kept_line = |float: usize, side: Side| {
            let id = self.items[self.floating[float].idx].0.window_id();
            kept?
                .iter()
                .find(|l| l.side == side && &l.id == id)
                .map(|l| l.line)
        };
        let band = |side: Side, limit: f64| {
            let across = matches!(side, Side::Top | Side::Bottom);
            let items = sides[side_idx(side)]
                .iter()
                .map(|&float| {
                    let f = &self.floating[float];
                    let size = size(f);
                    if across {
                        LineItem {
                            float,
                            len: size.w,
                            thick: size.h,
                            at: f.center.x,
                        }
                    } else {
                        LineItem {
                            float,
                            len: size.h,
                            thick: size.w,
                            at: f.center.y,
                        }
                    }
                })
                .collect();
            lines(items, limit, gap, |float| kept_line(float, side))
        };
        let thickness = |lines: &[Line]| {
            lines.iter().map(|l| l.thick).sum::<f64>() + gap * lines.len().saturating_sub(1) as f64
        };
        let with_gap = |len: f64| if len > 0. { len + gap } else { 0. };

        // With `Above`, the bands above and below run the whole width and the columns beside the
        // block fit between them; otherwise the columns run the whole height and the bands fit
        // between them.
        let (top, bottom, left, right, band_x, col_y);
        if arrangement == Arrangement::Above {
            band_x = content.loc.x..content.loc.x + content.size.w;
            top = band(Side::Top, content.size.w);
            bottom = band(Side::Bottom, content.size.w);
            col_y = content.loc.y + with_gap(thickness(&top))
                ..content.loc.y + content.size.h - with_gap(thickness(&bottom));
            if !force && col_y.end - col_y.start < bh - EPSILON {
                return None;
            }
            left = band(Side::Left, col_y.end - col_y.start);
            right = band(Side::Right, col_y.end - col_y.start);
        } else {
            col_y = content.loc.y..content.loc.y + content.size.h;
            left = band(Side::Left, content.size.h);
            right = band(Side::Right, content.size.h);
            band_x = content.loc.x + with_gap(thickness(&left))
                ..content.loc.x + content.size.w - with_gap(thickness(&right));
            if !force && band_x.end - band_x.start < bw - EPSILON {
                return None;
            }
            top = band(Side::Top, band_x.end - band_x.start);
            bottom = band(Side::Bottom, band_x.end - band_x.start);
        }
        let (ht, hb) = (thickness(&top), thickness(&bottom));
        let (wl, wr) = (thickness(&left), thickness(&right));
        if !force {
            let band_len = band_x.end - band_x.start;
            let col_len = col_y.end - col_y.start;
            if top
                .iter()
                .chain(&bottom)
                .any(|l| l.len > band_len + EPSILON)
                || left.iter().chain(&right).any(|l| l.len > col_len + EPSILON)
                || with_gap(wl) + bw + with_gap(wr) > content.size.w + EPSILON
                || with_gap(ht) + bh + with_gap(hb) > content.size.h + EPSILON
            {
                return None;
            }
        }

        // The block in the middle, or as close to it as the bands and columns let it.
        let mid_top = content.loc.y + with_gap(ht);
        let mid_bottom = content.loc.y + content.size.h - with_gap(hb);
        let lo = content.loc.x + with_gap(wl) + bw / 2.;
        let hi = content.loc.x + content.size.w - with_gap(wr) - bw / 2.;
        let block_x = middle.x.clamp(lo, hi.max(lo));
        let lo = mid_top + bh / 2.;
        let hi = mid_bottom - bh / 2.;
        let block_y = middle.y.clamp(lo, hi.max(lo));
        let block_loc = Point::from((block_x - bw / 2., block_y - bh / 2.));

        let mut slots =
            vec![(Rectangle::new(content.loc, Size::default()), None); self.items.len()];
        for (idx, row, r) in block.cells {
            slots[idx] = (Rectangle::new(block_loc + r.loc, r.size), Some(row));
        }
        for (idx, r) in in_pockets {
            slots[idx] = (Rectangle::new(block_loc + r.loc, r.size), None);
        }

        let mut placed_lines = Vec::new();
        let mut record = |side: Side, lines: &[Line]| {
            for (line, l) in lines.iter().enumerate() {
                for item in &l.items {
                    placed_lines.push(BandLine {
                        id: self.items[self.floating[item.float].idx]
                            .0
                            .window_id()
                            .clone(),
                        side,
                        line,
                    });
                }
            }
        };
        record(Side::Top, &top);
        record(Side::Bottom, &bottom);
        record(Side::Left, &left);
        record(Side::Right, &right);

        let mut put = |float: usize, loc: Point<f64, Logical>| {
            let f = &self.floating[float];
            slots[f.idx] = (Rectangle::new(loc, size(f)), None);
        };
        // Above: lines from the top edge down, the first line next to the block, windows sitting
        // on the line's bottom. Below: lines from the block down, windows hanging from the line's
        // top. Beside: columns out from the block, windows lined up against it; columns that run
        // the whole height go along the edges of the grid, out of the way of the bands between
        // them.
        let mut y = content.loc.y;
        for l in top.iter().rev() {
            for (item, start) in spread(&l.items, band_x.clone(), gap) {
                put(item.float, Point::from((start, y + l.thick - item.thick)));
            }
            y += l.thick + gap;
        }
        let mut y = content.loc.y + content.size.h - hb;
        for l in &bottom {
            for (item, start) in spread(&l.items, band_x.clone(), gap) {
                put(item.float, Point::from((start, y)));
            }
            y += l.thick + gap;
        }
        let mut x = if arrangement == Arrangement::Above {
            block_loc.x - gap
        } else {
            content.loc.x + wl
        };
        for l in &left {
            for (item, start) in spread(&l.items, col_y.clone(), gap) {
                put(item.float, Point::from((x - item.thick, start)));
            }
            x -= l.thick + gap;
        }
        let mut x = if arrangement == Arrangement::Above {
            block_loc.x + bw + gap
        } else {
            content.loc.x + content.size.w - wr
        };
        for l in &right {
            for (item, start) in spread(&l.items, col_y.clone(), gap) {
                put(item.float, Point::from((x, start)));
            }
            x += l.thick + gap;
        }

        Some(Placed {
            slots,
            lines: placed_lines,
        })
    }
}

/// Lines of `items`, each no longer than `limit`. A window stays in the line `kept` says it was
/// in while that line has room for it; the others, in the order they come in, take the first
/// line out from the block with room. So a new window never moves an older one to another line,
/// and neither does the grid growing or shrinking. In a line, windows go by where they are on
/// the workspace.
fn lines(
    items: Vec<LineItem>,
    limit: f64,
    gap: f64,
    kept: impl Fn(usize) -> Option<usize>,
) -> Vec<Line> {
    let mut out: Vec<Line> = Vec::new();
    let fits = |out: &[Line], line: usize, item: &LineItem| {
        line >= out.len() || out[line].len + gap + item.len <= limit + EPSILON
    };
    let add = |out: &mut Vec<Line>, line: usize, item: LineItem| {
        while out.len() <= line {
            out.push(Line {
                items: Vec::new(),
                len: -gap,
                thick: 0.,
            });
        }
        let l = &mut out[line];
        l.len += gap + item.len;
        l.thick = l.thick.max(item.thick);
        l.items.push(item);
    };

    let mut rest = Vec::new();
    for item in items {
        match kept(item.float) {
            Some(line) if fits(&out, line, &item) => add(&mut out, line, item),
            _ => rest.push(item),
        }
    }
    for item in rest {
        let mut line = 0;
        while !fits(&out, line, &item) {
            line += 1;
        }
        add(&mut out, line, item);
    }

    out.retain(|l| !l.items.is_empty());
    for l in &mut out {
        l.items
            .sort_by(|a, b| a.at.total_cmp(&b.at).then(a.float.cmp(&b.float)));
    }
    out
}

/// Where along a line its windows start: in their order, each as close to where it is on the
/// workspace as the others and the ends of the line let it, apart by `gap`.
fn spread(items: &[LineItem], range: Range<f64>, gap: f64) -> Vec<(LineItem, f64)> {
    // Runs of windows that touch, with their start and length.
    let mut runs: Vec<(Vec<LineItem>, f64, f64)> = Vec::new();
    let place = |run: &[LineItem], len: f64| {
        let mut offset = 0.;
        let mut sum = 0.;
        for item in run {
            sum += item.at - item.len / 2. - offset;
            offset += item.len + gap;
        }
        let start = sum / run.len() as f64;
        start.clamp(range.start, (range.end - len).max(range.start))
    };
    for &item in items {
        let start = place(&[item], item.len);
        runs.push((vec![item], start, item.len));
        // Merge with the run before while they overlap.
        while runs.len() > 1 {
            let (_, start, _) = runs[runs.len() - 1];
            let (_, prev_start, prev_len) = &runs[runs.len() - 2];
            if prev_start + prev_len + gap <= start + EPSILON {
                break;
            }
            let (run, _, len) = runs.pop().unwrap();
            let prev = runs.last_mut().unwrap();
            prev.0.extend(run);
            prev.2 += gap + len;
            prev.1 = place(&prev.0, prev.2);
        }
    }

    let mut out = Vec::new();
    for (run, start, _) in runs {
        let mut x = start;
        for item in run {
            out.push((item, x));
            x += item.len + gap;
        }
    }
    out
}
