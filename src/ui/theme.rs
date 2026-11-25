//! Palette, typography, spacing and widget styles — the single source of
//! truth for the look (architecture §4). Values recreate Telegram Desktop's
//! default "Night" theme by eye; no Telegram assets are used.

use iced::widget::{button, container, scrollable, svg, text_input};
use iced::{Background, Border, Color, Font, Shadow, Theme, Vector, border, font};

/// Bundled Noto Color Emoji (CBDT, SIL OFL 1.1) so emoji render in colour with
/// no system font installed. Same family name as the system package.
pub const NOTO_COLOR_EMOJI: &[u8] = include_bytes!("../../assets/fonts/NotoColorEmoji.ttf");
pub const INTER_REGULAR: &[u8] = include_bytes!("../../assets/fonts/Inter-Regular.ttf");
pub const INTER_SEMIBOLD: &[u8] = include_bytes!("../../assets/fonts/Inter-SemiBold.ttf");

pub const FONT: Font = Font::with_name("Inter");
pub const FONT_SEMIBOLD: Font = Font {
    family: font::Family::Name("Inter"),
    weight: font::Weight::Semibold,
    stretch: font::Stretch::Normal,
    style: font::Style::Normal,
};

const fn rgb(hex: u32) -> Color {
    Color {
        r: ((hex >> 16) & 0xff) as f32 / 255.0,
        g: ((hex >> 8) & 0xff) as f32 / 255.0,
        b: (hex & 0xff) as f32 / 255.0,
        a: 1.0,
    }
}

const fn rgba(hex: u32, a: f32) -> Color {
    let c = rgb(hex);
    Color { a, ..c }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub bg_app: Color,
    pub bg_sidebar: Color,
    pub bg_panel: Color,
    pub bg_elevated: Color,
    pub bg_hover: Color,
    pub bg_selected: Color,
    pub text_primary: Color,
    pub text_secondary: Color,
    pub text_muted: Color,
    pub accent: Color,
    pub accent_text: Color,
    pub danger: Color,
    pub success: Color,
    pub border: Color,
    pub bubble_out: Color,
    pub bubble_in: Color,
    pub badge: Color,
    pub badge_muted: Color,
    // Telegram-specific extras.
    pub bg_input: Color,
    pub time_in: Color,
    pub time_out: Color,
    pub check_out: Color,
    pub service_bg: Color,
    pub link: Color,
    pub selected_text: Color,
    pub reaction_in: Color,
    pub reaction_out: Color,
}

/// Telegram Desktop "Night" (default dark) palette, recreated.
pub const NIGHT: Palette = Palette {
    bg_app: rgb(0x0e1621),
    bg_sidebar: rgb(0x17212b),
    bg_panel: rgb(0x0e1621),
    bg_elevated: rgb(0x182533),
    bg_hover: rgb(0x202b36),
    bg_selected: rgb(0x2b5278),
    text_primary: rgb(0xf5f5f5),
    text_secondary: rgb(0x7f91a4),
    text_muted: rgb(0x6c7883),
    accent: rgb(0x5288c1),
    accent_text: rgb(0xffffff),
    danger: rgb(0xec3942),
    success: rgb(0x4dcd5e),
    border: rgb(0x0a121b),
    bubble_out: rgb(0x2b5278),
    bubble_in: rgb(0x182533),
    badge: rgb(0x3e6a96),
    badge_muted: rgb(0x3b4856),
    bg_input: rgb(0x242f3d),
    time_in: rgb(0x6d7f8f),
    time_out: rgb(0x7da8d3),
    check_out: rgb(0x64b5f6),
    service_bg: rgba(0x1e2c3a, 0.92),
    link: rgb(0x6ab3f3),
    selected_text: rgb(0xffffff),
    reaction_in: rgb(0x223448),
    reaction_out: rgb(0x386291),
};

