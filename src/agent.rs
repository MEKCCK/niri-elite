//! Agent seat: a second seat for automation that runs alongside the user's.
//!
//! The seat is created at startup, after the user's seat: GTK 3 ignores seats that appear later,
//! and clients that only handle one seat pick the first one, which stays the user's. It has its own
//! pointer, keyboard focus, popup grabs and clipboard, so clicking and typing through it doesn't
//! move the user's cursor or focus. All points are in window coordinates (see [`niri_ipc::agent`]),
//! and the agent pointer lives in the target window's buffer coordinate space rather than in the
//! global space, so actions don't depend on where the window is on screen.

use std::thread;

use async_channel::Sender;
use base64::Engine as _;
use niri_config::Config;
use niri_ipc::agent::{AgentAction, AgentButton, AgentCapture};
use niri_ipc::{Reply, Response};
use smithay::backend::allocator::Fourcc;
use smithay::backend::input::{Axis, AxisSource, ButtonState, InputTime, KeyState};
use smithay::desktop::{
    find_popup_root_surface, PopupGrab, PopupKeyboardGrab, PopupKind, PopupPointerGrab, Window,
    WindowSurfaceType,
};
use smithay::input::keyboard::{xkb, FilterResult, Keysym};
use smithay::input::pointer::{AxisFrame, ButtonEvent, Focus, MotionEvent};
use smithay::input::{Seat, SeatState};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::utils::{Point, Scale, Transform, SERIAL_COUNTER};
use smithay::wayland::shell::xdg::PopupSurface;

use crate::layout::LayoutElement as _;
use crate::niri::State;
use crate::render_helpers::xray::XrayPos;
use crate::render_helpers::{
    encompassing_geo, render_to_vec, RenderCtx, RenderIntent, RenderTarget,
};
use crate::utils::write_png_rgba8;

const SEAT_NAME: &str = "niri-agent";

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;

/// Logical pixels per wheel notch, for the discrete scroll value.
const WHEEL_STEP: f64 = 15.;

/// Most keysyms `inject_text_keysyms()` can bind in one keymap.
const TYPE_CHUNK: usize = 200;

pub struct AgentState {
    seat: Seat<State>,
    popup_grab: Option<(WlSurface, PopupGrab<State>)>,
}

impl AgentState {
    pub fn new(
        seat_state: &mut SeatState<State>,
        display: &DisplayHandle,
        config: &Config,
    ) -> Self {
        let mut seat = seat_state.new_wl_seat(display, SEAT_NAME);

        // Same layout as the user's keyboard, so key combinations resolve the same way.
        let keyboard = &config.input.keyboard;
        let (delay, rate) = (keyboard.repeat_delay.into(), keyboard.repeat_rate.into());
        if let Err(err) = seat.add_keyboard(keyboard.xkb.to_xkb_config(), delay, rate) {
            warn!("error adding agent keyboard with the configured layout: {err:?}");
            seat.add_keyboard(Default::default(), delay, rate).unwrap();
        }
        seat.add_pointer();

        Self {
            seat,
            popup_grab: None,
        }
    }

    pub fn is_agent_seat(&self, seat: &Seat<State>) -> bool {
        &self.seat == seat
    }
}

struct Target {
    window: Window,
    toplevel: WlSurface,
}

impl State {
    pub fn agent_action(&mut self, action: AgentAction, reply: Sender<Reply>) {
        let result = match action {
            AgentAction::Capture { id } => {
                // Replies from the encoding thread.
                if let Err(err) = self.agent_capture(id, reply.clone()) {
                    let _ = reply.try_send(Err(err));
                }
                return;
            }
            AgentAction::Move { id, x, y } => self.agent_target(id).map(|target| {
                self.agent_pointer_move(&target, Point::from((x, y)));
            }),
            AgentAction::Click {
                id,
                x,
                y,
                button,
                count,
            } => self.agent_target(id).map(|target| {
                self.agent_pointer_move(&target, Point::from((x, y)));
                for _ in 0..count.max(1) {
                    self.agent_button(&target, button, ButtonState::Pressed);
                    self.agent_button(&target, button, ButtonState::Released);
                }
            }),
            AgentAction::Button {
                id,
                x,
                y,
                button,
                pressed,
            } => self.agent_target(id).map(|target| {
                self.agent_pointer_move(&target, Point::from((x, y)));
                let state = if pressed {
                    ButtonState::Pressed
                } else {
                    ButtonState::Released
                };
                self.agent_button(&target, button, state);
            }),
            AgentAction::Scroll { id, x, y, dx, dy } => self.agent_target(id).map(|target| {
                self.agent_pointer_move(&target, Point::from((x, y)));
                self.agent_scroll(dx, dy);
            }),
            AgentAction::Key { id, keys } => self
                .agent_target(id)
                .and_then(|target| self.agent_key(&target, &keys)),
            AgentAction::Type { id, text } => self.agent_target(id).map(|target| {
                self.agent_type(&target, &text);
            }),
        };

        let _ = reply.try_send(result.map(|()| Response::Handled));
    }

