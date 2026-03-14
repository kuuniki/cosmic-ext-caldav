use chrono::{Datelike, Local, NaiveDate, NaiveDateTime};
use cosmic::app::Task;
use cosmic::iced::{
    platform_specific::shell::commands::popup::{destroy_popup, get_popup},
    Alignment, Length, Subscription,
};
use cosmic::iced_runtime::core::window;
use cosmic::widget::{self, autosize, button, container, grid, text, Id};
use cosmic::{Application, Apply, Element};
use std::time::Duration;

use crate::caldav::{CalDavClient, Calendar, CalendarEvent};
use crate::config::{Account, Config};

#[derive(Debug, Clone)]
pub enum Message {
    TogglePopup,
    PopupClosed(window::Id),
    Tick,
    SyncTick,
    /// Calendar list fetched for one account.
    /// String = account id.
    CalendarsLoaded(String, Result<Vec<Calendar>, String>),
    /// Events fetched for one account.
    /// First String = account id, second String = display label.
    EventsLoaded(String, String, Result<Vec<CalendarEvent>, String>),
    PrevMonth,
    NextMonth,
    SelectDay(u32),
    EditEvent(usize),
    ToggleAddForm,
    CancelForm,
    FormTitleChanged(String),
    FormStartDateChanged(String),
    FormStartTimeChanged(String),
    FormEndDateChanged(String),
    FormEndTimeChanged(String),
    FormLocationChanged(String),
    FormDescriptionChanged(String),
    FormReminderChanged(String),
    SubmitEvent,
    DeleteEditingEvent,
    EventCreated(Result<(), String>),
    EventUpdated(Result<(), String>),
    EventDeleted(Result<(), String>),
}

#[derive(Debug, Clone)]
struct AppletEvent {
    account_id: String,
    label: String,
    event: CalendarEvent,
}

#[derive(Debug, Clone)]
struct EditingEvent {
    account_id: String,
    event_href: String,
    event_etag: Option<String>,
    uid: String,
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
    /// Events from all accounts with enough metadata to edit/delete them.
    events: Vec<AppletEvent>,
    config: Config,
    loading: bool,
    /// Number of in-flight calendar/event fetch tasks across all accounts.
    /// SyncTick is skipped while this is > 0 to prevent overlapping syncs.
    pending_syncs: usize,
    show_add_form: bool,
    form_title: String,
    form_start_date: String,
    form_start_time: String,
    form_end_date: String,
    form_end_time: String,
    form_location: String,
    form_description: String,
    form_reminder: String,
    form_error: Option<String>,
    creating_event: bool,
    editing_event: Option<EditingEvent>,
}

impl Application for CalDavApplet {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;

    const APP_ID: &'static str = "dev.cosmic.applet.caldav";

    fn core(&self) -> &cosmic::app::Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut cosmic::app::Core {
        &mut self.core
    }

