
use chrono::Local;
use cosmic::app::{Core, Task};
use cosmic::iced::{Alignment, Length};
use cosmic::widget;
use cosmic::{Application, Element};
use zeroize::Zeroize;

use crate::caldav::{CalDavClient, Calendar, CalendarEvent};
use crate::config::{Account, Config};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Nextcloud,
    Google,
    Outlook,
}

impl Provider {
    fn help_text(self) -> &'static str {
        match self {
            Provider::Google => {
                "Google requires an App Password for CalDAV — your regular Google password \
                 will not work here. Create an App Password at \
                 myaccount.google.com/apppasswords and use your Gmail address as username. \
                 The CalDAV URL is auto-filled."
            }
            Provider::Outlook => {
                "Enter your Microsoft 365 email as username and your regular password. \
                 If you have 2FA enabled, use an App Password from \
                 account.microsoft.com/security. The CalDAV URL is auto-filled."
            }
            Provider::Nextcloud => {
                "Enter your Nextcloud URL, username and password to sync. Your login \
                 details are stored locally. For extra security, use an App Password \
                 instead (Settings -> Security -> App passwords)."
            }
        }
    }

    fn url_placeholder(self) -> &'static str {
        match self {
            Provider::Google => "https://www.google.com/calendar/dav/you@gmail.com/events/",
            Provider::Outlook => "https://outlook.office365.com/",
            Provider::Nextcloud => "https://cloud.example.com",
        }
    }

    fn default_url(self, username: &str) -> Option<String> {
        match self {
            Provider::Google => Some(format!(
                "https://www.google.com/calendar/dav/{}/events/",
                username
            )),
            Provider::Outlook => Some("https://outlook.office365.com/".to_string()),
            Provider::Nextcloud => None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    ViewAccounts,
    ViewAddAccount,
    ProviderSelected(Provider),
    ViewCalendars { account_id: String },
    ViewEvents {
        account_id: String,
        calendar_href: String,
    },
    UrlChanged(String),
    UsernameChanged(String),
    PasswordChanged(String),
    TestConnection,
    SaveAccount,
    DeleteAccount(String),
    ConnectionTestFinished(Result<(), String>),
    AccountSaveVerified(Result<(), String>),
    CalendarsLoaded {
        account_id: String,
        result: Result<Vec<Calendar>, String>,
    },
    EventsLoaded {
        account_id: String,
        calendar_href: String,
        result: Result<Vec<CalendarEvent>, String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum View {
    Accounts,
    AddAccount,
    Calendars { account_id: String },
    Events {
        account_id: String,
        calendar_href: String,
    },
}

#[derive(Debug, Clone)]
struct AccountForm {
    provider: Provider,
    url: String,
    username: String,
    password: String,
}

impl Default for AccountForm {
    fn default() -> Self {
        Self {
            provider: Provider::Nextcloud,
            url: String::new(),
            username: String::new(),
            password: String::new(),
        }
    }
}

impl Drop for AccountForm {
    fn drop(&mut self) {
        self.password.zeroize();
    }
}

impl AccountForm {
    fn reset(&mut self) {
        self.provider = Provider::Nextcloud;
        self.url.clear();
        self.username.clear();
        self.password.zeroize();
        self.password.clear();
    }

    fn select_provider(&mut self, provider: Provider) {
        self.provider = provider;
        if let Some(url) = provider.default_url(&self.username) {
            self.url = url;
        } else {
            self.url.clear();
        }
    }

    fn set_username(&mut self, username: String) {
        self.username = username;
        if let Some(url) = self.provider.default_url(&self.username) {
            self.url = url;
        }
    }

    fn is_complete(&self) -> bool {
        !self.url.is_empty() && !self.username.is_empty() && !self.password.is_empty()
    }

    fn into_client(&self) -> CalDavClient {
        CalDavClient::new(
            self.url.clone(),
            self.username.clone(),
            self.password.clone(),
        )
    }
}

pub struct App {
    core: Core,
    config: Config,
    view: View,
    form: AccountForm,
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

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, _flags: Self::Flags) -> (Self, Task<Self::Message>) {
        (
            Self {
                core,
                config: Config::load(),
                view: View::Accounts,
                form: AccountForm::default(),
                status_message: None,
                is_loading: false,
                calendars: Vec::new(),
                events: Vec::new(),
                current_calendar_name: String::new(),
            },
            Task::none(),
        )
    }

    fn header_center(&self) -> Vec<Element<'_, Self::Message>> {
        let title = match &self.view {
            View::Accounts | View::AddAccount => "",
            View::Calendars { .. } => "Calendars",
            View::Events { .. } => &self.current_calendar_name,
        };

        vec![
            widget::text::body(title)
                .wrapping(cosmic::iced::widget::text::Wrapping::None)
                .into(),
        ]
    }

    fn header_start(&self) -> Vec<Element<'_, Self::Message>> {
        vec![
            widget::container(widget::text(""))
                .width(Length::Fixed(120.0))
                .into(),
        ]
    }

    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        match message {
            Message::ViewAccounts => self.show_accounts_view(),
            Message::ViewAddAccount => self.show_add_account_view(),
            Message::ProviderSelected(provider) => self.form.select_provider(provider),
            Message::ViewCalendars { account_id } => {
                return self.begin_calendar_load(account_id);
            }
            Message::ViewEvents {
                account_id,
                calendar_href,
            } => {
                return self.begin_event_load(account_id, calendar_href);
            }
            Message::UrlChanged(url) => self.form.url = url,
            Message::UsernameChanged(username) => self.form.set_username(username),
            Message::PasswordChanged(password) => self.form.password = password,
            Message::TestConnection => {
                return self.begin_connection_test();
            }
            Message::SaveAccount => {
                return self.begin_account_save();
            }
            Message::DeleteAccount(id) => {
                self.config.remove_account(&id);
            }
            Message::ConnectionTestFinished(result) => {
                self.handle_connection_test_result(result);
            }
            Message::AccountSaveVerified(result) => {
                self.handle_account_save_verified(result);
            }
            Message::CalendarsLoaded { account_id, result } => {
                self.handle_calendars_loaded(&account_id, result);
            }
            Message::EventsLoaded {
                account_id,
                calendar_href,
                result,
            } => {
                self.handle_events_loaded(&account_id, &calendar_href, result);
            }
        }

        Task::none()
    }

    fn view(&self) -> Element<'_, Self::Message> {
        match &self.view {
            View::Accounts => self.view_accounts(),
            View::AddAccount => self.view_add_account(),
            View::Calendars { account_id } => self.view_calendars(account_id),
            View::Events { account_id, calendar_href } => {
                self.view_events(account_id, calendar_href)
            }
        }
    }
}

