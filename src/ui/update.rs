//! The update bar at the top of the window and the "Updates" section of
//! Settings (`crate::update`).

use iced::widget::{Space, button, column, container, row, text, toggler};
use iced::{Alignment, Element, Length};

use super::icons::Icon;
use super::theme::{self, FONT_SEMIBOLD};
use super::widgets::icon;
use crate::app::{App, Message, Ui};
use crate::config::APP_NAME;
use crate::update::state::{Action, Banner, Phase};

fn ui(u: Ui) -> Message {
    Message::Ui(u)
}

/// The slim bar shown while an update is available, installing or waiting
/// for a restart. Never modal: [x] closes it.
pub fn banner(app: &App) -> Option<Element<'_, Message>> {
    let b = app.updater.banner()?;
    let p = app.theme.palette;
    let m = app.theme.metrics;
    let label = |s: String| text(s).size(m.font_s).color(p.accent_text);
    let action = |s: &'static str, msg: Ui, strong: bool| {
        button(text(s).size(m.font_s).font(FONT_SEMIBOLD))
            .padding([4, 10])
            .on_press(ui(msg))
            .style(theme::update_bar_button(p, strong))
    };
    let close = button(icon(Icon::X, 16.0, p.accent_text))
        .padding(4)
        .on_press(ui(Ui::UpdateDismiss))
        .style(theme::update_bar_button(p, false));

    let mut items = row![].spacing(m.space_s).align_y(Alignment::Center);
    match b {
        Banner::Available {
            version,
            action: how,
        } => {
            items = items.push(label(format!("{APP_NAME} {version} is available")));
            match how {
                Action::SelfUpdate => {
                    items = items.push(action("Update", Ui::UpdateInstall, true));
                }
                Action::Cargo(cmd) => {
                    items = items
                        .push(
                            text(format!("Installed with cargo: run {cmd}"))
                                .size(m.font_xs)
                                .color(p.accent_text),
                        )
                        .push(action("Copy command", Ui::UpdateCopyCommand, true));
                }
                Action::OpenPage => {}
            }
            items = items
                .push(action("Release notes", Ui::UpdateReleaseNotes, false))
                .push(Space::new().width(Length::Fill))
                .push(close);
        }
        Banner::Downloading { version, percent } => {
            items = items
                .push(label(match percent {
                    Some(pc) => format!("Downloading {APP_NAME} {version}… {pc}%"),
                    None => format!("Downloading {APP_NAME} {version}…"),
                }))
                .push(Space::new().width(Length::Fill))
                .push(action("Cancel", Ui::UpdateCancel, false));
        }
        Banner::Verifying(v) => {
            items = items.push(label(format!("Verifying {APP_NAME} {v}…")));
        }
        Banner::Installing(v) => {
            items = items.push(label(format!("Installing {APP_NAME} {v}…")));
        }
        Banner::Installed(v) => {
            items = items
                .push(label(format!(
                    "{APP_NAME} {v} is installed. Restart to finish."
                )))
                .push(action("Restart now", Ui::UpdateRestart, true))
                .push(action("Later", Ui::UpdateDismiss, false));
        }
        Banner::Failed(e) => {
            items = items
                .push(label(format!("Update failed: {e}")))
                .push(action("Open release page", Ui::UpdateReleaseNotes, true))
                .push(Space::new().width(Length::Fill))
                .push(close);
        }
    }
    Some(
        container(items)
            .width(Length::Fill)
            .height(theme::UPDATE_BAR_HEIGHT)
            .padding([0, 12])
            .align_y(Alignment::Center)
            .style(theme::update_bar(p))
            .into(),
    )
}

/// "Updates" in Settings: version, install method, check button and its
/// result, "Update now", release notes and the auto-check switch.
pub fn settings_section(app: &App) -> Element<'_, Message> {
    let p = app.theme.palette;
    let m = app.theme.metrics;
    let u = &app.updater;
    let method = u.method.as_ref().map_or("detecting…", |m| m.label());
    let small_button = |label: &'static str, msg: Option<Ui>, primary: bool| {
        let b = button(text(label).size(m.font_s))
            .padding([6, 12])
            .on_press_maybe(msg.map(ui));
        if primary {
            b.style(theme::primary_button(p))
        } else {
            b.style(theme::secondary_button(p))
        }
    };

    let mut buttons = row![].spacing(6).align_y(Alignment::Center);
    let can_check = !u.busy() && u.network && !matches!(u.phase, Phase::Installed(_));
    buttons = buttons.push(small_button(
        "Check for updates",
        can_check.then_some(Ui::UpdateCheckNow),
        false,
    ));
    match &u.phase {
        Phase::Installed(_) => {
            buttons = buttons.push(small_button("Restart now", Some(Ui::UpdateRestart), true));
        }
        Phase::Downloading { .. } => {
            buttons = buttons.push(small_button("Cancel", Some(Ui::UpdateCancel), false));
        }
        _ if u.latest.is_some() && !u.busy() => match u.action() {
            Action::SelfUpdate => {
                buttons = buttons.push(small_button(
                    "Update now",
                    u.network.then_some(Ui::UpdateInstall),
                    true,
                ));
            }
            Action::Cargo(_) => {
                buttons = buttons.push(small_button(
                    "Copy command",
                    Some(Ui::UpdateCopyCommand),
                    true,
                ));
            }
            Action::OpenPage => {}
        },
        _ => {}
    }
    buttons = buttons.push(
        button(text("Release notes").size(m.font_s))
            .padding([6, 4])
            .on_press(ui(Ui::UpdateReleaseNotes))
            .style(theme::link_button(p)),
    );

    let status = u.status();
    let mut col = column![
        text("Updates").size(m.font_m).color(p.text_primary),
        text(format!("Version {} · {method}", u.current))
            .size(m.font_xs)
            .color(p.text_secondary),
    ]
    .spacing(4);
    if !status.is_empty() {
        col = col.push(text(status).size(m.font_xs).color(match u.phase {
            Phase::CheckFailed(_) | Phase::InstallFailed(_) => p.danger,
            _ => p.text_secondary,
        }));
    }
    if let (Action::Cargo(cmd), Some(_)) = (u.action(), &u.latest) {
        col = col.push(text(cmd).size(m.font_xs).color(p.text_primary));
    }
    col = col.push(buttons).push(
        row![
            text("Check for updates automatically")
                .size(m.font_s)
                .color(p.text_primary)
                .width(Length::Fill),
            toggler(u.auto)
                .on_toggle(|b| ui(Ui::SetCheckUpdates(b)))
                .size(22)
                .style(theme::switch(p)),
        ]
        .spacing(12)
        .align_y(Alignment::Center),
    );
    col.into()
}
