//! Where the windows of a grid with floating windows only go: in rows, the way the window
//! overviews of other desktops show them, by where they are on the screen, top to bottom and left
//! to right.
//!
//! A window stays in the row it was in, and a new one joins the row closest to where it is, so
//! windows coming and going only move the others in their row along, and the rest zooms.

use smithay::utils::{Logical, Point, Rectangle, Size};

use super::bands::{squeeze_exponent, squeezed_scale};
use super::{bisect, rect_center, GridItem, GridSource, EPSILON, MAX_SCALE};

/// A window that opens goes into another row than the closest one if that lets the windows show
/// larger by more than this.
const SCALE_GROW: f64 = 1. / 0.93;
use crate::layout::LayoutElement;

/// The most rows the windows go in.
const MAX_ROWS: usize = 6;

pub(super) struct Rows<'a, W: LayoutElement> {
    items: &'a [(GridItem<W>, GridSource)],
    content: Rectangle<f64, Logical>,
    gap: f64,
    /// How every item shrinks with the scale, like with tiled windows.
    exponents: Vec<f64>,
    /// Where every item is on the workspace.
    centers: Vec<Point<f64, Logical>>,
}

impl<'a, W: LayoutElement> Rows<'a, W> {
    pub fn new(
        items: &'a [(GridItem<W>, GridSource)],
        area: Rectangle<f64, Logical>,
        content: Rectangle<f64, Logical>,
        gap: f64,
    ) -> Self {
        let middle = rect_center(area);
        Self {
            items,
            content,
            gap,
            exponents: items
                .iter()
                .map(|(_, source)| squeeze_exponent(area, source.size))
                .collect(),
            centers: items
                .iter()
                .map(|(_, source)| {
                    source.pos.map_or(middle, |pos| {
                        pos + Point::from((source.size.w / 2., source.size.h / 2.))
                    })
                })
                .collect(),
        }
    }

    pub fn shown_scale(&self, idx: usize, scale: f64) -> f64 {
        squeezed_scale(scale, self.exponents[idx])
    }

    fn size(&self, idx: usize, scale: f64) -> Size<f64, Logical> {
        self.items[idx].1.size.upscale(self.shown_scale(idx, scale))
    }

    /// The width and the height of every row at `scale`.
    fn row_sizes(&self, rows: &[Vec<usize>], scale: f64) -> Vec<(f64, f64)> {
        rows.iter()
            .map(|row| {
                let width = row.iter().map(|&idx| self.size(idx, scale).w).sum::<f64>()
                    + self.gap * row.len().saturating_sub(1) as f64;
                let height = row
                    .iter()
                    .map(|&idx| self.size(idx, scale).h)
                    .fold(0., f64::max);
                (width, height)
            })
            .collect()
    }

    fn fits(&self, rows: &[Vec<usize>], scale: f64) -> bool {
        let sizes = self.row_sizes(rows, scale);
        let height = sizes.iter().map(|(_, h)| h).sum::<f64>()
            + self.gap * sizes.len().saturating_sub(1) as f64;
        sizes
            .iter()
            .all(|(w, _)| *w <= self.content.size.w + EPSILON)
            && height <= self.content.size.h + EPSILON
    }

    /// The largest scale at which `rows` fit.
    pub fn max_scale(&self, rows: &[Vec<usize>], min_scale: f64) -> f64 {
        if self.fits(rows, MAX_SCALE) {
            MAX_SCALE
        } else if !self.fits(rows, min_scale) {
            min_scale
        } else {
            bisect(min_scale, MAX_SCALE, 20, |s| self.fits(rows, s)).0
        }
    }

    /// In every row, the windows by where they are across, the rows by where they are up and
    /// down, without empty rows.
    fn sorted(&self, mut rows: Vec<Vec<usize>>) -> Vec<Vec<usize>> {
        rows.retain(|row| !row.is_empty());
        for row in &mut rows {
            row.sort_by(|&a, &b| {
                self.centers[a]
                    .x
                    .total_cmp(&self.centers[b].x)
                    .then(a.cmp(&b))
            });
        }
        let middle_y = |row: &Vec<usize>| {
            row.iter().map(|&idx| self.centers[idx].y).sum::<f64>() / row.len() as f64
        };
        rows.sort_by(|a, b| middle_y(a).total_cmp(&middle_y(b)));
        rows
    }