impl App {
    fn show_accounts_view(&mut self) {
        self.view = View::Accounts;
        self.status_message = None;
    }

    fn show_add_account_view(&mut self) {
        self.view = View::AddAccount;
        self.form.reset();
        self.status_message = None;
    }

    fn begin_calendar_load(&mut self, account_id: String) -> Task<Message> {
        self.view = View::Calendars {
            account_id: account_id.clone(),
        };
        self.calendars.clear();
        self.is_loading = true;
        self.status_message = None;

        let Some(account) = self.account_by_id(&account_id).cloned() else {
            self.is_loading = false;
            self.status_message = Some("Account not found".into());
            return Task::none();
        };

        let client = self.client_for_account(&account);
        Task::perform(
            async move { client.get_calendars().await },
            move |result| {
                cosmic::Action::App(Message::CalendarsLoaded {
                    account_id,
                    result,
                })
            },
        )
    }

    fn begin_event_load(&mut self, account_id: String, calendar_href: String) -> Task<Message> {
        self.current_calendar_name = self
            .calendars
            .iter()
            .find(|calendar| calendar.href == calendar_href)
            .map(|calendar| calendar.display_name.clone())
            .unwrap_or_else(|| "Events".to_string());

        self.view = View::Events {
            account_id: account_id.clone(),
            calendar_href: calendar_href.clone(),
        };
        self.events.clear();
        self.is_loading = true;
        self.status_message = None;

        let Some(account) = self.account_by_id(&account_id).cloned() else {
            self.is_loading = false;
            self.status_message = Some("Account not found".into());
            return Task::none();
        };

        let client = self.client_for_account(&account);
        let request_calendar_href = calendar_href.clone();

        Task::perform(
            async move { client.get_events(&request_calendar_href).await },
            move |result| {
                cosmic::Action::App(Message::EventsLoaded {
                    account_id,
                    calendar_href,
                    result,
                })
            },
        )
    }

    fn begin_connection_test(&mut self) -> Task<Message> {
        self.is_loading = true;
        self.status_message = Some("Testing connection...".into());

        let client = self.form.into_client();
        Task::perform(
            async move { client.test_connection().await },
            |result| cosmic::Action::App(Message::ConnectionTestFinished(result)),
        )
    }

    fn begin_account_save(&mut self) -> Task<Message> {
        if !self.form.is_complete() {
            self.status_message = Some("Please fill in all fields".into());
            return Task::none();
        }

        self.is_loading = true;
        self.status_message = Some("Verifying...".into());

        let client = self.form.into_client();
        Task::perform(
            async move { client.test_connection().await },
            |result| cosmic::Action::App(Message::AccountSaveVerified(result)),
        )
    }