    fn init(core: cosmic::app::Core, _flags: ()) -> (Self, Task<Message>) {
        let now = Local::now();
        let today = NaiveDate::from_ymd_opt(now.year(), now.month(), now.day()).unwrap_or_default();
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
            config,
            loading: false,
            pending_syncs: n_accounts,
            show_add_form: false,
            form_title: String::new(),
            form_start_date: today.format("%Y-%m-%d").to_string(),
            form_start_time: String::from("09:00"),
            form_end_date: today.format("%Y-%m-%d").to_string(),
            form_end_time: String::from("10:00"),
            form_location: String::new(),
            form_description: String::new(),
            form_reminder: String::from("15"),
            form_error: None,
            creating_event: false,
            editing_event: None,
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
                let popup_settings = self
                    .core
                    .applet
                    .get_popup_settings(main_id, new_id, None, None, None);
                return Task::batch(vec![get_popup(popup_settings), fetch_task]);
            }
        }
        Message::PopupClosed(id) => {
            if self.popup == Some(id) {
                self.popup = None;
            }
        }
        Message::Tick => {
            self.now = Local::now();
        }
        Message::SyncTick => {
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
            if self
                .config
                .accounts
                .first()
                .map(|a| a.id == account_id)
                .unwrap_or(false)
            {
                self.calendars = cals.clone();
            }

            if let Some(cal) = cals.into_iter().next() {
                if let Some(account) = self
                    .config
                    .accounts
                    .iter()
                    .find(|a| a.id == account_id)
                    .cloned()
                {
                    let label = account.username.clone();
                    let account_id = account.id.clone();
                    let client = CalDavClient::new(
                        account.url.clone(),
                        account.username.clone(),
                        account.password.clone(),
                    );
                    let href = cal.href.clone();
                    return Task::perform(
                        async move { client.get_events(&href).await },
                        move |r| cosmic::Action::App(Message::EventsLoaded(account_id, label, r)),
                    );
                }
            }

            self.complete_sync_slot();
        }
        Message::CalendarsLoaded(account_id, Err(e)) => {
            eprintln!("CalendarsLoaded error for {}: {}", account_id, e);
            self.complete_sync_slot();
        }
        Message::EventsLoaded(account_id, label, Ok(events)) => {
            self.events.extend(events.into_iter().map(|event| AppletEvent {
                account_id: account_id.clone(),
                label: label.clone(),
                event,
            }));
            self.complete_sync_slot();
            if self.pending_syncs == 0 {
                self.events.sort_by_key(|item| item.event.start);
            }
        }
        Message::EventsLoaded(account_id, label, Err(e)) => {
            eprintln!("EventsLoaded error for {}/{}: {}", account_id, label, e);
            self.complete_sync_slot();
        }
        Message::PrevMonth => {
            if self.view_month == 1 {
                self.view_month = 12;
                self.view_year -= 1;
            } else {
                self.view_month -= 1;
            }
        }
        Message::NextMonth => {
            if self.view_month == 12 {
                self.view_month = 1;
                self.view_year += 1;
            } else {
                self.view_month += 1;
            }
        }
        Message::SelectDay(d) => {
            self.date_selected =
                NaiveDate::from_ymd_opt(self.view_year, self.view_month, d).unwrap_or(self.date_selected);
            self.show_add_form = false;
            self.editing_event = None;
            self.form_error = None;
        }
        Message::EditEvent(index) => {
            if let Some(item) = self.events.get(index).cloned() {
                self.begin_edit(item);
            }
        }
        Message::ToggleAddForm => {
            if self.show_add_form && self.editing_event.is_none() {
                self.show_add_form = false;
                self.form_error = None;
            } else {
                self.show_add_form = true;
                self.editing_event = None;
                self.reset_form();
                self.form_start_date = self.date_selected.format("%Y-%m-%d").to_string();
                self.form_end_date = self.date_selected.format("%Y-%m-%d").to_string();
                self.form_error = None;
            }
        }
        Message::CancelForm => {
            self.show_add_form = false;
            self.editing_event = None;
            self.creating_event = false;
            self.form_error = None;
            self.reset_form();
        }
        Message::FormTitleChanged(s) => self.form_title = s,
        Message::FormStartDateChanged(s) => self.form_start_date = s,
        Message::FormStartTimeChanged(s) => self.form_start_time = s,
        Message::FormEndDateChanged(s) => self.form_end_date = s,
        Message::FormEndTimeChanged(s) => self.form_end_time = s,
        Message::FormLocationChanged(s) => self.form_location = s,
        Message::FormDescriptionChanged(s) => self.form_description = s,
        Message::FormReminderChanged(s) => self.form_reminder = s,
        Message::SubmitEvent => {
            if self.creating_event {
                return Task::none();
            }
            if self.form_title.trim().is_empty() {
                self.form_error = Some("Title required".into());
            } else {
                let start = parse_local_datetime(&self.form_start_date, &self.form_start_time);
                let end = parse_local_datetime(&self.form_end_date, &self.form_end_time);
                match (start, end) {
                    (Err(e), _) | (_, Err(e)) => {
                        self.form_error = Some(e);
                    }
                    (Ok(start), Ok(end)) if end <= start => {
                        self.form_error = Some("End date/time must be after start date/time".into());
                    }
                    (Ok(start), Ok(end)) => {
                        let summary = self.form_title.clone();
                        let location = self.form_location.clone();
                        let description = self.form_description.clone();
                        let reminder = self.form_reminder.parse::<i32>().unwrap_or(15);

                        if let Some(edit) = self.editing_event.clone() {
                            if let Some(account) = self
                                .config
                                .accounts
                                .iter()
                                .find(|a| a.id == edit.account_id)
                                .cloned()
                            {
                                let client = CalDavClient::new(
                                    account.url.clone(),
                                    account.username.clone(),
                                    account.password.clone(),
                                );
                                self.creating_event = true;
                                self.form_error = None;
                                return Task::perform(
                                    async move {
                                        client
                                            .update_event(
                                                &edit.event_href,
                                                edit.event_etag.as_deref(),
                                                &edit.uid,
                                                &summary,
                                                start,
                                                end,
                                                &location,
                                                &description,
                                                reminder,
                                            )
                                            .await
                                    },
                                    |r| cosmic::Action::App(Message::EventUpdated(r)),
                                );
                            } else {
                                self.form_error = Some("Account not found for this event".into());
                            }
                        } else if let Some(account) = self.config.accounts.first().cloned() {
                            if let Some(cal) = self.calendars.first().cloned() {
                                let client = CalDavClient::new(
                                    account.url.clone(),
                                    account.username.clone(),
                                    account.password.clone(),
                                );
                                let href = cal.href.clone();
                                self.creating_event = true;
                                self.form_error = None;
                                return Task::perform(
                                    async move {
                                        client
                                            .create_event(
                                                &href,
                                                &summary,
                                                start,
                                                end,
                                                &location,
                                                &description,
                                                reminder,
                                            )
                                            .await
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
                }
            }
        }
        Message::DeleteEditingEvent => {
            if self.creating_event {
                return Task::none();
            }
            if let Some(edit) = self.editing_event.clone() {
                if let Some(account) = self
                    .config
                    .accounts
                    .iter()
                    .find(|a| a.id == edit.account_id)
                    .cloned()
                {
                    let client = CalDavClient::new(
                        account.url.clone(),
                        account.username.clone(),
                        account.password.clone(),
                    );
                    self.creating_event = true;
                    self.form_error = None;
                    return Task::perform(
                        async move { client.delete_event(&edit.event_href, edit.event_etag.as_deref()).await },
                        |r| cosmic::Action::App(Message::EventDeleted(r)),
                    );
                } else {
                    self.form_error = Some("Account not found for this event".into());
                }
            }
        }
        Message::EventCreated(Ok(())) | Message::EventUpdated(Ok(())) | Message::EventDeleted(Ok(())) => {
            self.creating_event = false;
            self.show_add_form = false;
            self.editing_event = None;
            self.reset_form();

            let n = self.config.accounts.len();
            if n > 0 {
                self.pending_syncs = n;
                self.events.clear();
                self.calendars.clear();
                return spawn_sync_tasks(&self.config.accounts);
            }
        }
        Message::EventCreated(Err(e)) | Message::EventUpdated(Err(e)) | Message::EventDeleted(Err(e)) => {
            self.creating_event = false;
            self.form_error = Some(e);
        }
    }

    Task::none()
}

    fn view(&self) -> Element<'_, Message> {
        use std::sync::LazyLock;

        static AUTOSIZE_ID: LazyLock<Id> = LazyLock::new(|| Id::new("caldav-autosize"));

        let date_str = self.now.format("%a, %-d %b %H:%M").to_string();
        let btn = button::custom(text::body(date_str))
            .padding([0, 8])
            .class(cosmic::theme::Button::AppletIcon)
            .on_press(Message::TogglePopup);

        autosize::autosize(btn, AUTOSIZE_ID.clone()).into()
    }

    fn view_window(&self, _id: window::Id) -> Element<'_, Message> {
        let today = NaiveDate::from_ymd_opt(self.now.year(), self.now.month(), self.now.day())
            .unwrap_or_default();

        let month_name = NaiveDate::from_ymd_opt(self.view_year, self.view_month, 1)
            .map(|d| d.format("%B").to_string())
            .unwrap_or_default();

        let month_controls = cosmic::iced::widget::row![
            button::icon(widget::icon::from_name("go-previous-symbolic"))
                .padding(8)
                .on_press(Message::PrevMonth),
            button::icon(widget::icon::from_name("go-next-symbolic"))
                .padding(8)
                .on_press(Message::NextMonth),
            button::icon(widget::icon::from_name("list-add-symbolic"))
                .padding(8)
                .on_press(Message::ToggleAddForm),
        ]
        .spacing(4);

        let header = cosmic::iced::widget::row![
            text(format!("{} {}", month_name, self.view_year)).size(16),
            container(text(" ")).width(Length::Fill),
            month_controls,
        ]
        .align_y(Alignment::Center)
        .padding([12, 20]);

        let calendar = self.calendar_grid(today);
        let content = cosmic::iced::widget::column![
            header,
            calendar.padding(cosmic::iced::Padding {
                top: 0.0,
                right: 12.0,
                bottom: 6.0,
                left: 12.0,
            }),
            widget::divider::horizontal::default(),
            self.events_list(),
            self.add_event_section(),
        ];

        self.core.applet.popup_container(container(content)).into()
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch(vec![
            cosmic::iced::time::every(Duration::from_secs(1)).map(|_| Message::Tick),
            cosmic::iced::time::every(Duration::from_secs(300)).map(|_| Message::SyncTick),
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
        accounts
            .iter()
            .cloned()
            .map(|account| {
                let account_id = account.id.clone();
                let client = CalDavClient::new(account.url.clone(), account.username.clone(), account.password.clone());
                Task::perform(async move { client.get_calendars().await }, move |r| {
                    cosmic::Action::App(Message::CalendarsLoaded(account_id, r))
                })
            })
            .collect::<Vec<_>>(),
    )
}

impl CalDavApplet {
    fn reset_form(&mut self) {
        self.form_title = String::new();
        self.form_start_date = self.date_selected.format("%Y-%m-%d").to_string();
        self.form_start_time = String::from("09:00");
        self.form_end_date = self.date_selected.format("%Y-%m-%d").to_string();
        self.form_end_time = String::from("10:00");
        self.form_location = String::new();
        self.form_description = String::new();
        self.form_reminder = String::from("15");
        self.form_error = None;
    }

    fn begin_edit(&mut self, item: AppletEvent) {
        let start_local = item
            .event
            .start
            .map(|dt| dt.with_timezone(&Local))
            .unwrap_or_else(Local::now);
        let end_local = item
            .event
            .end
            .map(|dt| dt.with_timezone(&Local))
            .filter(|end| *end > start_local)
            .unwrap_or(start_local + chrono::Duration::hours(1));

        self.date_selected = start_local.date_naive();
        self.form_title = item.event.summary.clone();
        self.form_start_date = start_local.format("%Y-%m-%d").to_string();
        self.form_start_time = start_local.format("%H:%M").to_string();
        self.form_end_date = end_local.format("%Y-%m-%d").to_string();
        self.form_end_time = end_local.format("%H:%M").to_string();
        self.form_location = item.event.location.clone().unwrap_or_default();
        self.form_description = item.event.description.clone().unwrap_or_default();
        self.form_reminder = String::from("15");
        self.form_error = None;
        self.show_add_form = true;
        self.editing_event = Some(EditingEvent {
            account_id: item.account_id,
            event_href: item.event.href,
            event_etag: item.event.etag,
            uid: item.event.uid,
        });
    }

    fn complete_sync_slot(&mut self) {
        self.pending_syncs = self.pending_syncs.saturating_sub(1);
        if self.pending_syncs == 0 {
            self.loading = false;
        }
    }

    fn calendar_grid(&self, today: NaiveDate) -> widget::Grid<'_, Message> {
        let mut calendar = grid().width(Length::Fill);

        // First day of the month and what weekday it falls on (0=Sun)
        let first_of_month =
            NaiveDate::from_ymd_opt(self.view_year, self.view_month, 1).unwrap_or_default();
        let first_weekday = first_of_month.weekday().num_days_from_sunday();
        let dim = days_in_month(self.view_year, self.view_month);

        // Pre-compute visuals for all days in one pass over events (O(events))
        // instead of calling day_visual per cell (O(days * events)).
        let visuals = self.month_day_visuals(first_of_month, dim);

        // Day-of-week headers: Sun Mon Tue Wed Thu Fri Sat
        for h in ["Su", "Mo", "Tu", "We", "Th", "Fr", "Sa"] {
            calendar = calendar.push(text::caption(h).apply(container).center_x(Length::Fixed(44.0)));
        }
        calendar = calendar.insert_row();

        // Only render rows needed for this month
        let total_cells = first_weekday + dim;
        let total_rows = (total_cells + 6) / 7;
        let num_cells = total_rows * 7;
        let mut day: i32 = 1 - first_weekday as i32;

        for i in 0..num_cells {
            if i > 0 && i % 7 == 0 {
                calendar = calendar.insert_row();
            }

            let d = day;
            day += 1;

            if d < 1 || d as u32 > dim {
                calendar = calendar.push(
                    container(text(""))
                        .width(Length::Fixed(44.0))
                        .height(Length::Fixed(44.0)),
                );
            } else {
                let du = d as u32;
                let day_date = NaiveDate::from_ymd_opt(self.view_year, self.view_month, du)
                    .unwrap_or_default();
                let is_selected = self.date_selected == day_date;
                let is_today = today == day_date;
                calendar = calendar.push(date_button(du, is_selected, is_today, visuals[(du - 1) as usize]));
            }
        }

        calendar
    }

fn events_list(&self) -> Element<'_, Message> {
    let day_events: Vec<(usize, &AppletEvent)> = self
        .events
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            if event_overlaps_date(&item.event, self.date_selected) {
                Some((index, item))
            } else {
                None
            }
        })
        .collect();

    let mut col = widget::column::with_capacity(5).spacing(4).padding([8, 12]);

    if self.loading {
        col = col.push(text::body("Loading events..."));
    } else if self.config.accounts.is_empty() {
        col = col.push(text::body("No accounts configured"));
    } else if day_events.is_empty() {
        col = col.push(text::body("No events this day"));
    } else {
        let show_account_label = self.config.accounts.len() > 1;

        for (index, item) in day_events {
            let time_str = format_event_time_range(&item.event);
            let base_title = if show_account_label {
                format!("{}: {}", item.label, item.event.summary)
            } else {
                item.event.summary.clone()
            };
            let title_str = match item.event.location.as_deref().filter(|s| !s.is_empty()) {
                Some(loc) => format!("{} ({})", base_title, loc),
                None => base_title,
            };

            let row = cosmic::iced::widget::row![
                container(text::body(time_str)).width(Length::Shrink),
                container(text::body(title_str)).width(Length::Fill),
            ]
            .spacing(8)
            .align_y(Alignment::Start);

            col = col.push(
                button::custom(row)
                    .width(Length::Fill)
                    .class(cosmic::theme::Button::Text)
                    .on_press(Message::EditEvent(index)),
            );
        }
    }

    col.into()
}

