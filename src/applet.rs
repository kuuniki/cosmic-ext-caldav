use cosmic::app::Task;
use cosmic::iced::{
    platform_specific::shell::commands::popup::{destroy_popup, get_popup},
    Alignment, Length, Subscription,
};
use cosmic::iced_runtime::core::window;
use cosmic::widget::{self, Id, autosize, button, container, grid, text};
use cosmic::{Apply, Application, Element};
use chrono::{Datelike, Local, NaiveDate};
use std::time::Duration;

use crate::caldav::{CalDavClient, Calendar, CalendarEvent};
use crate::config::{Account, Config};

#[derive(Debug, Clone)]
pub enum Message {
    TogglePopup,
    PopupClosed(window::Id),
    Tick,
    SyncTick,
    /// Calendar list fetched for one account.  String = account id.
    CalendarsLoaded(String, Result<Vec<Calendar>, String>),
    /// Events fetched for one account.  String = account username (display label).
    EventsLoaded(String, Result<Vec<CalendarEvent>, String>),
    PrevMonth,
    NextMonth,
    SelectDay(u32),
    ToggleAddForm,
    FormTitleChanged(String),
    FormHourChanged(String),
    FormMinuteChanged(String),
    FormDurationChanged(String),
    FormLocationChanged(String),
    FormDescriptionChanged(String),
    FormReminderChanged(String),
    SubmitEvent,
    EventCreated(Result<(), String>),
}

pub struct CalDavApplet {
    core: cosmic::app::Core,
    popup: Option<window::Id>,
    now: chrono::DateTime<Local>,
    view_year: i32,
    view_month: u32,
    date_selected: NaiveDate,
    /// First account's calendars — used to determine the target for event creation.
    calendars: Vec<Calendar>,
    /// Events from all accounts, each tagged with the account username label.
    events: Vec<(String, CalendarEvent)>,
    config: Config,
    loading: bool,
    /// Number of in-flight calendar/event fetch tasks across all accounts.
    /// SyncTick is skipped while this is > 0 to prevent overlapping syncs.
    pending_syncs: usize,
    show_add_form: bool,
    form_title: String,
    form_hour: String,
    form_minute: String,
    form_duration: String,
    form_location: String,
    form_description: String,
    form_reminder: String,
    form_error: Option<String>,
    last_synced: Option<chrono::DateTime<Local>>,
}

impl Application for CalDavApplet {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;
    const APP_ID: &'static str = "dev.cosmic.applet.caldav";

    fn core(&self) -> &cosmic::app::Core { &self.core }
    fn core_mut(&mut self) -> &mut cosmic::app::Core { &mut self.core }