    fn handle_connection_test_result(&mut self, result: Result<(), String>) {
        self.is_loading = false;
        self.status_message = Some(match result {
            Ok(()) => "✓ Connection successful!".into(),
            Err(error) => {
                eprintln!("Connection test error: {}", error);
                format!("✗ {}", error)
            }
        });
    }

    fn handle_account_save_verified(&mut self, result: Result<(), String>) {
        self.is_loading = false;

        match result {
            Ok(()) => match self.config.add_account(
                self.form.url.clone(),
                self.form.username.clone(),
                self.form.password.clone(),
            ) {
                Ok(()) => {
                    self.form.password.zeroize();
                    self.form.password.clear();
                    self.view = View::Accounts;
                    self.status_message = None;
                }
                Err(error) => {
                    self.status_message = Some(format!("✗ {}", error));
                }
            },
            Err(error) => {
                eprintln!("Connection error: {}", error);
                self.status_message = Some(format!("✗ {}", error));
            }
        }
    }

    fn handle_calendars_loaded(
        &mut self,
        account_id: &str,
        result: Result<Vec<Calendar>, String>,
    ) {
        self.is_loading = false;

        if !matches!(
            &self.view,
            View::Calendars {
                account_id: current_id
            } if current_id == account_id
        ) {
            return;
        }

        match result {
            Ok(calendars) => {
                self.calendars = calendars;
                self.status_message = None;
            }
            Err(error) => {
                self.calendars.clear();
                self.status_message = Some(format!("Error: {}", error));
            }
        }
    }

    fn handle_events_loaded(
        &mut self,
        account_id: &str,
        calendar_href: &str,
        result: Result<Vec<CalendarEvent>, String>,
    ) {
        self.is_loading = false;

        if !matches!(
            &self.view,
            View::Events {
                account_id: current_id,
                calendar_href: current_href,
            } if current_id == account_id && current_href == calendar_href
        ) {
            return;
        }

        match result {
            Ok(mut events) => {
                events.sort_by(|left, right| left.start.cmp(&right.start));
                self.events = events;
                self.status_message = None;
            }
            Err(error) => {
                self.events.clear();
                self.status_message = Some(format!("Error: {}", error));
            }
        }
    }

    fn account_by_id(&self, account_id: &str) -> Option<&Account> {
        self.config.accounts.iter().find(|account| account.id == account_id)
    }

    fn client_for_account(&self, account: &Account) -> CalDavClient {
        CalDavClient::new(
            account.url.clone(),
            account.username.clone(),
            account.password.clone(),
        )
    }

    fn view_accounts(&self) -> Element<'_, Message> {
        let mut column = widget::column::with_capacity(10).spacing(12).padding(24);

        if self.config.accounts.is_empty() {
            column = column.push(
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
            column = column.push(
                widget::container(widget::text("Accounts").size(28))
                    .center_x(Length::Fill)
                    .width(Length::Fill),
            );
            column = column.push(cosmic::iced::widget::Space::new().height(8));

            for account in &self.config.accounts {
                column = column.push(self.account_row(account));
            }
        }

        column = column.push(
            widget::container(
                widget::button::suggested("Add Calendar Account")
                    .on_press(Message::ViewAddAccount),
            )
            .center_x(Length::Fill)
            .width(Length::Fill),
        );

        widget::scrollable(column).into()
    }

