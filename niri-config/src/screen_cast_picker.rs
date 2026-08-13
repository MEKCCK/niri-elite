use crate::utils::MergeWith;
use crate::{Color, FloatOrInt};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenCastPicker {
    pub background_color: Color,
    pub accent_color: Color,
    pub text_color: Color,
    pub accent_text_color: Color,
    pub corner_radius: f64,
}

impl Default for ScreenCastPicker {
    fn default() -> Self {
        Self {
            background_color: Color::new_unpremul(0.1, 0.1, 0.1, 1.),
            accent_color: Color::new_unpremul(1., 1., 1., 1.),
            text_color: Color::new_unpremul(1., 1., 1., 1.),
            accent_text_color: Color::new_unpremul(0.1, 0.1, 0.1, 1.),
            corner_radius: 16.,
        }
    }
}

#[derive(knuffel::Decode, Debug, Default, Clone, Copy, PartialEq)]
pub struct ScreenCastPickerPart {
    #[knuffel(child)]
    pub background_color: Option<Color>,
    #[knuffel(child)]
    pub accent_color: Option<Color>,
    #[knuffel(child)]
    pub text_color: Option<Color>,
    #[knuffel(child)]
    pub accent_text_color: Option<Color>,
    #[knuffel(child, unwrap(argument))]
    pub corner_radius: Option<FloatOrInt<0, 65535>>,
}

impl MergeWith<ScreenCastPickerPart> for ScreenCastPicker {
    fn merge_with(&mut self, part: &ScreenCastPickerPart) {
        merge_clone!(
            (self, part),
            background_color,
            accent_color,
            text_color,
            accent_text_color,
        );
        merge!((self, part), corner_radius);
    }
}
