//! Settings panel (background mode): close to tray, start minimized, launch
//! at login, the global notification mute and updates.

use iced::widget::{button, column, container, row, text, toggler};
use iced::{Alignment, Element, Length};

use super::theme::{self, FONT_SEMIBOLD};
use super::widgets::modal;
use crate::app::background::TrayStatus;
use crate::app::{App, Message, Ui};
use crate::remote::{MuteRequest, Remote};

fn toggle_row<'a>(
    app: &'a App,
    title: &'a str,
    hint: String,
    on: bool,
    msg: impl Fn(bool) -> Ui + 'a,
) -> Element<'a, Message> {
    let p = app.theme.palette;
    let m = app.theme.metrics;
    row![
        column![
            text(title).size(m.font_m).color(p.text_primary),
            text(hint).size(m.font_xs).color(p.text_secondary),
        ]
        .spacing(2)
        .width(Length::Fill),
        toggler(on)
            .on_toggle(move |b| Message::Ui(msg(b)))
            .size(22)
            .style(theme::switch(p)),
    ]
    .spacing(12)
    .align_y(Alignment::Center)
    .into()
}

/// The modal card over a dimmed window; click outside (or Esc) closes it.
pub fn view(app: &App) -> Element<'_, Message> {
    let p = app.theme.palette;
    let m = app.theme.metrics;

    let tray_hint = match app.window.tray {
        TrayStatus::Available => {
            "Keep running in the system tray when the window is closed".to_owned()
        }
        TrayStatus::Pending => "Looking for a system tray…".to_owned(),
        TrayStatus::Unavailable => "No system tray found: closing the window quits".to_owned(),
    };

    let mute_state = app
        .prefs
        .mute
        .describe(chrono::Utc::now().timestamp(), app.tz_offset);
    let mute_button = |label: &'static str, request| {
        button(text(label).size(m.font_s))
            .padding([6, 12])
            .on_press(Message::Ui(Ui::Remote(Remote::Mute(request))))
            .style(theme::menu_item(p, false))
    };
    let mute_controls: Element<'_, Message> = if mute_state.is_some() {
        mute_button("Unmute", MuteRequest::Off).into()
    } else {
        row![
            mute_button("Mute for 1 hour", MuteRequest::OneHour),
            mute_button("Until re-enabled", MuteRequest::UntilEnabled),
        ]
        .spacing(4)
        .into()
    };

    let mut card = column![
        text("Settings")
            .size(m.font_l)
            .font(FONT_SEMIBOLD)
            .color(p.text_primary),
        toggle_row(
            app,
            "Close to tray",
            tray_hint,
            app.prefs.close_to_tray,
            Ui::SetCloseToTray
        ),
        toggle_row(
            app,
            "Start minimized to tray",
            "Open without a window; click the tray icon to show it".into(),
            app.prefs.start_minimized,
            Ui::SetStartMinimized
        ),
    ]
    .spacing(14);
    if cfg!(target_os = "linux") {
        card = card.push(toggle_row(
            app,
            "Launch at login",
            "Start in the tray when you log in (off by default)".into(),
            app.autostart,
            Ui::SetAutostart,
        ));
    }
    card = card
        .push(
            column![
                text("Notifications").size(m.font_m).color(p.text_primary),
                text(match &mute_state {
                    Some(until) => format!("Muted {until}"),
                    None => "Messages show a desktop notification".to_owned(),
                })
                .size(m.font_xs)
                .color(p.text_secondary),
                mute_controls,
            ]
            .spacing(4),
        )
        .push(super::update::settings_section(app))
        .push(
            container(
                button(text("Done").size(m.font_m))
                    .padding([8, 22])
                    .on_press(Message::Ui(Ui::CloseSettings))
                    .style(theme::primary_button(p)),
            )
            .align_right(Length::Fill),
        );

    let card = container(card)
        .padding(20)
        .width(440)
        .style(theme::elevated(p));
    modal(card, Message::Ui(Ui::CloseSettings))
}