    fn init(core: cosmic::app::Core, _flags: ()) -> (Self, Task<Message>) {
        let now = Local::now();
        let today = NaiveDate::from_ymd_opt(now.year(), now.month(), now.day())
            .unwrap_or_default();
        let config = Config::load();
        let n_accounts = config.accounts.len();
        let accounts = config.accounts.clone();
        let app = Self {
            core,
            popup: None,
            view_year: now.year(),
            view_month: now.month(),
            date_selected: today,
            calendars: Vec::new(),
            events: Vec::new(),
            last_synced: None,
            config,
            loading: false,
            pending_syncs: n_accounts,
            show_add_form: false,
            form_title: String::new(),
            form_hour: String::new(),
            form_minute: String::new(),
            form_duration: String::new(),
            form_location: String::new(),
            form_description: String::new(),
            form_reminder: String::from("15"),
            form_error: None,
            now,
        };
        let init_task = spawn_sync_tasks(&accounts);
        (app, init_task)
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::TogglePopup => {
                if let Some(id) = self.popup.take() {
                    return destroy_popup(id);
                } else {
                    let new_id = window::Id::unique();
                    self.popup = Some(new_id);
                    let n = self.config.accounts.len();
                    let fetch_task = if n > 0 {
                        self.loading = true;
                        self.pending_syncs = n;
                        self.events.clear();
                        self.calendars.clear();
                        spawn_sync_tasks(&self.config.accounts)
                    } else {
                        self.loading = false;
                        Task::none()
                    };
                    let main_id = self.core.main_window_id().unwrap_or(window::Id::unique());
                    let popup_settings = self.core.applet.get_popup_settings(
                        main_id, new_id, None, None, None,
                    );
                    return Task::batch(vec![get_popup(popup_settings), fetch_task]);
                }
            }
            Message::PopupClosed(id) => {
                if self.popup == Some(id) { self.popup = None; }
            }
            Message::Tick => {
                self.now = Local::now();
            }
            Message::SyncTick => {
                // Skip if any sync task is still running.
                if self.popup.is_none() && self.pending_syncs == 0 {
                    let n = self.config.accounts.len();
                    if n > 0 {
                        self.pending_syncs = n;
                        self.events.clear();
                        self.calendars.clear();
                        return spawn_sync_tasks(&self.config.accounts);
                    }
                }
            }
            Message::CalendarsLoaded(account_id, Ok(cals)) => {
                // Store the first account's calendars so SubmitEvent knows
                // where to PUT new events.
                if self.config.accounts.first().map(|a| a.id == account_id).unwrap_or(false) {
                    self.calendars = cals.clone();
                }
                if let Some(cal) = cals.into_iter().next() {
                    if let Some(account) = self.config.accounts.iter()
                        .find(|a| a.id == account_id)
                        .cloned()
                    {
                        let label = account.username.clone();
                        let client = CalDavClient::new(
                            account.url, account.username, account.password,
                        );
                        let href = cal.href.clone();
                        return Task::perform(
                            async move { client.get_events(&href).await },
                            move |r| cosmic::Action::App(Message::EventsLoaded(label, r)),
                        );
                    }
                }
                // No calendar or matching account — release this sync slot.
                self.pending_syncs = self.pending_syncs.saturating_sub(1);
                if self.pending_syncs == 0 { self.loading = false; }
            }
            Message::CalendarsLoaded(account_id, Err(e)) => {
                eprintln!("CalendarsLoaded error for {}: {}", account_id, e);
                self.pending_syncs = self.pending_syncs.saturating_sub(1);
                if self.pending_syncs == 0 { self.loading = false; }
            }
            Message::EventsLoaded(label, Ok(events)) => {
                self.events.extend(events.into_iter().map(|e| (label.clone(), e)));
                self.pending_syncs = self.pending_syncs.saturating_sub(1);
                if self.pending_syncs == 0 {
                    self.loading = false;
                    self.last_synced = Some(Local::now());
                    // Sort merged events from all accounts by start time.
                    self.events.sort_by_key(|(_, e)| e.start);
                }
            }
            Message::EventsLoaded(label, Err(e)) => {
                eprintln!("EventsLoaded error for {}: {}", label, e);
                self.pending_syncs = self.pending_syncs.saturating_sub(1);
                if self.pending_syncs == 0 { self.loading = false; }
            }
            Message::PrevMonth => {
                if self.view_month == 1 { self.view_month = 12; self.view_year -= 1; }
                else { self.view_month -= 1; }
            }
            Message::NextMonth => {
                if self.view_month == 12 { self.view_month = 1; self.view_year += 1; }
                else { self.view_month += 1; }
            }
            Message::SelectDay(d) => {
                self.date_selected = NaiveDate::from_ymd_opt(self.view_year, self.view_month, d)
                    .unwrap_or(self.date_selected);
                self.show_add_form = false;
            }
            Message::ToggleAddForm => {
                self.show_add_form = !self.show_add_form;
                self.form_error = None;
            }
            Message::FormTitleChanged(s) => { self.form_title = s; }
            Message::FormHourChanged(s) => { self.form_hour = s; }
            Message::FormMinuteChanged(s) => { self.form_minute = s; }
            Message::FormDurationChanged(s) => { self.form_duration = s; }
            Message::FormLocationChanged(s) => { self.form_location = s; }
            Message::FormDescriptionChanged(s) => { self.form_description = s; }
            Message::FormReminderChanged(s) => { self.form_reminder = s; }
            Message::SubmitEvent => {
                // Clamp to valid ranges before passing to chrono / iCalendar.
                let hour = self.form_hour.parse::<u32>().unwrap_or(9).min(23);
                let minute = self.form_minute.parse::<u32>().unwrap_or(0).min(59);
                // Cap duration at 24 hours (1 440 minutes) to avoid malformed DTEND.
                let duration = self.form_duration.parse::<u32>().unwrap_or(60).min(1440);
                if self.form_title.trim().is_empty() {
                    self.form_error = Some("Title required".into());
                } else if let Some(account) = self.config.accounts.first().cloned() {
                    if let Some(cal) = self.calendars.first().cloned() {
                        let client = CalDavClient::new(
                            account.url, account.username, account.password,
                        );
                        let summary = self.form_title.clone();
                        let date = self.date_selected;
                        let href = cal.href.clone();
                        let location = self.form_location.clone();
                        let description = self.form_description.clone();
                        let reminder = self.form_reminder.parse::<i32>().unwrap_or(15);
                        return Task::perform(
                            async move {
                                client.create_event(
                                    &href, &summary, date, hour, minute,
                                    duration, &location, &description, reminder,
                                ).await
                            },
                            |r| cosmic::Action::App(Message::EventCreated(r)),
                        );
                    } else {
                        self.form_error = Some("No calendar found".into());
                    }
                } else {
                    self.form_error = Some("No account configured".into());
                }
            }
            Message::EventCreated(Ok(())) => {
                self.show_add_form = false;
                self.form_title = String::new();
                self.form_hour = String::new();
                self.form_minute = String::new();
                self.form_duration = String::new();
                self.form_location = String::new();
                self.form_description = String::new();
                self.form_reminder = String::from("15");
                self.form_error = None;
                // Reload events for all accounts so the new event appears.
                let n = self.config.accounts.len();
                if n > 0 {
                    self.pending_syncs = n;
                    self.events.clear();
                    self.calendars.clear();
                    return spawn_sync_tasks(&self.config.accounts);
                }
            }
            Message::EventCreated(Err(e)) => { self.form_error = Some(e); }
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        use std::sync::LazyLock;
        static AUTOSIZE_ID: LazyLock<Id> = LazyLock::new(|| Id::new("caldav-autosize"));
        let date_str = self.now.format("%a, %-d %b  %H:%M").to_string();
        let btn = button::custom(text::body(date_str))
            .padding([0, 8])
            .class(cosmic::theme::Button::AppletIcon)
            .on_press(Message::TogglePopup);
        autosize::autosize(btn, AUTOSIZE_ID.clone()).into()
    }