/// Pre-compute day visuals for the entire month in a single pass over events.
    fn month_day_visuals(&self, first_of_month: NaiveDate, days: u32) -> Vec<DayVisual> {
        let mut visuals = vec![DayVisual::default(); days as usize];

        for item in &self.events {
            let event = &item.event;
            let Some(start_utc) = event.start else {
                continue;
            };

            let start_local = start_utc.with_timezone(&Local);
            let end_local = event
                .end
                .map(|dt| dt.with_timezone(&Local))
                .filter(|end| *end > start_local)
                .unwrap_or(start_local + chrono::Duration::minutes(1));

            let start_date = start_local.date_naive();
            let mut end_date = end_local.date_naive();

            if end_local.time() == chrono::NaiveTime::MIN {
                if let Some(prev) = end_date.pred_opt() {
                    end_date = prev;
                }
            }

            let is_multiday = end_date > start_date;

            for d in 0..days {
                let date = match first_of_month.checked_add_signed(chrono::Duration::days(d as i64)) {
                    Some(date) => date,
                    None => continue,
                };
                if !event_overlaps_date(event, date) {
                    continue;
                }

                let vis = &mut visuals[d as usize];
                vis.has_events = true;

                if !is_multiday {
                    vis.marker = merge_day_marker(vis.marker, DayMarker::Single);
                    continue;
                }

                vis.has_multiday = true;

                let marker = if date == start_date {
                    DayMarker::Start
                } else if date == end_date {
                    DayMarker::End
                } else if date > start_date && date < end_date {
                    DayMarker::Middle
                } else {
                    DayMarker::Single
                };

                vis.marker = merge_day_marker(vis.marker, marker);
            }
        }

        visuals
    }

