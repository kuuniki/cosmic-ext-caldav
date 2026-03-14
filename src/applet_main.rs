mod applet;
mod caldav;
mod config;

fn main() -> cosmic::iced::Result {
    cosmic::applet::run::<applet::CalDavApplet>(())
}