    fn view_window(&self, _id: window::Id) -> Element<'_, Message> {
        let today = NaiveDate::from_ymd_opt(self.now.year(), self.now.month(), self.now.day())
            .unwrap_or_default();

        let month_name = match self.view_month {
            1=>"January",2=>"February",3=>"March",4=>"April",
            5=>"May",6=>"June",7=>"July",8=>"August",
            9=>"September",10=>"October",11=>"November",12=>"December",
            _=>""
        };

        let month_controls = cosmic::iced::widget::row![
            button::icon(widget::icon::from_name("go-previous-symbolic"))
                .padding(8).on_press(Message::PrevMonth),
            button::icon(widget::icon::from_name("go-next-symbolic"))
                .padding(8).on_press(Message::NextMonth),
            button::icon(widget::icon::from_name("list-add-symbolic"))
                .padding(8).on_press(Message::ToggleAddForm),
        ].spacing(4);

        let header = cosmic::iced::widget::row![
            text(format!("{} {}", month_name, self.view_year)).size(16),
            container(text("")).width(Length::Fill),
            month_controls,
        ]
        .align_y(Alignment::Center)
        .padding([12, 20]);

        let calendar = self.calendar_grid(today);

        let content = cosmic::iced::widget::column![
            header,
            calendar.padding(cosmic::iced::Padding { top: 0.0, right: 12.0, bottom: 4.0, left: 12.0 }),
            widget::divider::horizontal::default(),
            self.events_list(),
            self.add_event_section(),
        ];