    fn account_row<'a>(&self, account: &'a Account) -> Element<'a, Message> {
        let account_id = account.id.clone();

        widget::settings::item(
            &account.display_name,
            widget::row::with_children(vec![
                widget::button::standard("Calendars")
                    .on_press(Message::ViewCalendars {
                        account_id: account_id.clone(),
                    })
                    .into(),
                widget::button::destructive("Remove")
                    .on_press(Message::DeleteAccount(account_id))
                    .into(),
            ])
            .spacing(8),
        )
        .into()
    }

    fn view_add_account(&self) -> Element<'_, Message> {
        let mut column = widget::column::with_capacity(12)
            .spacing(12)
            .padding([12, 24, 24, 24])
            .max_width(500);

        column = column.push(
            widget::container(widget::text("Add Calendar Account").size(28))
                .center_x(Length::Fill)
                .width(Length::Fill),
        );

        column = column.push(
            widget::container(
                widget::row::with_children(vec![
                    widget::button::standard("Nextcloud")
                        .on_press(Message::ProviderSelected(Provider::Nextcloud))
                        .into(),
                    widget::button::standard("Google")
                        .on_press(Message::ProviderSelected(Provider::Google))
                        .into(),
                    widget::button::standard("Outlook 365")
                        .on_press(Message::ProviderSelected(Provider::Outlook))
                        .into(),
                ])
                .spacing(8),
            )
            .center_x(Length::Fill)
            .width(Length::Fill),
        );

        column = column.push(widget::text::body(self.form.provider.help_text()));

        column = column.push(widget::settings::item(
            "CalDAV URL",
            widget::text_input(self.form.provider.url_placeholder(), &self.form.url)
                .on_input(Message::UrlChanged)
                .width(Length::Fixed(260.0)),
        ));

        column = column.push(widget::settings::item(
            "Username",
            widget::text_input("Username", &self.form.username)
                .on_input(Message::UsernameChanged)
                .width(Length::Fixed(260.0)),
        ));

        column = column.push(widget::settings::item(
            "Password",
            widget::text_input("Password", &self.form.password)
                .password()
                .on_input(Message::PasswordChanged)
                .on_submit(|_| Message::SaveAccount)
                .width(Length::Fixed(260.0)),
        ));

        if let Some(message) = &self.status_message {
            column = column.push(widget::text::body(message.as_str()));
        }

        column = column.push(
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
                ])
                .spacing(8),
            )
            .padding([8, 0, 0, 0])
            .center_x(Length::Fill)
            .width(Length::Fill),
        );

        widget::container(column)
            .center_x(Length::Fill)
            .width(Length::Fill)
            .into()
    }

    fn view_calendars(&self, account_id: &str) -> Element<'_, Message> {
        let account_id = account_id.to_string();
        let mut column = widget::column::with_capacity(10).spacing(8).padding(24);

        column = column.push(
            widget::button::standard("← Back").on_press(Message::ViewAccounts),
        );

        if self.is_loading {
            column = column.push(widget::text::body("Loading calendars..."));
        } else if self.calendars.is_empty() {
            column = column.push(widget::text::body("No calendars found"));
        } else {
            column = column.push(widget::text::title3("Calendars"));

            for calendar in &self.calendars {
                let calendar_href = calendar.href.clone();
                let account_id = account_id.clone();

                column = column.push(widget::settings::item(
                    &calendar.display_name,
                    widget::button::standard("View Events").on_press(Message::ViewEvents {
                        account_id,
                        calendar_href,
                    }),
                ));
            }
        }

        if let Some(message) = &self.status_message {
            column = column.push(widget::text::body(message.as_str()));
        }

        widget::scrollable(column).into()
    }

    fn view_events(&self, account_id: &str, _calendar_href: &str) -> Element<'_, Message> {
        let mut column = widget::column::with_capacity(10).spacing(8).padding(24);

        column = column.push(
            widget::button::standard("← Back").on_press(Message::ViewCalendars {
                account_id: account_id.to_string(),
            }),
        );
        column = column.push(widget::text::title3(&self.current_calendar_name));

        if self.is_loading {
            column = column.push(widget::text::body("Loading events..."));
        } else if self.events.is_empty() {
            column = column.push(widget::text::body("No events found"));
        } else {
            for event in &self.events {
                column = column.push(self.event_card(event));
            }
        }

        if let Some(message) = &self.status_message {
            column = column.push(widget::text::body(message.as_str()));
        }

        widget::scrollable(column).into()
    }

    fn event_card(&self, event: &CalendarEvent) -> Element<'_, Message> {
        let date_str = match event.start {
            Some(start_dt) => {
                let start = start_dt
                    .with_timezone(&Local)
                    .format("%a, %d %b %Y  %H:%M");
                match event.end {
                    Some(end_dt) => format!(
                        "{} \u{2013} {}",
                        start,
                        end_dt.with_timezone(&Local).format("%H:%M")
                    ),
                    None => start.to_string(),
                }
            }
            None => "No date".to_string(),
        };

        let location = event
            .location
            .as_deref()
            .filter(|value| !value.is_empty())
            .map(|value| format!("📍 {}", value));

        let description = event
            .description
            .as_deref()
            .filter(|value| !value.is_empty())
            .map(|value| {
                let cut = value
                    .char_indices()
                    .nth(100)
                    .map(|(index, _)| index)
                    .unwrap_or(value.len());

                if cut < value.len() {
                    format!("{}…", &value[..cut])
                } else {
                    value.to_owned()
                }
            });

        let mut column = widget::column::with_capacity(4).spacing(4);
        column = column.push(widget::text::heading(event.summary.clone()));
        column = column.push(widget::text(date_str));

        if let Some(location) = location {
            column = column.push(widget::text(location));
        }

        if let Some(description) = description {
            column = column.push(widget::text(description));
        }

        widget::container(column)
            .padding(16)
            .width(Length::Fill)
            .into()
    }
}
