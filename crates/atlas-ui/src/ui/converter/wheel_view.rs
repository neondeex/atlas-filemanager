//! The format ring as its own entity. Hover, selection and entry animations
//! request frames here, so only the ring re-renders, never the whole window.
use super::{
    geometry,
    render::{ease_out_back, ease_out_cubic, entry_progress, sector_alpha, smooth},
    wheel,
};
use crate::app::App;
use gpui::prelude::*;
use gpui::ColorExt as _;
use gpui::{div, px, Context, IntoElement, MouseButton, Render, WeakEntity, Window};
use gpui_component::ActiveTheme;
use std::time::Instant;

/// Pick readable ink against the actual opaque sector, including custom themes.
fn sector_ink(background: gpui::Hsla) -> gpui::Hsla {
    let rgba = gpui::hsla_to_rgba(background);
    let linear = |channel: f32| {
        if channel <= 0.04045 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    };
    let luminance =
        0.2126 * linear(rgba.red) + 0.7152 * linear(rgba.green) + 0.0722 * linear(rgba.blue);
    if luminance > 0.179 {
        gpui::black()
    } else {
        gpui::white()
    }
}

#[cfg(test)]
mod contrast_tests {
    use super::*;

    #[test]
    fn sector_labels_choose_contrasting_ink_in_dark_light_and_accent_colors() {
        for (background, expected) in [
            (gpui::black(), gpui::white()),
            (gpui::white(), gpui::black()),
            (gpui::yellow(), gpui::black()),
            (gpui::blue(), gpui::white()),
            (gpui::rgb_to_hsla(gpui::rgb(0xff6600)), gpui::black()),
            (gpui::rgb_to_hsla(gpui::rgb(0x18181b)), gpui::white()),
        ] {
            assert_eq!(sector_ink(background), expected);
        }
    }
}

#[derive(Clone, PartialEq)]
pub(super) struct WheelOption {
    pub label: gpui::SharedString,
    pub available: bool,
    pub engine: String,
}

#[derive(Clone, PartialEq)]
pub(super) struct WheelProps {
    pub scale: f32,
    pub options: Vec<WheelOption>,
    pub selected: usize,
    pub hovered: Option<usize>,
    pub glass: bool,
    pub dark: bool,
    pub engine_needed: String,
}

pub(super) struct WheelView {
    app: WeakEntity<App>,
    pub props: WheelProps,
    opened: Instant,
    /// Per-sector hover / selection amounts, eased toward their targets.
    hover_amt: Vec<f32>,
    sel_amt: Vec<f32>,
    tick: Option<Instant>,
}

impl WheelView {
    pub fn new(app: WeakEntity<App>, opened: Instant, props: WheelProps) -> Self {
        Self {
            app,
            props,
            opened,
            hover_amt: Vec::new(),
            sel_amt: Vec::new(),
            tick: None,
        }
    }

    /// Ease the amounts; true while anything still moves.
    fn step(&mut self) -> bool {
        let n = self.props.options.len();
        let current = self.props.selected.min(n.saturating_sub(1));
        if self.hover_amt.len() != n {
            self.hover_amt = vec![0.; n];
            self.sel_amt = vec![0.; n];
            if n > 0 {
                self.sel_amt[current] = 1.;
            }
        }
        let now = Instant::now();
        let dt = self
            .tick
            .map_or(1. / 60., |t| now.duration_since(t).as_secs_f32())
            .clamp(0.001, 0.05);
        let lab = cfg!(any(feature = "conversion-lab", feature = "ui-lab"));
        let mut moving = false;
        for i in 0..n {
            let hovered = f32::from(u8::from(self.props.hovered == Some(i)));
            let selected = f32::from(u8::from(current == i));
            for (value, target, rate) in [
                (&mut self.hover_amt[i], hovered, 16.),
                (&mut self.sel_amt[i], selected, 20.),
            ] {
                if lab {
                    *value = target;
                    continue;
                }
                *value += (target - *value) * (1. - (-rate * dt).exp());
                if (target - *value).abs() < 0.003 {
                    *value = target;
                } else {
                    moving = true;
                }
            }
        }
        self.tick = moving.then_some(now);
        moving
    }
}