        self.core.applet.popup_container(container(content)).into()
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch(vec![
            cosmic::iced::time::every(Duration::from_secs(1))
                .map(|_| Message::Tick),
            cosmic::iced::time::every(Duration::from_secs(300))
                .map(|_| Message::SyncTick),
        ])
    }

    fn on_close_requested(&self, id: window::Id) -> Option<Message> {
        Some(Message::PopupClosed(id))
    }
}

/// Spawns one `get_calendars` task per account, all running concurrently.
/// Returns `Task::none()` when `accounts` is empty.
fn spawn_sync_tasks(accounts: &[Account]) -> Task<Message> {
    if accounts.is_empty() {
        return Task::none();
    }
    Task::batch(
        accounts.iter().cloned().map(|account| {
            let account_id = account.id.clone();
            let client = CalDavClient::new(account.url, account.username, account.password);
            Task::perform(
                async move { client.get_calendars().await },
                move |r| cosmic::Action::App(Message::CalendarsLoaded(account_id, r)),
            )
        }).collect::<Vec<_>>(),
    )
}

impl CalDavApplet {
    fn calendar_grid(&self, today: NaiveDate) -> widget::Grid<'_, Message> {
        let mut calendar = grid().width(Length::Fill);

        // First day of the month and what weekday it falls on (0=Sun)
        let first_of_month = NaiveDate::from_ymd_opt(self.view_year, self.view_month, 1)
            .unwrap_or_default();
        let first_weekday = first_of_month.weekday().num_days_from_sunday();
        let days_in_month = days_in_month(self.view_year, self.view_month);

        // Day-of-week headers: Sun Mon Tue Wed Thu Fri Sat
        let headers = ["Su","Mo","Tu","We","Th","Fr","Sa"];
        for h in headers {
            calendar = calendar.push(
                text::caption(h)
                    .apply(container)
                    .center_x(Length::Fixed(44.0))
            );
        }
        calendar = calendar.insert_row();