fn add_event_section(&self) -> Element<'_, Message> {
    if !self.show_add_form {
        return cosmic::iced::widget::Space::new().into();
    }

    let date_label = self.date_selected.format("%-d %b %Y").to_string();
    let editing = self.editing_event.is_some();
    let title_text = if editing {
        format!("Edit event — {}", date_label)
    } else {
        format!("New event — {}", date_label)
    };

    let mut col = widget::column::with_capacity(8)
        .spacing(6)
        .padding([4, 12, 8, 12]);

    col = col.push(text::body(title_text));

    col = col.push(widget::text_input("Title *", &self.form_title).on_input(Message::FormTitleChanged));

    col = col.push(text::caption("Start (YYYY-MM-DD, HH:MM)"));
    col = col.push(
        cosmic::iced::widget::row![
            widget::text_input("2026-01-15", &self.form_start_date)
                .on_input(Message::FormStartDateChanged)
                .width(Length::Fixed(120.0)),
            widget::text_input("09:00", &self.form_start_time)
                .on_input(Message::FormStartTimeChanged)
                .width(Length::Fixed(80.0)),
        ]
        .spacing(6)
        .align_y(cosmic::iced::Alignment::Center),
    );

    col = col.push(text::caption("End (YYYY-MM-DD, HH:MM)"));
    col = col.push(
        cosmic::iced::widget::row![
            widget::text_input("2026-01-15", &self.form_end_date)
                .on_input(Message::FormEndDateChanged)
                .width(Length::Fixed(120.0)),
            widget::text_input("10:00", &self.form_end_time)
                .on_input(Message::FormEndTimeChanged)
                .width(Length::Fixed(80.0)),
        ]
        .spacing(6)
        .align_y(cosmic::iced::Alignment::Center),
    );

    col = col.push(text::caption("Location"));
    col = col.push(widget::text_input("Location", &self.form_location).on_input(Message::FormLocationChanged));

    col = col.push(text::caption("Description"));
    col = col.push(widget::text_input("Notes", &self.form_description).on_input(Message::FormDescriptionChanged));

    col = col.push(text::caption("Reminder (minutes before)"));
    col = col.push(widget::text_input("15", &self.form_reminder).on_input(Message::FormReminderChanged));

    if let Some(err) = &self.form_error {
        col = col.push(text::body(err.clone()));
    }

    let save_label = if self.creating_event { "Saving..." } else { "Save" };
    let mut save_button = button::custom(text::body(save_label))
        .padding([6, 24])
        .class(cosmic::theme::Button::Suggested);
    if !self.creating_event {
        save_button = save_button.on_press(Message::SubmitEvent);
    }

    let mut actions = cosmic::iced::widget::row![
        button::custom(text::body("Cancel"))
            .padding([6, 24])
            .class(cosmic::theme::Button::Text)
            .on_press(Message::CancelForm),
        container(text(" ")).width(Length::Fill),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    if editing {
        let mut delete_button = button::custom(text::body("Delete"))
            .padding([6, 24])
            .class(cosmic::theme::Button::Standard);
        if !self.creating_event {
            delete_button = delete_button.on_press(Message::DeleteEditingEvent);
        }
        actions = actions.push(delete_button);
    }

    actions = actions.push(save_button);

    col = col.push(actions);

    col.into()
}
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum DayMarker {
    #[default]
    None,
    Single,
    Start,
    Middle,
    End,
}