    fn agent_seat(&self) -> Seat<State> {
        self.niri.agent.seat.clone()
    }

    fn agent_target(&mut self, id: u64) -> Result<Target, String> {
        let (_, mapped) = self
            .niri
            .layout
            .windows()
            .find(|(_, mapped)| mapped.id().get() == id)
            .ok_or_else(|| format!("window {id} was not found"))?;

        Ok(Target {
            window: mapped.window.clone(),
            toplevel: mapped.toplevel().wl_surface().clone(),
        })
    }

    /// Moves the agent pointer to `pos` in window coordinates.
    fn agent_pointer_move(&mut self, target: &Target, pos: Point<f64, smithay::utils::Logical>) {
        let seat = self.agent_seat();
        let pointer = seat.get_pointer().unwrap();

        // The agent pointer uses the window's buffer coordinates as its global space.
        let location = pos + target.window.geometry().loc.to_f64();
        let under = target
            .window
            .surface_under(location, WindowSurfaceType::ALL)
            .map(|(surface, surface_loc)| (surface, surface_loc.to_f64()));

        pointer.motion(
            self,
            under,
            &MotionEvent {
                location,
                serial: SERIAL_COUNTER.next_serial(),
                time: InputTime::now(),
            },
        );
        pointer.frame(self);
    }

    fn agent_button(&mut self, target: &Target, button: AgentButton, state: ButtonState) {
        let seat = self.agent_seat();
        let pointer = seat.get_pointer().unwrap();

        // Click to focus, like the user's seat. A click inside an open popup is handled by the
        // popup grab instead.
        if state == ButtonState::Pressed && !self.agent_popup_grab_on(&target.toplevel) {
            self.agent_focus(target);
        }

        let button = match button {
            AgentButton::Left => BTN_LEFT,
            AgentButton::Right => BTN_RIGHT,
            AgentButton::Middle => BTN_MIDDLE,
        };
        pointer.button(
            self,
            &ButtonEvent {
                serial: SERIAL_COUNTER.next_serial(),
                time: InputTime::now(),
                button,
                state,
            },
        );
        pointer.frame(self);
    }

    fn agent_scroll(&mut self, dx: f64, dy: f64) {
        let seat = self.agent_seat();
        let pointer = seat.get_pointer().unwrap();

        let mut frame = AxisFrame::new(InputTime::now()).source(AxisSource::Wheel);
        for (axis, value) in [(Axis::Horizontal, dx), (Axis::Vertical, dy)] {
            if value != 0. {
                let v120 = (value / WHEEL_STEP * 120.).round() as i32;
                frame = frame.value(axis, value).v120(axis, v120);
            }
        }
        pointer.axis(self, frame);
        pointer.frame(self);
    }

