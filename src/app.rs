use cosmic::app::{Core, Task};
use cosmic::iced::{Alignment, Length};
use cosmic::widget;
use cosmic::{Application, Element};
use chrono::Local;
use zeroize::Zeroize;

use crate::caldav::{CalDavClient, Calendar, CalendarEvent};
use crate::config::{Account, Config};

#[derive(Debug, Clone)]
pub enum Message {
    ViewAccounts,
    ViewAddAccount,
    ProviderSelected(String),
    ViewCalendars(String),
    ViewEvents(String, String),
    UrlChanged(String),
    UsernameChanged(String),
    PasswordChanged(String),
    TestConnection,
    SaveAccount,
    DeleteAccount(String),
    ConnectionResult(Result<(), String>),
    TestResult(Result<(), String>),
    CalendarsLoaded(Result<Vec<Calendar>, String>),
    EventsLoaded(Result<Vec<CalendarEvent>, String>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum View {
    Accounts,
    AddAccount,
    Calendars(String),
    Events(String, String),
}

pub struct App {
    core: Core,
    config: Config,
    view: View,
    url_input: String,
    provider: Option<String>,
    username_input: String,
    password_input: String,
    status_message: Option<String>,
    is_loading: bool,
    calendars: Vec<Calendar>,
    events: Vec<CalendarEvent>,
    current_calendar_name: String,
}

impl Application for App {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;
    const APP_ID: &'static str = "dev.cosmic.caldav";

    fn core(&self) -> &Core { &self.core }
    fn core_mut(&mut self) -> &mut Core { &mut self.core }

    fn init(core: Core, _flags: Self::Flags) -> (Self, Task<Self::Message>) {
        let app = Self {
            core,
            view: View::Accounts,
            config: Config::load(),
            url_input: String::new(),
            provider: None,
            username_input: String::new(),
            password_input: String::new(),
            status_message: None,
            is_loading: false,
            calendars: Vec::new(),
            events: Vec::new(),
            current_calendar_name: String::new(),
        };
        (app, Task::none())
    }

    fn header_center(&self) -> Vec<Element<'_, Self::Message>> {
        let title = match &self.view {
            View::Accounts => "",
            View::AddAccount => "",
            View::Calendars(_) => "Calendars",
            View::Events(_, _) => &self.current_calendar_name,
        };
        vec![widget::text::body(title).wrapping(cosmic::iced::widget::text::Wrapping::None).into()]
    }
    fn header_start(&self) -> Vec<Element<'_, Self::Message>> {
        // Balance the 3 window control buttons on the right so title is truly centered
        vec![widget::container(widget::text("")).width(Length::Fixed(120.0)).into()]
    }

    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        match message {
            Message::ViewAccounts => {
                self.view = View::Accounts;
                self.status_message = None;
            }
            Message::ViewAddAccount => {
                self.view = View::AddAccount;
                self.provider = None;
                self.url_input.clear();
                self.username_input.clear();
                // Zero memory before clearing so the password isn't left in the heap
                self.password_input.zeroize();
                self.password_input.clear();
                self.status_message = None;
            }
            Message::ViewCalendars(account_id) => {
                self.view = View::Calendars(account_id.clone());
                self.is_loading = true;
                self.status_message = None;
                if let Some(account) = self.config.accounts.iter().find(|a| a.id == account_id).cloned() {
                    let client = CalDavClient::new(
                        account.url.clone(),
                        account.username.clone(),
                        account.password.clone(),
                    );
                    return Task::perform(
                        async move { client.get_calendars().await },
                        move |result| cosmic::Action::App(Message::CalendarsLoaded(result)),
                    );
                }
                // Account not found — release the loading state so the UI doesn't lock.
                self.is_loading = false;
                self.status_message = Some("Account not found".into());
            }
            Message::ViewEvents(account_id, href) => {
                let cal_name = self.calendars.iter()
                    .find(|c| c.href == href)
                    .map(|c| c.display_name.clone())
                    .unwrap_or_else(|| "Events".to_string());
                self.current_calendar_name = cal_name;
                self.view = View::Events(account_id.clone(), href.clone());
                self.is_loading = true;
                self.status_message = None;
                if let Some(account) = self.config.accounts.iter().find(|a| a.id == account_id).cloned() {
                    let client = CalDavClient::new(
                        account.url.clone(),
                        account.username.clone(),
                        account.password.clone(),
                    );
                    let h = href.clone();
                    return Task::perform(
                        async move { client.get_events(&h).await },
                        move |result| cosmic::Action::App(Message::EventsLoaded(result)),
                    );
                }
                // Account not found — release the loading state so the UI doesn't lock.
                self.is_loading = false;
                self.status_message = Some("Account not found".into());
            }
            Message::ProviderSelected(p) => {
                match p.as_str() {
                    "nextcloud" => { self.url_input = String::new(); }
                    "google" => { self.url_input = format!("https://www.google.com/calendar/dav/{}/events/", self.username_input); }
                    "outlook" => { self.url_input = "https://outlook.office365.com/".to_string(); }
                    _ => {}
                }
                self.provider = Some(p);
            }
            Message::UrlChanged(v) => self.url_input = v,
            Message::UsernameChanged(v) => {
                self.username_input = v;
                // Keep Google URL in sync with username
                if self.provider.as_deref() == Some("google") {
                    self.url_input = format!("https://www.google.com/calendar/dav/{}/events/", self.username_input);
                }
            }
            Message::PasswordChanged(v) => self.password_input = v,
            Message::TestConnection => {
                self.is_loading = true;
                self.status_message = Some("Testing connection...".into());
                let client = CalDavClient::new(
                    self.url_input.clone(),
                    self.username_input.clone(),
                    self.password_input.clone(),
                );
                return Task::perform(
                    async move { client.test_connection().await },
                    |r| cosmic::Action::App(Message::TestResult(r)),
                );
            }
            Message::SaveAccount => {
                if self.url_input.is_empty() || self.username_input.is_empty() || self.password_input.is_empty() {
                    self.status_message = Some("Please fill in all fields".into());
                    return Task::none();
                }
                self.is_loading = true;
                self.status_message = Some("Verifying...".into());
                let client = CalDavClient::new(
                    self.url_input.clone(),
                    self.username_input.clone(),
                    self.password_input.clone(),
                );
                return Task::perform(
                    async move { client.test_connection().await },
                    |r| cosmic::Action::App(Message::ConnectionResult(r)),
                );
            }
            Message::TestResult(result) => {
                self.is_loading = false;
                match result {
                    Ok(()) => self.status_message = Some("✓ Connection successful!".into()),
                    Err(e) => {
                        // Log the full technical detail; show a safe summary to the user
                        eprintln!("Connection test error: {}", e);
                        self.status_message = Some(format!("✗ {}", e));
                    }
                }
            }
            Message::ConnectionResult(result) => {
                self.is_loading = false;
                match result {
                    Ok(()) => {
                        if self.view == View::AddAccount {
                            match self.config.add_account(
                                self.url_input.clone(),
                                self.username_input.clone(),
                                self.password_input.clone(),
                            ) {
                                Ok(()) => {
                                    // Zero the password from UI memory now that it is stored
                                    self.password_input.zeroize();
                                    self.password_input.clear();
                                    self.view = View::Accounts;
                                    self.status_message = None;
                                }
                                Err(e) => {
                                    self.status_message = Some(format!("✗ {}", e));
                                }
                            }
                        } else {
                            self.status_message = Some("✓ Connection successful!".into());
                        }
                    }
                    Err(e) => {
                        eprintln!("Connection error: {}", e);
                        self.status_message = Some(format!("✗ {}", e));
                    }
                }
            }
            Message::DeleteAccount(id) => {
                self.config.remove_account(&id);
            }
            Message::CalendarsLoaded(result) => {
                self.is_loading = false;
                match result {
                    Ok(cals) => self.calendars = cals,
                    Err(e) => self.status_message = Some(format!("Error: {}", e)),
                }
            }
            Message::EventsLoaded(result) => {
                self.is_loading = false;
                match result {
                    Ok(mut evts) => {
                        evts.sort_by(|a, b| a.start.cmp(&b.start));
                        self.events = evts;
                    }
                    Err(e) => self.status_message = Some(format!("Error: {}", e)),
                }
            }
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Self::Message> {
        match &self.view {
            View::Accounts => self.view_accounts(),
            View::AddAccount => self.view_add_account(),
            View::Calendars(id) => self.view_calendars(id),
            View::Events(id, href) => self.view_events(id, href),
        }
    }
}

impl App {
    fn view_accounts(&self) -> Element<'_, Message> {
        let mut col = widget::column::with_capacity(10)
            .spacing(12)
            .padding(24);
        if self.config.accounts.is_empty() {
            col = col.push(
                widget::container(
                    widget::column::with_children(vec![
                        widget::text("COSMIC CalDAV Applet").size(32).into(),
                        cosmic::iced::widget::Space::new().height(16).into(),
                        widget::text("Add an account to get started").size(16).into(),
                        cosmic::iced::widget::Space::new().height(8).into(),
                    ])
                    .spacing(4)
                    .align_x(Alignment::Center),
                )
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .width(Length::Fill)
                .height(Length::Fill),
            );
        } else {
            col = col.push(
                widget::container(widget::text("Accounts").size(28))
                    .center_x(Length::Fill).width(Length::Fill)
            );
            col = col.push(cosmic::iced::widget::Space::new().height(8));
            for account in &self.config.accounts {
                col = col.push(self.account_row(account));
            }
        }

