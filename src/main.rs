//! Ferrogram entry point.

use ferrogram::app::App;
use ferrogram::config::{APP_ID, Config};
use ferrogram::ui::theme::{FONT, INTER_REGULAR, INTER_SEMIBOLD, NOTO_COLOR_EMOJI};

/// One window at a time: every window id renders the same view.
fn view(app: &App, _window: iced::window::Id) -> iced::Element<'_, ferrogram::app::Message> {
    ferrogram::ui::view(app)
}

fn main() -> iced::Result {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "Ferrogram — unofficial Telegram client\n\n\
             USAGE: ferrogram [--mock] [--minimized] [--api-setup] [--install-desktop-entry]\n\
                    [--screenshot <screen> <out.png>]\n\n\
             screens: login-phone login-code login-password api-setup chats search reply media settings\n\
                      folders folder-manager folder-editor folder-picker chat-menu\n\
                      info info-files info-links info-music info-voice info-gifs\n\
                      update-banner update-settings\n\
             env: FERROGRAM_API_ID, FERROGRAM_API_HASH, FERRO_THEME=light, FERRO_TRACE_STARTUP=1,\n\
                  FERRO_UPDATE_FORCE=1 (update checks in debug/mock builds)"
        );
        return Ok(());
    }
    // Renderer choice (architecture §1): the CPU renderer (tiny-skia) keeps
    // idle RSS around 30 MB, while wgpu costs 100+ MB for its device and
    // pipelines. Default to tiny-skia; `FERRO_RENDERER=wgpu` or an explicit
    // `ICED_BACKEND` opts back into the GPU path.
    if std::env::var_os("ICED_BACKEND").is_none()
        && std::env::var("FERRO_RENDERER").as_deref() != Ok("wgpu")
    {
        // SAFETY: called at the top of `main`, before any thread is spawned.
        unsafe { std::env::set_var("ICED_BACKEND", "tiny-skia") };
    }
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!(
            "ferrogram {}\nbuilt-in Telegram API credentials: {}",
            env!("CARGO_PKG_VERSION"),
            if ferrogram::config::embedded_credentials().is_some() {
                "yes"
            } else {
                "no (development build)"
            }
        );
        return Ok(());
    }
    if args.iter().any(|a| a == "--install-desktop-entry") {
        let data_home = directories::BaseDirs::new().map(|d| d.data_dir().to_path_buf());
        let exe = std::env::current_exe();
        return match (data_home, exe) {
            (Some(home), Ok(exe)) => match ferrogram::desktop::install_desktop_entry(&home, &exe) {
                Ok((entry, icon)) => {
                    println!(
                        "installed {}\ninstalled {}",
                        entry.display(),
                        icon.display()
                    );
                    Ok(())
                }
                Err(e) => {
                    eprintln!("could not install desktop entry: {e}");
                    std::process::exit(1)
                }
            },
            _ => {
                eprintln!("could not determine the data directory or executable path");
                std::process::exit(1)
            }
        };
    }
    let config = Config::from_env(&args);
    ferrogram::logging::init((!config.mock).then(|| config.log_path()).as_deref());
    tracing::info!(
        mock = config.mock,
        "starting Ferrogram {}",
        env!("CARGO_PKG_VERSION")
    );

    // Single instance: a second launch shows the first one's window and
    // exits (two tray icons would be wrong). Mock mode has its own socket so
    // a demo never grabs the real instance; screenshots run unlocked.
    if config.screenshot.is_none() {
        let name = ferrogram::instance::socket_name(
            APP_ID,
            ferrogram::instance::current_uid(),
            config.mock,
        );
        let ask = if config.minimized {
            ferrogram::instance::Ask::Nothing
        } else {
            ferrogram::instance::Ask::Show
        };
        let sink: std::sync::Arc<dyn Fn(ferrogram::remote::Remote) + Send + Sync> =
            std::sync::Arc::new(ferrogram::remote::send);
        if ferrogram::instance::acquire(&name, ask, sink) == ferrogram::instance::Role::Secondary {
            eprintln!("Ferrogram is already running; asked it to show its window.");
            return Ok(());
        }
    }

    // A daemon keeps running with zero windows: that is what lets the window
    // be closed (freeing the renderer surfaces) while the app lives in the tray.
    let app = iced::daemon(move || App::boot(config.clone()), App::update, view)
        .title(|app: &App, _window: iced::window::Id| app.title())
        .theme(|app: &App, _window: iced::window::Id| Some(app.theme()))
        .subscription(App::subscription)
        .font(INTER_REGULAR)
        .font(INTER_SEMIBOLD)
        .default_font(FONT);
    // Colour emoji without a system font (Telegram Desktop ships its own set).
    if ferrogram::ui::widgets::needs_bundled_emoji_font() {
        app.font(NOTO_COLOR_EMOJI).run()
    } else {
        app.run()
    }
}