    fn agent_key(&mut self, target: &Target, keys: &str) -> Result<(), String> {
        let syms = parse_keys(keys)?;

        let seat = self.agent_seat();
        let keyboard = seat.get_keyboard().unwrap();
        let codes = syms
            .iter()
            .map(|sym| {
                keyboard.keycode_for_keysym(*sym).ok_or_else(|| {
                    format!(
                        "key {} is not on the keyboard layout",
                        xkb::keysym_get_name(*sym)
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        self.agent_focus(target);

        let input = |state: &mut State, code, key_state| {
            keyboard.input(
                state,
                code,
                key_state,
                SERIAL_COUNTER.next_serial(),
                InputTime::now(),
                |_, _, _| FilterResult::<()>::Forward,
            );
        };
        for code in &codes {
            input(self, *code, KeyState::Pressed);
        }
        for code in codes.iter().rev() {
            input(self, *code, KeyState::Released);
        }

        Ok(())
    }

    fn agent_type(&mut self, target: &Target, text: &str) {
        let syms: Vec<Keysym> = text
            .chars()
            .map(|c| match c {
                '\n' => Keysym::Return,
                '\t' => Keysym::Tab,
                c => xkb::utf32_to_keysym(c as u32),
            })
            .filter(|sym| *sym != Keysym::NoSymbol)
            .collect();

        self.agent_focus(target);

        let seat = self.agent_seat();
        let keyboard = seat.get_keyboard().unwrap();
        for chunk in syms.chunks(TYPE_CHUNK) {
            keyboard.inject_text_keysyms(self, chunk);
        }
    }

    /// Gives agent keyboard focus to the target window, unless one of its popups holds it.
    fn agent_focus(&mut self, target: &Target) {
        if self.agent_popup_grab_on(&target.toplevel) {
            return;
        }

        let seat = self.agent_seat();
        let keyboard = seat.get_keyboard().unwrap();
        if keyboard.current_focus().as_ref() != Some(&target.toplevel) {
            keyboard.set_focus(
                self,
                Some(target.toplevel.clone()),
                SERIAL_COUNTER.next_serial(),
            );
        }
    }

    fn agent_popup_grab_on(&mut self, root: &WlSurface) -> bool {
        let agent = &mut self.niri.agent;
        if agent
            .popup_grab
            .as_ref()
            .is_some_and(|(_, grab)| grab.has_ended())
        {
            agent.popup_grab = None;
        }
        agent
            .popup_grab
            .as_ref()
            .is_some_and(|(grab_root, _)| grab_root == root)
    }

    /// Handles an xdg_popup grab requested with the agent seat.
    ///
    /// Returns `false` if `seat` is not the agent seat. The agent seat doesn't take part in the
    /// user's focus, so this skips all checks against the user's focus and just grabs.
    pub fn agent_popup_grab(
        &mut self,
        surface: &PopupSurface,
        seat: &Seat<State>,
        serial: smithay::utils::Serial,
    ) -> bool {
        if !self.niri.agent.is_agent_seat(seat) {
            return false;
        }

        let popup = PopupKind::Xdg(surface.clone());
        let Ok(root) = find_popup_root_surface(&popup) else {
            return true;
        };

        let mut grab = match self
            .niri
            .popups
            .grab_popup(root.clone(), popup, seat, serial)
        {
            Ok(grab) => grab,
            Err(err) => {
                trace!("ignoring agent popup grab: {err:?}");
                return true;
            }
        };

        let keyboard = seat.get_keyboard().unwrap();
        let pointer = seat.get_pointer().unwrap();
        let keyboard_grab_mismatches = keyboard.is_grabbed()
            && !(keyboard.has_grab(serial)
                || grab.previous_serial().is_none_or(|s| keyboard.has_grab(s)));
        let pointer_grab_mismatches = pointer.is_grabbed()
            && !(pointer.has_grab(serial)
                || grab.previous_serial().is_none_or(|s| pointer.has_grab(s)));
        if keyboard_grab_mismatches || pointer_grab_mismatches {
            trace!("ignoring agent popup grab because of current grab mismatch");
            grab.ungrab(smithay::desktop::PopupUngrabStrategy::All);
            return true;
        }

        keyboard.set_grab(self, PopupKeyboardGrab::new(&grab), serial);
        pointer.set_grab(self, PopupPointerGrab::new(&grab), serial, Focus::Keep);
        self.niri.agent.popup_grab = Some((root, grab));
        true
    }

    /// Renders the window with its popups and replies with the PNG from a thread.
    fn agent_capture(&mut self, id: u64, reply: Sender<Reply>) -> Result<(), String> {
        let (monitor, mapped) = self
            .niri
            .layout
            .windows()
            .find(|(_, mapped)| mapped.id().get() == id)
            .ok_or_else(|| format!("window {id} was not found"))?;
        let scale = monitor.map_or(1., |mon| mon.output().current_scale().fractional_scale());
        let scale = Scale::from(scale);

        let result = self.backend.with_primary_renderer(|renderer| {
            let mut elements = Vec::new();
            let ctx = RenderCtx {
                renderer,
                target: RenderTarget::ScreenCapture,
                intent: RenderIntent::Normal,
                xray: None,
            };
            // Render the window geometry at the origin, so element positions are in window
            // coordinates.
            mapped.render(
                ctx,
                Point::from((0., 0.)),
                scale,
                1.,
                XrayPos::default(),
                &mut |elem| elements.push(elem),
            );

            let geo = encompassing_geo(scale, elements.iter());
            let elements = elements.iter().rev().map(|elem| {
                smithay::backend::renderer::element::utils::RelocateRenderElement::from_element(
                    elem,
                    geo.loc.upscale(-1),
                    smithay::backend::renderer::element::utils::Relocate::Relative,
                )
            });
            let pixels = render_to_vec(
                renderer,
                geo.size,
                scale,
                Transform::Normal,
                Fourcc::Abgr8888,
                elements,
            )?;
            anyhow::Ok((geo, pixels))
        });
        let (geo, pixels) = result
            .ok_or_else(|| String::from("primary renderer is not available"))?
            .map_err(|err| format!("error rendering window: {err:?}"))?;

        let origin = geo.loc.to_f64().to_logical(scale);
        let scale = scale.x;
        thread::spawn(move || {
            let mut png = Vec::new();
            let result = write_png_rgba8(&mut png, geo.size.w as u32, geo.size.h as u32, &pixels)
                .map(|()| {
                    Response::AgentCapture(AgentCapture {
                        png_base64: base64::engine::general_purpose::STANDARD.encode(&png),
                        origin: (origin.x, origin.y),
                        scale,
                    })
                })
                .map_err(|err| format!("error encoding image: {err:?}"));
            let _ = reply.send_blocking(result);
        });

        Ok(())
    }
}

/// Parses a combination like `ctrl+shift+t` into keysyms, modifiers first.
fn parse_keys(keys: &str) -> Result<Vec<Keysym>, String> {
    let mut parts: Vec<&str> = keys.split('+').map(str::trim).collect();
    // Allow "ctrl+plus" style as well as a literal trailing "+": "ctrl++".
    if keys.ends_with("++") {
        parts.pop();
        parts.pop();
        parts.push("+");
    }
    let (key, mods) = parts
        .split_last()
        .filter(|(key, _)| !key.is_empty())
        .ok_or_else(|| format!("no key in {keys:?}"))?;

    let mut syms = Vec::new();
    for name in mods {
        let sym = match name.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => Keysym::Control_L,
            "shift" => Keysym::Shift_L,
            "alt" => Keysym::Alt_L,
            "super" | "mod4" | "logo" | "win" => Keysym::Super_L,
            _ => return Err(format!("unknown modifier {name:?} in {keys:?}")),
        };
        syms.push(sym);
    }

    let mut sym = xkb::keysym_from_name(key, xkb::KEYSYM_NO_FLAGS);
    if sym == Keysym::NoSymbol {
        sym = xkb::keysym_from_name(key, xkb::KEYSYM_CASE_INSENSITIVE);
    }
    if sym == Keysym::NoSymbol {
        let mut chars = key.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) {
            sym = xkb::utf32_to_keysym(c as u32);
        }
    }
    if sym == Keysym::NoSymbol {
        return Err(format!("unknown key {key:?} in {keys:?}"));
    }
    syms.push(sym);

    Ok(syms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_key_combinations() {
        assert_eq!(parse_keys("Return").unwrap(), [Keysym::Return]);
        assert_eq!(parse_keys("return").unwrap(), [Keysym::Return]);
        assert_eq!(
            parse_keys("ctrl+shift+t").unwrap(),
            [Keysym::Control_L, Keysym::Shift_L, Keysym::t]
        );
        assert_eq!(
            parse_keys("ctrl++").unwrap(),
            [Keysym::Control_L, Keysym::plus]
        );
        assert_eq!(parse_keys("alt+F4").unwrap(), [Keysym::Alt_L, Keysym::F4]);
        assert!(parse_keys("hyper+a").is_err());
        assert!(parse_keys("ctrl+").is_err());
    }
}