/// Telegram Desktop default light palette, recreated.
pub const DAY: Palette = Palette {
    bg_app: rgb(0xe6ebee),
    bg_sidebar: rgb(0xffffff),
    bg_panel: rgb(0xdfe7ee),
    bg_elevated: rgb(0xffffff),
    bg_hover: rgb(0xf1f1f1),
    bg_selected: rgb(0x419fd9),
    text_primary: rgb(0x000000),
    text_secondary: rgb(0x999999),
    text_muted: rgb(0xa0acb6),
    accent: rgb(0x40a7e3),
    accent_text: rgb(0xffffff),
    danger: rgb(0xd14e4e),
    success: rgb(0x4fae4e),
    border: rgb(0xdadada),
    bubble_out: rgb(0xeffdde),
    bubble_in: rgb(0xffffff),
    badge: rgb(0x4ea4f6),
    badge_muted: rgb(0xbbbbbb),
    bg_input: rgb(0xf1f1f1),
    time_in: rgb(0xa0acb6),
    time_out: rgb(0x6cc264),
    check_out: rgb(0x5dc452),
    service_bg: rgba(0x6a8296, 0.55),
    link: rgb(0x168acd),
    selected_text: rgb(0xffffff),
    reaction_in: rgb(0xe8f3fc),
    reaction_out: rgb(0xd5f2c2),
};

/// Telegram's placeholder avatar colours (top, bottom of the gradient).
pub const AVATAR_COLORS: [(Color, Color); 7] = [
    (rgb(0xff885e), rgb(0xff516a)), // red
    (rgb(0xffcd6a), rgb(0xffa85c)), // orange
    (rgb(0x82b1ff), rgb(0x665fff)), // violet
    (rgb(0xa0de7e), rgb(0x54cb68)), // green
    (rgb(0x53edd6), rgb(0x28c9b7)), // cyan
    (rgb(0x72d5fd), rgb(0x2a9ef1)), // blue
    (rgb(0xe0a2f3), rgb(0xd669ed)), // pink
];

/// Colours used for sender names in groups (same order as avatars).
pub const NAME_COLORS: [Color; 7] = [
    rgb(0xfc5c51),
    rgb(0xfa790f),
    rgb(0x895dd5),
    rgb(0x0fb297),
    rgb(0x00c1a6),
    rgb(0x3ca5ec),
    rgb(0xd54fb4),
];

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    pub sidebar_width: f32,
    pub chat_row_height: f32,
    pub avatar_list: f32,
    pub avatar_header: f32,
    pub avatar_msg: f32,
    pub header_height: f32,
    pub composer_height: f32,
    pub bubble_radius: f32,
    pub bubble_tail_radius: f32,
    pub bubble_max_width: f32,
    pub space_xs: f32,
    pub space_s: f32,
    pub space_m: f32,
    pub space_l: f32,
    pub font_xs: f32,
    pub font_s: f32,
    pub font_m: f32,
    pub font_l: f32,
    pub font_xl: f32,
}