#[derive(Debug, Clone, Copy, Default)]
struct DayVisual {
    has_events: bool,
    has_multiday: bool,
    marker: DayMarker,
}

impl DayVisual {
    fn marker(self) -> &'static str {
        match self.marker {
            DayMarker::None => "",
            DayMarker::Single => "•",
            DayMarker::Start => "╺━",
            DayMarker::Middle => "━━",
            DayMarker::End => "━╸",
        }
    }
}

fn merge_day_marker(current: DayMarker, new_marker: DayMarker) -> DayMarker {
    use DayMarker::*;

    match (current, new_marker) {
        (Middle, _) | (_, Middle) => Middle,
        (Start, End) | (End, Start) => Middle,
        (Start, _) | (_, Start) => Start,
        (End, _) | (_, End) => End,
        (Single, other) => other,
        (None, other) => other,
    }
}

fn date_button(
    day: u32,
    is_selected: bool,
    is_today: bool,
    day_visual: DayVisual,
) -> button::Button<'static, Message> {
    let style = if is_selected {
        cosmic::theme::Button::Suggested
    } else if is_today {
        cosmic::theme::Button::Standard
    } else {
        cosmic::theme::Button::Text
    };

    let mut label_col = widget::column::with_capacity(2)
        .width(Length::Fill)
        .align_x(Alignment::Center);

    label_col = label_col.push(text::body(format!("{day}")).apply(container).center(Length::Fill));

    let marker = day_visual.marker();
    label_col = label_col.push(
        text::caption(marker)
            .apply(container)
            .center(Length::Fill),
    );

    button::custom(label_col)
        .class(style)
        .width(Length::Fixed(44.0))
        .height(Length::Fixed(48.0))
        .on_press(Message::SelectDay(day))
}

