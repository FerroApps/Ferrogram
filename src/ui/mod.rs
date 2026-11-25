//! Views: pure functions `&App -> Element<Message>`.

pub mod call;
pub mod chat;
pub mod chatmenu;
pub mod folders;
pub mod format;
pub mod icons;
pub mod info;
pub mod login;
pub mod settings;
pub mod sidebar;
pub mod theme;
pub mod update;
pub mod widgets;

use iced::widget::{container, opaque, row, stack, text};
use iced::{Element, Length};

use crate::app::{App, Message};

pub fn view(app: &App) -> Element<'_, Message> {
    let content = if app.shows_main() {
        main_view(app)
    } else {
        login::view(app)
    };
    // The update bar sits above everything, like the official client's.
    match update::banner(app) {
        Some(bar) => iced::widget::column![bar, content].into(),
        None => content,
    }
}

fn main_view(app: &App) -> Element<'_, Message> {
    let p = app.theme.palette;
    let divider = container(iced::widget::Space::new().width(1).height(Length::Fill))
        .style(theme::fill(p.border));
    let mut main = row![sidebar::view(app), divider, chat::view(app)];
    if let Some(panel) = info::view(app) {
        let divider2 = container(iced::widget::Space::new().width(1).height(Length::Fill))
            .style(theme::fill(p.border));
        main = main.push(divider2).push(panel);
    }
    let main: Element<'_, Message> = match call::bar(app) {
        Some(bar) => iced::widget::column![bar, main].into(),
        None => main.into(),
    };
    let mut layers = stack![main].width(Length::Fill).height(Length::Fill);
    if let Some(toast) = &app.toast {
        layers = layers.push(
            container(
                container(text(toast.clone()).size(13).color(p.text_primary))
                    .padding([8, 14])
                    .style(theme::elevated(p)),
            )
            .align_bottom(Length::Fill)
            .center_x(Length::Fill)
            .padding(80),
        );
    }
    if let Some(viewer) = chat::media_viewer(app) {
        layers = layers.push(opaque(viewer));
    }
    if let Some(menu) = chatmenu::header_menu(app) {
        layers = layers.push(menu);
    }
    if let Some(dialog) = chatmenu::confirm_dialog(app) {
        layers = layers.push(dialog);
    }
    if app.settings_open {
        layers = layers.push(settings::view(app));
    }
    if app.folders.manager_open {
        layers = layers.push(folders::manager(app));
    }
    if let Some(editor) = folders::editor(app) {
        layers = layers.push(editor);
    }
    if let Some(panel) = call::panel(app) {
        layers = layers.push(panel);
    }
    layers.into()
}