pub const METRICS: Metrics = Metrics {
    sidebar_width: 340.0,
    chat_row_height: 62.0,
    avatar_list: 50.0,
    avatar_header: 40.0,
    avatar_msg: 33.0,
    header_height: 54.0,
    composer_height: 52.0,
    bubble_radius: 14.0,
    bubble_tail_radius: 4.0,
    bubble_max_width: 470.0,
    space_xs: 2.0,
    space_s: 6.0,
    space_m: 10.0,
    space_l: 16.0,
    font_xs: 12.0,
    font_s: 13.0,
    font_m: 14.5,
    font_l: 16.0,
    font_xl: 22.0,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FerroTheme {
    pub palette: Palette,
    pub metrics: Metrics,
    pub dark: bool,
}

impl FerroTheme {
    pub fn night() -> Self {
        Self {
            palette: NIGHT,
            metrics: METRICS,
            dark: true,
        }
    }

    pub fn day() -> Self {
        Self {
            palette: DAY,
            metrics: METRICS,
            dark: false,
        }
    }

    pub fn iced(&self) -> Theme {
        let p = &self.palette;
        Theme::custom(
            if self.dark {
                "Ferrogram Night"
            } else {
                "Ferrogram Day"
            },
            iced::theme::Palette {
                background: p.bg_app,
                text: p.text_primary,
                primary: p.accent,
                success: p.success,
                warning: rgb(0xe5a64e),
                danger: p.danger,
            },
        )
    }
}

// ------------------------------------------------------------------ styles

/// Dim layer behind modal cards.
pub const SCRIM: Color = Color::from_rgba(0.0, 0.0, 0.0, 0.55);

// ---- calls (ui/call.rs)
pub const CALL_PANEL_WIDTH: f32 = 340.0;
pub const CALL_PANEL_HEIGHT: f32 = 500.0;
pub const CALL_AVATAR: f32 = 120.0;
pub const CALL_BUTTON: f32 = 56.0;
pub const CALL_BUTTON_ICON: f32 = 24.0;
pub const CALL_BAR_HEIGHT: f32 = 38.0;
pub const CALL_EMOJI: f32 = 26.0;
pub const CALL_SIGNAL_BAR: f32 = 4.0;
/// The call panel background (Telegram's call window is a deep blue).
pub const CALL_BG: Color = rgb(0x1b2838);
pub const CALL_BUTTON_BG: Color = Color::from_rgba(1.0, 1.0, 1.0, 0.12);
pub const CALL_BUTTON_ON: Color = Color::from_rgba(1.0, 1.0, 1.0, 0.9);

/// A filled circular button (call controls).
pub fn round_button(bg: Color) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let k = if matches!(status, button::Status::Hovered | button::Status::Pressed) {
            1.12
        } else {
            1.0
        };
        button::Style {
            background: Some(Background::Color(Color {
                r: (bg.r * k).min(1.0),
                g: (bg.g * k).min(1.0),
                b: (bg.b * k).min(1.0),
                a: if k > 1.0 { (bg.a * 1.4).min(1.0) } else { bg.a },
            })),
            text_color: Color::WHITE,
            border: border::rounded(CALL_BUTTON / 2.0),
            shadow: Shadow::default(),
            snap: true,
        }
    }
}

/// The call card.
pub fn call_card(_p: Palette) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(Background::Color(CALL_BG)),
        border: Border {
            radius: 12.0.into(),
            width: 0.0,
            color: Color::TRANSPARENT,
        },
        shadow: Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.5),
            offset: Vector::new(0.0, 4.0),
            blur_radius: 18.0,
        },
        ..container::Style::default()
    }
}

/// The in-call bar above the chat (click to open the panel).
pub fn call_bar(p: Palette) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let k = if matches!(status, button::Status::Hovered) {
            1.08
        } else {
            1.0
        };
        button::Style {
            background: Some(Background::Color(Color {
                r: (p.success.r * k).min(1.0),
                g: (p.success.g * k).min(1.0),
                b: (p.success.b * k).min(1.0),
                a: 1.0,
            })),
            text_color: Color::WHITE,
            border: Border::default(),
            shadow: Shadow::default(),
            snap: true,
        }
    }
}

pub fn fill(color: Color) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(Background::Color(color)),
        ..container::Style::default()
    }
}

pub fn pill(bg: Color, radius: f32) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(Background::Color(bg)),
        border: border::rounded(radius),
        ..container::Style::default()
    }
}

/// Gradient avatar circle.
pub fn avatar(index: usize, size: f32) -> impl Fn(&Theme) -> container::Style {
    let (top, bottom) = AVATAR_COLORS[index % AVATAR_COLORS.len()];
    move |_| container::Style {
        background: Some(Background::Gradient(iced::Gradient::Linear(
            iced::gradient::Linear::new(std::f32::consts::PI)
                .add_stop(0.0, top)
                .add_stop(1.0, bottom),
        ))),
        border: border::rounded(size / 2.0),
        text_color: Some(Color::WHITE),
        ..container::Style::default()
    }
}