fn days_in_month(year: i32, month: u32) -> u32 {
    let next = if month == 12 {
        NaiveDate::from_ymd_opt(year + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1)
    };

    next.and_then(|d| d.pred_opt())
        .map(|d| d.day())
        .unwrap_or(30)
}

fn parse_local_datetime(date: &str, time: &str) -> Result<chrono::DateTime<Local>, String> {
    use chrono::TimeZone;

    let naive_date = NaiveDate::parse_from_str(date.trim(), "%Y-%m-%d")
        .map_err(|_| "Invalid date format. Use YYYY-MM-DD".to_string())?;
    let naive_time = chrono::NaiveTime::parse_from_str(time.trim(), "%H:%M")
        .map_err(|_| "Invalid time format. Use HH:MM (24-hour)".to_string())?;
    let naive = NaiveDateTime::new(naive_date, naive_time);

    Local
        .from_local_datetime(&naive)
        .earliest()
        .ok_or_else(|| "Invalid or ambiguous local date/time (DST gap)".to_string())
}

fn local_day_bounds(date: NaiveDate) -> Option<(chrono::DateTime<Local>, chrono::DateTime<Local>)> {
    use chrono::TimeZone;

    let start_naive = date.and_hms_opt(0, 0, 0)?;
    let next_day = date.succ_opt()?;
    let end_naive = next_day.and_hms_opt(0, 0, 0)?;

    let start = Local.from_local_datetime(&start_naive).earliest()?;
    let end = Local.from_local_datetime(&end_naive).earliest()?;

    Some((start, end))
}

fn event_overlaps_date(event: &CalendarEvent, date: NaiveDate) -> bool {
    let Some(start_utc) = event.start else {
        return false;
    };

    let start_local = start_utc.with_timezone(&Local);
    let end_local = event
        .end
        .map(|dt| dt.with_timezone(&Local))
        .filter(|end| *end > start_local)
        .unwrap_or(start_local + chrono::Duration::minutes(1));

    let Some((day_start, next_day_start)) = local_day_bounds(date) else {
        return start_local.date_naive() == date;
    };

    start_local < next_day_start && end_local > day_start
}

fn format_event_time_range(event: &CalendarEvent) -> String {
    let Some(start_utc) = event.start else {
        return String::new();
    };

    let start_local = start_utc.with_timezone(&Local);

    match event.end.map(|dt| dt.with_timezone(&Local)) {
        Some(end_local) if end_local.date_naive() != start_local.date_naive() => {
            format!(
                "{} – {}",
                start_local.format("%b %-d %H:%M"),
                end_local.format("%b %-d %H:%M")
            )
        }
        Some(end_local) => format!(
            "{}–{}",
            start_local.format("%H:%M"),
            end_local.format("%H:%M")
        ),
        None => start_local.format("%H:%M").to_string(),
    }
}