    /// The windows in `count` rows from scratch: top to bottom, the rows about as wide as each
    /// other.
    pub fn fresh(&self, count: usize) -> Vec<Vec<usize>> {
        let mut order: Vec<usize> = (0..self.items.len()).collect();
        order.sort_by(|&a, &b| {
            self.centers[a]
                .y
                .total_cmp(&self.centers[b].y)
                .then(self.centers[a].x.total_cmp(&self.centers[b].x))
                .then(a.cmp(&b))
        });
        let width = |idx: usize| self.items[idx].1.size.w;
        let split = |limit: f64| {
            let mut rows: Vec<Vec<usize>> = Vec::new();
            let mut row_width = f64::INFINITY;
            for &idx in &order {
                if row_width + width(idx) <= limit + EPSILON {
                    row_width += width(idx);
                    rows.last_mut().unwrap().push(idx);
                } else {
                    row_width = width(idx);
                    rows.push(vec![idx]);
                }
            }
            rows
        };
        let total: f64 = order.iter().map(|&idx| width(idx)).sum();
        let widest = order.iter().map(|&idx| width(idx)).fold(0., f64::max);
        let (mut lo, mut hi) = (widest, total.max(widest));
        for _ in 0..30 {
            let mid = (lo + hi) / 2.;
            if split(mid).len() <= count {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        self.sorted(split(hi))
    }

    /// The windows in the rows `kept` says they were in, and windows that weren't in any in the
    /// row closest to where they are, unless another row lets the windows show clearly larger.
    /// `None` if no window was in a row.
    pub fn kept(&self, kept: &[(W::Id, usize)], min_scale: f64) -> Option<Vec<Vec<usize>>> {
        let count = kept.iter().map(|(_, row)| row + 1).max()?;
        let mut rows = vec![Vec::new(); count];
        let mut new = Vec::new();
        for (idx, (item, _)) in self.items.iter().enumerate() {
            match kept.iter().find(|(id, _)| id == item.window_id()) {
                Some((_, row)) => rows[*row].push(idx),
                None => new.push(idx),
            }
        }
        rows.retain(|row| !row.is_empty());
        if rows.is_empty() {
            return None;
        }
        for idx in new {
            let distance = |row: &Vec<usize>| {
                let y = row.iter().map(|&i| self.centers[i].y).sum::<f64>() / row.len() as f64;
                (y - self.centers[idx].y).abs()
            };
            let mut by_distance: Vec<usize> = (0..rows.len()).collect();
            by_distance.sort_by(|&a, &b| distance(&rows[a]).total_cmp(&distance(&rows[b])));
            let scale_in = |row: usize| {
                let mut with = rows.clone();
                with[row].push(idx);
                self.max_scale(&with, min_scale)
            };
            let mut best = (by_distance[0], scale_in(by_distance[0]));
            for &row in &by_distance[1..] {
                let scale = scale_in(row);
                if scale > best.1 * SCALE_GROW {
                    best = (row, scale);
                }
            }
            rows[best.0].push(idx);
        }
        Some(self.sorted(rows))
    }

    /// Small changes to the rows the windows were in: none, a row split in two, its upper
    /// windows above the others, and two rows next to each other made one. So that one more
    /// row, or one fewer, doesn't move all the windows.
    pub fn changes(&self, rows: Vec<Vec<usize>>) -> Vec<Vec<Vec<usize>>> {
        let mut out = Vec::new();
        for (i, row) in rows.iter().enumerate() {
            if row.len() < 2 {
                continue;
            }
            let mut by_y = row.clone();
            by_y.sort_by(|&a, &b| {
                self.centers[a]
                    .y
                    .total_cmp(&self.centers[b].y)
                    .then(a.cmp(&b))
            });
            let lower = by_y.split_off(by_y.len() / 2);
            let mut split = rows.clone();
            split[i] = by_y;
            split.insert(i + 1, lower);
            out.push(self.sorted(split));
        }
        for i in 1..rows.len() {
            let mut merged = rows.clone();
            let next = merged.remove(i);
            merged[i - 1].extend(next);
            out.push(self.sorted(merged));
        }
        out.insert(0, rows);
        out
    }

    /// Where every item goes with `rows` at `scale`: every row centered, the rows together in the
    /// middle of the grid.
    pub fn place(&self, rows: &[Vec<usize>], scale: f64) -> Vec<Rectangle<f64, Logical>> {
        let sizes = self.row_sizes(rows, scale);
        let height = sizes.iter().map(|(_, h)| h).sum::<f64>()
            + self.gap * sizes.len().saturating_sub(1) as f64;
        let middle = rect_center(self.content);
        let mut out = vec![Rectangle::default(); self.items.len()];
        let mut y = middle.y - height / 2.;
        for (row, (row_width, row_height)) in rows.iter().zip(sizes) {
            let mut x = middle.x - row_width / 2.;
            for &idx in row {
                let size = self.size(idx, scale);
                out[idx] = Rectangle::new(Point::from((x, y + (row_height - size.h) / 2.)), size);
                x += size.w + self.gap;
            }
            y += row_height + self.gap;
        }
        out
    }

    /// The rows every window is in, by its id.
    pub fn ids(&self, rows: &[Vec<usize>]) -> Vec<(W::Id, usize)> {
        rows.iter()
            .enumerate()
            .flat_map(|(row_idx, row)| {
                row.iter()
                    .map(move |&idx| (self.items[idx].0.window_id().clone(), row_idx))
            })
            .collect()
    }
}

pub(super) const MAX_FLOATING_ROWS: usize = MAX_ROWS;