/// Message bubble. `tail` keeps one corner almost square where the tail sits.
pub fn bubble(
    p: Palette,
    m: Metrics,
    outgoing: bool,
    tail: bool,
) -> impl Fn(&Theme) -> container::Style {
    move |_| {
        let r = m.bubble_radius;
        let t = if tail { m.bubble_tail_radius } else { r };
        let radius = if outgoing {
            border::Radius {
                top_left: r,
                top_right: r,
                bottom_right: t,
                bottom_left: r,
            }
        } else {
            border::Radius {
                top_left: r,
                top_right: r,
                bottom_right: r,
                bottom_left: t,
            }
        };
        container::Style {
            background: Some(Background::Color(if outgoing {
                p.bubble_out
            } else {
                p.bubble_in
            })),
            border: Border {
                radius,
                ..Border::default()
            },
            ..container::Style::default()
        }
    }
}

pub fn elevated(p: Palette) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(Background::Color(p.bg_elevated)),
        border: Border {
            radius: 10.0.into(),
            width: 1.0,
            color: p.border,
        },
        shadow: Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.45),
            offset: Vector::new(0.0, 3.0),
            blur_radius: 12.0,
        },
        ..container::Style::default()
    }
}

pub fn chat_row(p: Palette, selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let bg = if selected {
            p.bg_selected
        } else if matches!(status, button::Status::Hovered | button::Status::Pressed) {
            p.bg_hover
        } else {
            p.bg_sidebar
        };
        button::Style {
            background: Some(Background::Color(bg)),
            text_color: p.text_primary,
            border: Border::default(),
            shadow: Shadow::default(),
            snap: true,
        }
    }
}

/// Flat icon button (header, composer).
pub fn icon_button(p: Palette, round: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| button::Style {
        background: matches!(status, button::Status::Hovered | button::Status::Pressed)
            .then_some(Background::Color(p.bg_hover)),
        text_color: p.text_secondary,
        border: border::rounded(if round { 20.0 } else { 6.0 }),
        shadow: Shadow::default(),
        snap: true,
    }
}

/// A folder tab: plain text, accent colour when selected (the underline is
/// drawn by the strip).
pub fn folder_tab(p: Palette, selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| button::Style {
        background: matches!(status, button::Status::Hovered | button::Status::Pressed)
            .then_some(Background::Color(p.bg_hover)),
        text_color: if selected { p.link } else { p.text_secondary },
        border: border::rounded(6.0),
        shadow: Shadow::default(),
        snap: true,
    }
}

pub fn menu_item(p: Palette, danger: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| button::Style {
        background: matches!(status, button::Status::Hovered | button::Status::Pressed)
            .then_some(Background::Color(p.bg_hover)),
        text_color: if danger { p.danger } else { p.text_primary },
        border: border::rounded(6.0),
        shadow: Shadow::default(),
        snap: true,
    }
}

pub fn primary_button(p: Palette) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let bg = match status {
            button::Status::Hovered => Color {
                r: p.accent.r * 1.08,
                g: p.accent.g * 1.08,
                b: p.accent.b * 1.08,
                a: 1.0,
            },
            button::Status::Disabled => Color { a: 0.5, ..p.accent },
            _ => p.accent,
        };
        button::Style {
            background: Some(Background::Color(bg)),
            text_color: p.accent_text,
            border: border::rounded(8.0),
            shadow: Shadow::default(),
            snap: true,
        }
    }
}

/// On/off switch in the settings panel (accent when on, like the official
/// client's toggles).
pub fn switch(
    p: Palette,
) -> impl Fn(&Theme, iced::widget::toggler::Status) -> iced::widget::toggler::Style {
    use iced::widget::toggler::{Status, Style};
    move |_, status| {
        let on = match status {
            Status::Active { is_toggled } | Status::Hovered { is_toggled } => is_toggled,
            Status::Disabled { is_toggled } => is_toggled,
        };
        let off_track = p.badge_muted;
        Style {
            background: Background::Color(if on { p.accent } else { off_track }),
            background_border_width: 0.0,
            background_border_color: Color::TRANSPARENT,
            foreground: Background::Color(Color::WHITE),
            foreground_border_width: 0.0,
            foreground_border_color: Color::TRANSPARENT,
            text_color: Some(p.text_primary),
            border_radius: None,
            padding_ratio: 0.2,
        }
    }
}

pub fn link_button(p: Palette) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, _| button::Style {
        background: None,
        text_color: p.link,
        border: Border::default(),
        shadow: Shadow::default(),
        snap: true,
    }
}