        col = col.push(
            widget::container(
                widget::button::suggested("Add Calendar Account")
                    .on_press(Message::ViewAddAccount)
            )
            .center_x(Length::Fill)
            .width(Length::Fill)
        );

        widget::scrollable(col).into()
    }

    fn account_row<'a>(&self, account: &'a Account) -> Element<'a, Message> {
        let id = account.id.clone();
        widget::settings::item(
            &account.display_name,
            widget::row::with_children(vec![
                widget::button::standard("Calendars")
                    .on_press(Message::ViewCalendars(id.clone()))
                    .into(),
                widget::button::destructive("Remove")
                    .on_press(Message::DeleteAccount(id))
                    .into(),
            ])
            .spacing(8),
        )
        .into()
    }

    fn view_add_account(&self) -> Element<'_, Message> {
        let mut col = widget::column::with_capacity(12)
            .spacing(12)
            .padding([12, 24, 24, 24])
            .max_width(500);

        col = col.push(
            widget::container(widget::text("Add Calendar Account").size(28))
                .center_x(Length::Fill).width(Length::Fill)
        );

        let provider = self.provider.as_deref().unwrap_or("nextcloud");
        col = col.push(
            widget::container(
                widget::row::with_children(vec![
                    widget::button::standard("Nextcloud")
                        .on_press(Message::ProviderSelected("nextcloud".into()))
                        .into(),
                    widget::button::standard("Google")
                        .on_press(Message::ProviderSelected("google".into()))
                        .into(),
                    widget::button::standard("Outlook 365")
                        .on_press(Message::ProviderSelected("outlook".into()))
                        .into(),
                ]).spacing(8)
            ).center_x(Length::Fill).width(Length::Fill)
        );

        let help_text = match provider {
            "google" => "Google requires an App Password for CalDAV — your regular Google password won't work here. Create an App Password at myaccount.google.com/apppasswords and use your Gmail address as username. The CalDAV URL is auto-filled.",
            "outlook" => "Enter your Microsoft 365 email as username and your regular password. If you have 2FA enabled, use an App Password from account.microsoft.com/security. The CalDAV URL is auto-filled.",
            _ => "Enter your Nextcloud URL, username and password to sync. Your login details are stored locally. For extra security, use an App Password instead (Settings -> Security -> App passwords).",
        };
        col = col.push(widget::text::body(help_text));
        let url_placeholder = match provider {
            "google" => "https://www.google.com/calendar/dav/you@gmail.com/events/",
            "outlook" => "https://outlook.office365.com/",
            _ => "https://cloud.example.com",
        };
        col = col.push(
            widget::settings::item(
                "CalDAV URL",
                widget::text_input(url_placeholder, &self.url_input)
                    .on_input(Message::UrlChanged)
                    .width(Length::Fixed(260.0)),
            )
        );
        col = col.push(
            widget::settings::item(
                "Username",
                widget::text_input("Username", &self.username_input)
                    .on_input(Message::UsernameChanged)
                    .width(Length::Fixed(260.0)),
            )
        );
        col = col.push(
            widget::settings::item(
                "Password",
                widget::text_input("Password", &self.password_input)
                    .password()
                    .on_input(Message::PasswordChanged)
                    .on_submit(|_| Message::SaveAccount)
                    .width(Length::Fixed(260.0)),
            )
        );

        if let Some(msg) = &self.status_message {
            col = col.push(widget::text::body(msg.as_str()));
        }

        col = col.push(
            widget::container(
                widget::row::with_children(vec![
                    widget::button::standard("Cancel")
                        .on_press(Message::ViewAccounts)
                        .width(Length::Fill)
                        .into(),
                    widget::button::standard("Test Connection")
                        .on_press(Message::TestConnection)
                        .width(Length::Fill)
                        .into(),
                    widget::button::suggested("Save Account")
                        .on_press(Message::SaveAccount)
                        .width(Length::Fill)
                        .into(),
                ]).spacing(8)
            ).padding([8, 0, 0, 0]).center_x(Length::Fill).width(Length::Fill)
        );

        widget::container(col)
            .center_x(Length::Fill)
            .width(Length::Fill)
            .into()
    }

    fn view_calendars(&self, account_id: &str) -> Element<'_, Message> {
        let account_id = account_id.to_string();
        let mut col = widget::column::with_capacity(10)
            .spacing(8)
            .padding(24);

        col = col.push(
            widget::button::standard("← Back")
                .on_press(Message::ViewAccounts),
        );

        if self.is_loading {
            col = col.push(widget::text::body("Loading calendars..."));
        } else if self.calendars.is_empty() {
            col = col.push(widget::text::body("No calendars found"));
        } else {
            col = col.push(widget::text::title3("Calendars"));
            for cal in &self.calendars {
                let href = cal.href.clone();
                let aid = account_id.clone();
                col = col.push(
                    widget::settings::item(
                        &cal.display_name,
                        widget::button::standard("View Events")
                            .on_press(Message::ViewEvents(aid, href)),
                    )
                );
            }
        }

        if let Some(msg) = &self.status_message {
            col = col.push(widget::text::body(msg.as_str()));
        }

        widget::scrollable(col).into()
    }

    fn view_events(&self, account_id: &str, _href: &str) -> Element<'_, Message> {
        let mut col = widget::column::with_capacity(10)
            .spacing(8)
            .padding(24);

        col = col.push(
            widget::button::standard("← Back")
                .on_press(Message::ViewCalendars(account_id.to_string())),
        );
        col = col.push(widget::text::title3(&self.current_calendar_name));

        if self.is_loading {
            col = col.push(widget::text::body("Loading events..."));
        } else if self.events.is_empty() {
            col = col.push(widget::text::body("No events found"));
        } else {
            for event in &self.events {
                col = col.push(self.event_card(event));
            }
        }

        if let Some(msg) = &self.status_message {
            col = col.push(widget::text::body(msg.as_str()));
        }

        widget::scrollable(col).into()
    }

    fn event_card(&self, event: &CalendarEvent) -> Element<'_, Message> {
        let date_str = match event.start {
            Some(start_dt) => {
                let start = start_dt.with_timezone(&Local).format("%a, %d %b %Y  %H:%M");
                match event.end {
                    Some(end_dt) => format!("{} \u{2013} {}", start,
                        end_dt.with_timezone(&Local).format("%H:%M")),
                    None => start.to_string(),
                }
            }
            None => "No date".to_string(),
        };

        let loc_str = event.location.as_deref()
            .filter(|s| !s.is_empty())
            .map(|s| format!("📍 {}", s));

        let desc_str = event.description.as_deref()
            .filter(|s| !s.is_empty())
            .map(|s| {
                // Slice at a char boundary to avoid an extra heap allocation per render
                let cut = s.char_indices().nth(100).map(|(i, _)| i).unwrap_or(s.len());
                if cut < s.len() {
                    format!("{}…", &s[..cut])
                } else {
                    s.to_owned()
                }
            });

        let mut col = widget::column::with_capacity(4).spacing(4);
        col = col.push(widget::text::heading(event.summary.clone()));
        col = col.push(widget::text(date_str));
        if let Some(s) = loc_str {
            col = col.push(widget::text(s));
        }
        if let Some(s) = desc_str {
            col = col.push(widget::text(s));
        }

        widget::container(col)
            .padding(16)
            .width(Length::Fill)
            .into()
    }
}
