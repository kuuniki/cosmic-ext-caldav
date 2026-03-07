mod app;
mod caldav;
mod config;
use cosmic::app::Settings;
fn main() -> cosmic::iced::Result {
    let settings = Settings::default()
        .size(cosmic::iced::Size::new(560.0, 505.0));
    cosmic::app::run::<app::App>(settings, ())
}
