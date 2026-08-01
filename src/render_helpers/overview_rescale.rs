use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::utils::{CommitCounter, DamageSet, OpaqueRegions};
use smithay::backend::renderer::Renderer;
use smithay::utils::user_data::UserDataMap;
use smithay::utils::{Buffer, Physical, Point, Rectangle, Scale, Transform};

/// Rescales an element while rounding both rectangle endpoints consistently.
#[derive(Debug)]
pub struct OverviewRescaleRenderElement<E> {
    element: E,
    origin: Point<i32, Physical>,
    scale: Scale<f64>,
}

impl<E> OverviewRescaleRenderElement<E> {
    pub fn from_element(
        element: E,
        origin: Point<i32, Physical>,
        scale: impl Into<Scale<f64>>,
    ) -> Self {
        Self {
            element,
            origin,
            scale: scale.into(),
        }
    }
}

fn scale_rect_round(
    mut rect: Rectangle<i32, Physical>,
    origin: Point<i32, Physical>,
    scale: Scale<f64>,
) -> Rectangle<i32, Physical> {
    rect.loc -= origin;

    let x1 = (f64::from(rect.loc.x) * scale.x).round() as i32;
    let y1 = (f64::from(rect.loc.y) * scale.y).round() as i32;
    let x2 = (f64::from(rect.loc.x + rect.size.w) * scale.x).round() as i32;
    let y2 = (f64::from(rect.loc.y + rect.size.h) * scale.y).round() as i32;

    Rectangle::new(Point::from((x1, y1)) + origin, (x2 - x1, y2 - y1).into())
}

fn scale_rect_outward(
    rect: Rectangle<i32, Physical>,
    scale: Scale<f64>,
) -> Rectangle<i32, Physical> {
    let x1 = (f64::from(rect.loc.x) * scale.x).floor() as i32;
    let y1 = (f64::from(rect.loc.y) * scale.y).floor() as i32;
    let x2 = (f64::from(rect.loc.x + rect.size.w) * scale.x).ceil() as i32;
    let y2 = (f64::from(rect.loc.y + rect.size.h) * scale.y).ceil() as i32;

    Rectangle::new((x1, y1).into(), (x2 - x1, y2 - y1).into())
}

fn scale_rect_inward(
    rect: Rectangle<i32, Physical>,
    scale: Scale<f64>,
) -> Option<Rectangle<i32, Physical>> {
    let x1 = (f64::from(rect.loc.x) * scale.x).ceil() as i32;
    let y1 = (f64::from(rect.loc.y) * scale.y).ceil() as i32;
    let x2 = (f64::from(rect.loc.x + rect.size.w) * scale.x).floor() as i32;
    let y2 = (f64::from(rect.loc.y + rect.size.h) * scale.y).floor() as i32;

    (x1 < x2 && y1 < y2).then(|| Rectangle::new((x1, y1).into(), (x2 - x1, y2 - y1).into()))
}

impl<E: Element> Element for OverviewRescaleRenderElement<E> {
    fn id(&self) -> &Id {
        self.element.id()
    }

    fn current_commit(&self) -> CommitCounter {
        self.element.current_commit()
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.element.src()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        scale_rect_round(self.element.geometry(scale), self.origin, self.scale)
    }

    fn transform(&self) -> Transform {
        self.element.transform()
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        self.element
            .damage_since(scale, commit)
            .into_iter()
            .map(|rect| scale_rect_outward(rect, self.scale))
            .collect()
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        self.element
            .opaque_regions(scale)
            .into_iter()
            .filter_map(|rect| scale_rect_inward(rect, self.scale))
            .collect()
    }

    fn alpha(&self) -> f32 {
        self.element.alpha()
    }

    fn kind(&self) -> Kind {
        self.element.kind()
    }

    fn is_framebuffer_effect(&self) -> bool {
        self.element.is_framebuffer_effect()
    }
}

impl<R: Renderer, E: RenderElement<R>> RenderElement<R> for OverviewRescaleRenderElement<E> {
    fn draw(
        &self,
        frame: &mut R::Frame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), R::Error> {
        self.element
            .draw(frame, src, dst, damage, opaque_regions, cache)
    }

    fn underlying_storage(&self, renderer: &mut R) -> Option<UnderlyingStorage<'_>> {
        self.element.underlying_storage(renderer)
    }

    fn capture_framebuffer(
        &self,
        frame: &mut R::Frame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        cache: &UserDataMap,
    ) -> Result<(), R::Error> {
        self.element.capture_framebuffer(frame, src, dst, cache)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn odd_sized_adjacent_rects_share_scaled_endpoints() {
        let scale = Scale::from(0.5);
        let origin = Point::default();
        let window: Rectangle<i32, Physical> = Rectangle::new((11, 21).into(), (1265, 1389).into());
        let right_ring = Rectangle::new((1276, 21).into(), (2, 1389).into());
        let bottom_ring = Rectangle::new((11, 1410).into(), (1265, 2).into());

        let window = scale_rect_round(window, origin, scale);
        let right_ring = scale_rect_round(right_ring, origin, scale);
        let bottom_ring = scale_rect_round(bottom_ring, origin, scale);

        assert_ne!(6 + 633, right_ring.loc.x);
        assert_ne!(11 + 695, bottom_ring.loc.y);
        assert_eq!(window.loc.x + window.size.w, right_ring.loc.x);
        assert_eq!(window.loc.y + window.size.h, bottom_ring.loc.y);
    }
}