pub fn reaction_chip(
    p: Palette,
    outgoing: bool,
    chosen: bool,
) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let base = if chosen {
            if outgoing { p.time_out } else { p.accent }
        } else if outgoing {
            p.reaction_out
        } else {
            p.reaction_in
        };
        let bg = if matches!(status, button::Status::Hovered) {
            Color { a: 0.85, ..base }
        } else {
            base
        };
        button::Style {
            background: Some(Background::Color(bg)),
            text_color: if chosen { Color::WHITE } else { p.text_primary },
            border: border::rounded(13.0),
            shadow: Shadow::default(),
            snap: true,
        }
    }
}

/// Borderless input (composer, sidebar search, login fields).
pub fn input(
    p: Palette,
    bg: Color,
    radius: f32,
    focused_border: bool,
) -> impl Fn(&Theme, text_input::Status) -> text_input::Style {
    move |_, status| {
        let focused = matches!(status, text_input::Status::Focused { .. });
        text_input::Style {
            background: Background::Color(bg),
            border: Border {
                radius: radius.into(),
                width: if focused_border {
                    if focused { 2.0 } else { 1.0 }
                } else {
                    0.0
                },
                color: if focused { p.accent } else { p.bg_input },
            },
            icon: p.text_muted,
            placeholder: p.text_muted,
            value: p.text_primary,
            selection: Color {
                a: 0.45,
                ..p.accent
            },
        }
    }
}

pub fn scroll(p: Palette) -> impl Fn(&Theme, scrollable::Status) -> scrollable::Style {
    move |_, status| {
        let active = !matches!(status, scrollable::Status::Active { .. });
        let scroller = scrollable::Scroller {
            background: Background::Color(if active {
                Color {
                    a: 0.35,
                    ..p.text_secondary
                }
            } else {
                Color::TRANSPARENT
            }),
            border: border::rounded(3.0),
        };
        let rail = scrollable::Rail {
            background: None,
            border: Border::default(),
            scroller,
        };
        scrollable::Style {
            container: container::Style::default(),
            vertical_rail: rail,
            horizontal_rail: rail,
            gap: None,
            auto_scroll: scrollable::AutoScroll {
                background: Background::Color(p.bg_elevated),
                border: border::rounded(8.0),
                shadow: Shadow::default(),
                icon: p.text_secondary,
            },
        }
    }
}

pub fn tint(color: Color) -> impl Fn(&Theme, svg::Status) -> svg::Style {
    move |_, _| svg::Style { color: Some(color) }
}

/// Height of the "new version available" bar at the top of the window.
pub const UPDATE_BAR_HEIGHT: f32 = 36.0;

/// The update bar: the accent colour, like Telegram Desktop's update button.
pub fn update_bar(p: Palette) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(Background::Color(p.accent)),
        text_color: Some(p.accent_text),
        ..container::Style::default()
    }
}

/// A flat text button on the update bar (translucent white on hover).
pub fn update_bar_button(
    p: Palette,
    strong: bool,
) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let base = if strong { 0.18 } else { 0.0 };
        let a = match status {
            button::Status::Hovered | button::Status::Pressed => base + 0.14,
            _ => base,
        };
        button::Style {
            background: (a > 0.0).then_some(Background::Color(Color::from_rgba(1.0, 1.0, 1.0, a))),
            text_color: p.accent_text,
            border: border::rounded(6.0),
            shadow: Shadow::default(),
            snap: true,
        }
    }
}

/// A secondary (outlined) button in dialogs.
pub fn secondary_button(p: Palette) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| button::Style {
        background: Some(Background::Color(match status {
            button::Status::Hovered | button::Status::Pressed => p.bg_hover,
            _ => Color::TRANSPARENT,
        })),
        text_color: match status {
            button::Status::Disabled => p.text_muted,
            _ => p.accent,
        },
        border: Border {
            radius: 8.0.into(),
            width: 1.0,
            color: p.border,
        },
        shadow: Shadow::default(),
        snap: true,
    }
}