impl Render for WheelView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let progress = entry_progress(self.opened);
        if self.step() || progress < 1.0 {
            window.request_animation_frame();
        }
        let WheelProps {
            scale,
            ref options,
            selected,
            hovered,
            glass: is_glass,
            dark,
            ..
        } = self.props;
        let rings = geometry::rings(options.len().max(1));
        let selected = selected.min(options.len().saturating_sub(1));
        // The ring grows from the click point with a small overshoot.
        let scale = scale * (0.80 + 0.20 * ease_out_back(smooth(progress, 0.0, 0.7)));
        let diameter = geometry::radius(&rings) * 2.0 * scale;

        let (accent, fg, muted, border, popover, secondary) = {
            let t = cx.theme();
            (
                t.accent,
                t.foreground,
                t.muted_foreground,
                t.border,
                t.popover,
                t.secondary,
            )
        };
        // Glass sectors sit on a frosted disc (below), so a light veil is
        // enough; a bare translucent fill let file names read through.
        // Opaque sectors keep file names from showing through the labels.
        let sector_fill = secondary.opacity(1.0);
        let sector_disabled = popover.opacity(1.0);
        let active_fill = accent.opacity(1.0);

        // A faint halo keeps the sectors legible over busy file lists.
        let halo = ease_out_cubic(smooth(progress, 0.0, 0.5));
        let mut ring_el = div()
            .relative()
            .size(px(diameter))
            .rounded_full()
            .when(is_glass, |s| {
                s.bg(popover.opacity(halo * if dark { 0.62 } else { 0.70 }))
                    .backdrop_blur(px(18.0 * scale))
            })
            .shadow(vec![gpui::BoxShadow {
                color: gpui::hsla(0., 0., 0., halo * if dark { 0.30 } else { 0.16 }).into(),
                offset: gpui::point(px(0.0), px(5.0 * scale)),
                blur_radius: px(24.0 * scale),
                spread_radius: px(-4.0 * scale),
                inset: false,
            }])
            .child(wheel::paint_wheel(
                scale,
                rings.clone(),
                selected,
                hovered,
                options.iter().map(|o| o.available).collect(),
                (
                    sector_fill,
                    active_fill,
                    sector_disabled,
                    accent.opacity(0.22),
                ),
                progress,
                (self.hover_amt.clone(), self.sel_amt.clone()),
                self.app.clone(),
            ));

        let center = diameter * 0.5;
        for (k, ring) in rings.iter().enumerate() {
            let radius = ring.mid() * scale;
            let arc = std::f32::consts::TAU * ring.mid() / ring.len as f32;
            let width = arc.min(geometry::BAND * 1.7) * scale;
            for local in 0..ring.len {
                let index = ring.start + local;
                let option = &options[index];
                let label = option.label.clone();
                let size = ((if k == 0 { 15. } else { 13. })
                    * if label.len() > 4 { 0.84 } else { 1.0 }
                    * scale)
                    .max(9.);
                let angle = geometry::angle(local, ring.len);
                let x = center + radius * angle.cos();
                let y = center + radius * angle.sin();
                let order = (index + options.len() - selected) % options.len();
                let lift = 2.5 * scale * self.hover_amt.get(index).copied().unwrap_or(0.);
                ring_el = ring_el.child(
                    div()
                        .absolute()
                        .opacity(sector_alpha(progress, order, options.len()))
                        .left(px(x - width * 0.5))
                        .top(px(y - size * 0.7 - lift))
                        .w(px(width))
                        .h(px(size * 1.4))
                        .text_center()
                        .truncate()
                        .text_size(px(size))
                        .line_height(px(size * 1.4))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(if selected == index {
                            sector_ink(active_fill)
                        } else {
                            sector_ink(if option.available {
                                sector_fill
                            } else {
                                sector_disabled
                            })
                        })
                        .child(label),
                );
            }
        }

        let option = &options[selected];
        let label = option.label.clone();
        let enabled = option.available;
        let engine_name = option.engine.clone();
        let engine_needed = self.props.engine_needed.clone();

        // Center action disc
        let hub = (geometry::HUB - geometry::GAP) * 2.0 * scale;
        let roomy = scale >= 0.8;
        let app = self.app.clone();
        ring_el.child(
            div()
                .id("convert-hub")
                .absolute()
                .left(px(center - hub * 0.5))
                .top(px(center - hub * 0.5))
                .size(px(hub))
                .rounded_full()
                .bg(if is_glass {
                    popover.opacity(0.75)
                } else {
                    popover.opacity(0.95)
                })
                .when(is_glass, |s| s.backdrop_blur(px(14.0)))
                .border_1()
                .border_color(border.opacity(0.55))
                .shadow_md()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_0p5()
                .when(enabled, |s| {
                    s.cursor_pointer().hover(move |s| {
                        s.border_color(accent)
                            .bg(accent.opacity(if dark { 0.15 } else { 0.08 }))
                    })
                })
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    if enabled {
                        let _ = app.update(cx, |app, cx| {
                            app.start_conversion();
                            cx.notify();
                        });
                    }
                    cx.stop_propagation();
                })
                .child(
                    div()
                        .text_size(px((17. * scale).max(10.)))
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(fg)
                        .child(label),
                )
                .when(roomy, |s| {
                    s.child(
                        div()
                            .px_1p5()
                            .rounded_full()
                            .bg(if enabled {
                                accent.opacity(0.16)
                            } else {
                                muted.opacity(0.12)
                            })
                            .text_size(px(10. * scale))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(if enabled { accent } else { muted })
                            .child(if enabled { engine_name } else { engine_needed }),
                    )
                }),
        )
    }
}