        // Only render rows needed for this month
        let total_cells = first_weekday + days_in_month;
        let total_rows = (total_cells + 6) / 7;
        let num_cells = total_rows * 7;
        let mut day: i32 = 1 - first_weekday as i32;
        for i in 0..num_cells {
            if i > 0 && i % 7 == 0 {
                calendar = calendar.insert_row();
            }
            let d = day;
            day += 1;
            if d < 1 || d as u32 > days_in_month {
                // Empty cell for days outside this month
                calendar = calendar.push(
                    container(text(""))
                        .width(Length::Fixed(44.0))
                        .height(Length::Fixed(44.0))
                );
            } else {
                let du = d as u32;
                let is_selected = self.date_selected ==
                    NaiveDate::from_ymd_opt(self.view_year, self.view_month, du).unwrap_or_default();
                let is_today = today ==
                    NaiveDate::from_ymd_opt(self.view_year, self.view_month, du).unwrap_or_default();
                let has_events = self.events_on_day(du);
                calendar = calendar.push(date_button(du, is_selected, is_today, has_events));
            }
        }
        calendar
    }

    fn events_list(&self) -> Element<'_, Message> {
        let day_events: Vec<(&str, &CalendarEvent)> = self.events.iter().filter_map(|(label, e)| {
            e.start.and_then(|dt| {
                let local = dt.with_timezone(&Local);
                if NaiveDate::from_ymd_opt(local.year(), local.month(), local.day())
                    == Some(self.date_selected)
                {
                    Some((label.as_str(), e))
                } else {
                    None
                }
            })
        }).collect();

        let mut col = widget::column::with_capacity(5).spacing(4).padding([8, 12]);

        if self.loading {
            col = col.push(text::body("Loading events..."));
        } else if self.config.accounts.is_empty() {
            col = col.push(text::body("No accounts configured"));
        } else if day_events.is_empty() {
            col = col.push(text::body("No events this day"));
        } else {
            for (label, event) in day_events {
                let time_str = event.start
                    .map(|dt| dt.with_timezone(&Local).format("%H:%M").to_string())
                    .unwrap_or_default();
                col = col.push(
                    cosmic::iced::widget::row![
                        text::body(time_str).width(Length::Fixed(42.0)),
                        text::body(format!("{}: {}", label, event.summary)),
                    ]
                    .spacing(4)
                    .align_y(Alignment::Center)
                );
            }
        }
        col.into()
    }

    fn events_on_day(&self, day: u32) -> bool {
        self.events.iter().any(|(_, e)| {
            e.start.map(|dt| {
                let local = dt.with_timezone(&Local);
                local.year() == self.view_year
                    && local.month() == self.view_month
                    && local.day() == day
            }).unwrap_or(false)
        })
    }

    fn add_event_section(&self) -> Element<'_, Message> {
        let date_label = self.date_selected.format("%-d %b %Y").to_string();
        if !self.show_add_form {
            return widget::column::with_capacity(0).into();
        }

        let mut col = widget::column::with_capacity(8).spacing(6).padding([4, 12, 8, 12]);
        col = col.push(text::body(format!("New event — {}", date_label)));

        // Title
        col = col.push(
            widget::text_input("Title *", &self.form_title)
                .on_input(Message::FormTitleChanged)
        );

        // Start time row
        col = col.push(text::caption("Start time"));
        col = col.push(
            cosmic::iced::widget::row![
                widget::text_input("HH", &self.form_hour)
                    .on_input(Message::FormHourChanged)
                    .width(Length::Fixed(52.0)),
                text::body(":"),
                widget::text_input("MM", &self.form_minute)
                    .on_input(Message::FormMinuteChanged)
                    .width(Length::Fixed(52.0)),
            ].spacing(4).align_y(cosmic::iced::Alignment::Center)
        );

        // Duration
        col = col.push(text::caption("Duration (minutes)"));
        col = col.push(
            widget::text_input("60", &self.form_duration)
                .on_input(Message::FormDurationChanged)
        );

        // Location
        col = col.push(text::caption("Location"));
        col = col.push(
            widget::text_input("Location", &self.form_location)
                .on_input(Message::FormLocationChanged)
        );

        // Description
        col = col.push(text::caption("Description"));
        col = col.push(
            widget::text_input("Notes", &self.form_description)
                .on_input(Message::FormDescriptionChanged)
        );

        // Reminder
        col = col.push(text::caption("Reminder (minutes before)"));
        col = col.push(
            widget::text_input("15", &self.form_reminder)
                .on_input(Message::FormReminderChanged)
        );

        if let Some(err) = &self.form_error {
            col = col.push(text::body(err.clone()));
        }

        col = col.push(
            container(
                button::custom(text::body("Save"))
                    .padding([6, 32])
                    .class(cosmic::theme::Button::Suggested)
                    .on_press(Message::SubmitEvent)
            )
            .center_x(Length::Fill)
        );

        col.into()
    }
}

fn date_button(day: u32, is_selected: bool, is_today: bool, has_events: bool) -> button::Button<'static, Message> {
    let style = if is_selected {
        cosmic::theme::Button::Suggested
    } else if is_today {
        cosmic::theme::Button::Standard
    } else {
        cosmic::theme::Button::Text
    };

    let label = if has_events {
        text::body(format!("{day}·"))
            .apply(container)
            .center(Length::Fill)
    } else {
        text::body(format!("{day}"))
            .apply(container)
            .center(Length::Fill)
    };
    button::custom(label)
    .class(style)
    .width(Length::Fixed(44.0))
    .height(Length::Fixed(44.0))
    .on_press(Message::SelectDay(day))
}

fn days_in_month(year: i32, month: u32) -> u32 {
    let next = if month == 12 {
        NaiveDate::from_ymd_opt(year + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1)
    };
    next.and_then(|d| d.pred_opt()).map(|d| d.day()).unwrap_or(30)
}
