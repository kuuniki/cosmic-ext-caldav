
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
    CalendarsLoaded {
        account_id: String,
        result: Result<Vec<Calendar>, String>,
    },
    EventsLoaded {
        account_id: String,
        account_label: String,
        result: Result<Vec<CalendarEvent>, String>,
    },
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

#[derive(Debug, Clone)]
struct CalendarViewport {
    year: i32,
    month: u32,
    selected_date: NaiveDate,
}

impl CalendarViewport {
    fn new(today: NaiveDate) -> Self {
        Self {
            year: today.year(),
            month: today.month(),
            selected_date: today,
        }
    }

    fn show_previous_month(&mut self) {
        if self.month == 1 {
            self.month = 12;
            self.year -= 1;
        } else {
            self.month -= 1;
        }
    }

    fn show_next_month(&mut self) {
        if self.month == 12 {
            self.month = 1;
            self.year += 1;
        } else {
            self.month += 1;
        }
    }

    fn select_day(&mut self, day: u32) {
        if let Some(date) = NaiveDate::from_ymd_opt(self.year, self.month, day) {
            self.selected_date = date;
        }
    }

    fn first_of_month(&self) -> NaiveDate {
        NaiveDate::from_ymd_opt(self.year, self.month, 1).unwrap_or_default()
    }

    fn month_name(&self) -> String {
        self.first_of_month()
            .format("%B")
            .to_string()
    }
}

#[derive(Debug, Clone)]
struct SyncState {
    loading: bool,
    pending_syncs: usize,
}

impl SyncState {
    fn idle() -> Self {
        Self {
            loading: false,
            pending_syncs: 0,
        }
    }

    fn begin(&mut self, account_count: usize) {
        self.loading = account_count > 0;
        self.pending_syncs = account_count;
    }

    fn finish_one(&mut self) {
        self.pending_syncs = self.pending_syncs.saturating_sub(1);
        if self.pending_syncs == 0 {
            self.loading = false;
        }
    }
}

#[derive(Debug, Clone)]
struct EventFormState {
    visible: bool,
    title: String,
    start_date: String,
    start_time: String,
    end_date: String,
    end_time: String,
    location: String,
    description: String,
    reminder: String,
    error: Option<String>,
    is_submitting: bool,
    editing_event: Option<EditingEvent>,
}

impl EventFormState {
    fn new(date: NaiveDate) -> Self {
        Self {
            visible: false,
            title: String::new(),
            start_date: date.format("%Y-%m-%d").to_string(),
            start_time: "09:00".to_string(),
            end_date: date.format("%Y-%m-%d").to_string(),
            end_time: "10:00".to_string(),
            location: String::new(),
            description: String::new(),
            reminder: "15".to_string(),
            error: None,
            is_submitting: false,
            editing_event: None,
        }
    }

    fn reset_for_date(&mut self, date: NaiveDate) {
        self.title.clear();
        self.start_date = date.format("%Y-%m-%d").to_string();
        self.start_time = "09:00".to_string();
        self.end_date = date.format("%Y-%m-%d").to_string();
        self.end_time = "10:00".to_string();
        self.location.clear();
        self.description.clear();
        self.reminder = "15".to_string();
        self.error = None;
        self.is_submitting = false;
        self.editing_event = None;
    }

    fn begin_new_event(&mut self, date: NaiveDate) {
        self.visible = true;
        self.reset_for_date(date);
    }

    fn cancel(&mut self, date: NaiveDate) {
        self.visible = false;
        self.reset_for_date(date);
    }

    fn begin_edit(&mut self, item: &AppletEvent) {
        let start_local = item
            .event
            .start
            .map(|date_time| date_time.with_timezone(&Local))
            .unwrap_or_else(Local::now);

        let end_local = item
            .event
            .end
            .map(|date_time| date_time.with_timezone(&Local))
            .filter(|end| *end > start_local)
            .unwrap_or(start_local + chrono::Duration::hours(1));

        self.visible = true;
        self.title = item.event.summary.clone();
        self.start_date = start_local.format("%Y-%m-%d").to_string();
        self.start_time = start_local.format("%H:%M").to_string();
        self.end_date = end_local.format("%Y-%m-%d").to_string();
        self.end_time = end_local.format("%H:%M").to_string();
        self.location = item.event.location.clone().unwrap_or_default();
        self.description = item.event.description.clone().unwrap_or_default();
        self.reminder = "15".to_string();
        self.error = None;
        self.is_submitting = false;
        self.editing_event = Some(EditingEvent {
            account_id: item.account_id.clone(),
            event_href: item.event.href.clone(),
            event_etag: item.event.etag.clone(),
            uid: item.event.uid.clone(),
        });
    }

    fn is_editing(&self) -> bool {
        self.editing_event.is_some()
    }

    fn save_label(&self) -> &'static str {
        if self.is_submitting {
            "Saving..."
        } else {
            "Save"
        }
    }

    fn parsed_reminder_minutes(&self) -> i32 {
        self.reminder.parse::<i32>().unwrap_or(15)
    }
}

pub struct CalDavApplet {
    core: cosmic::app::Core,
    popup: Option<window::Id>,
    now: chrono::DateTime<Local>,
    calendar: CalendarViewport,
    calendars: Vec<Calendar>,
    events: Vec<AppletEvent>,
    config: Config,
    sync: SyncState,
    form: EventFormState,
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
        let account_count = config.accounts.len();
        let accounts = config.accounts.clone();

        let app = Self {
            core,
            popup: None,
            now,
            calendar: CalendarViewport::new(today),
            calendars: Vec::new(),
            events: Vec::new(),
            config,
            sync: {
                let mut sync = SyncState::idle();
                sync.begin(account_count);
                sync
            },
            form: EventFormState::new(today),
        };

        (app, spawn_sync_tasks(&accounts))
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::TogglePopup => return self.toggle_popup(),
            Message::PopupClosed(id) => self.handle_popup_closed(id),
            Message::Tick => self.now = Local::now(),
            Message::SyncTick => return self.handle_sync_tick(),
            Message::CalendarsLoaded { account_id, result } => {
                return self.handle_calendars_loaded(account_id, result);
            }
            Message::EventsLoaded {
                account_id,
                account_label,
                result,
            } => {
                self.handle_events_loaded(account_id, account_label, result);
            }
            Message::PrevMonth => self.calendar.show_previous_month(),
            Message::NextMonth => self.calendar.show_next_month(),
            Message::SelectDay(day) => self.handle_day_selected(day),
            Message::EditEvent(index) => self.handle_edit_event(index),
            Message::ToggleAddForm => self.toggle_add_form(),
            Message::CancelForm => self.cancel_form(),
            Message::FormTitleChanged(value) => self.form.title = value,
            Message::FormStartDateChanged(value) => self.form.start_date = value,
            Message::FormStartTimeChanged(value) => self.form.start_time = value,
            Message::FormEndDateChanged(value) => self.form.end_date = value,
            Message::FormEndTimeChanged(value) => self.form.end_time = value,
            Message::FormLocationChanged(value) => self.form.location = value,
            Message::FormDescriptionChanged(value) => self.form.description = value,
            Message::FormReminderChanged(value) => self.form.reminder = value,
            Message::SubmitEvent => return self.submit_event(),
            Message::DeleteEditingEvent => return self.delete_editing_event(),
            Message::EventCreated(result) => return self.finish_event_mutation(result),
            Message::EventUpdated(result) => return self.finish_event_mutation(result),
            Message::EventDeleted(result) => return self.finish_event_mutation(result),
        }

        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        use std::sync::LazyLock;

        static AUTOSIZE_ID: LazyLock<Id> = LazyLock::new(|| Id::new("caldav-autosize"));

        let date_str = self.now.format("%a, %-d %b %H:%M").to_string();
        let button = button::custom(text::body(date_str))
            .padding([0, 8])
            .class(cosmic::theme::Button::AppletIcon)
            .on_press(Message::TogglePopup);

        autosize::autosize(button, AUTOSIZE_ID.clone()).into()
    }

    fn view_window(&self, _id: window::Id) -> Element<'_, Message> {
        let today = NaiveDate::from_ymd_opt(self.now.year(), self.now.month(), self.now.day())
            .unwrap_or_default();

        let header = self.popup_header();
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

impl CalDavApplet {
    fn toggle_popup(&mut self) -> Task<Message> {
        if let Some(id) = self.popup.take() {
            destroy_popup(id)
        } else {
            self.open_popup()
        }
    }

    fn open_popup(&mut self) -> Task<Message> {
        let popup_id = window::Id::unique();
        self.popup = Some(popup_id);

        let fetch_task = self.refresh_accounts();
        let main_id = self
            .core
            .main_window_id()
            .unwrap_or(window::Id::unique());

        let popup_settings = self
            .core
            .applet
            .get_popup_settings(main_id, popup_id, None, None, None);

        Task::batch(vec![get_popup(popup_settings), fetch_task])
    }

    fn handle_popup_closed(&mut self, id: window::Id) {
        if self.popup == Some(id) {
            self.popup = None;
        }
    }

    fn handle_sync_tick(&mut self) -> Task<Message> {
        if self.popup.is_none() && self.sync.pending_syncs == 0 {
            return self.refresh_accounts();
        }

        Task::none()
    }

    fn refresh_accounts(&mut self) -> Task<Message> {
        let account_count = self.config.accounts.len();

        if account_count == 0 {
            self.sync = SyncState::idle();
            self.events.clear();
            self.calendars.clear();
            return Task::none();
        }

        self.sync.begin(account_count);
        self.events.clear();
        self.calendars.clear();

        spawn_sync_tasks(&self.config.accounts)
    }

    fn handle_calendars_loaded(
        &mut self,
        account_id: String,
        result: Result<Vec<Calendar>, String>,
    ) -> Task<Message> {
        match result {
            Ok(calendars) => {
                if self
                    .config
                    .accounts
                    .first()
                    .map(|account| account.id == account_id)
                    .unwrap_or(false)
                {
                    self.calendars = calendars.clone();
                }

                let Some(first_calendar) = calendars.first().cloned() else {
                    self.sync.finish_one();
                    return Task::none();
                };

                let Some(account) = self.account_by_id(&account_id).cloned() else {
                    self.sync.finish_one();
                    return Task::none();
                };

                let account_label = account.username.clone();
                let client = self.client_for_account(&account);
                let calendar_href = first_calendar.href.clone();

                Task::perform(
                    async move { client.get_events(&calendar_href).await },
                    move |result| {
                        cosmic::Action::App(Message::EventsLoaded {
                            account_id,
                            account_label,
                            result,
                        })
                    },
                )
            }
            Err(error) => {
                eprintln!("CalendarsLoaded error for {}: {}", account_id, error);
                self.sync.finish_one();
                Task::none()
            }
        }
    }

    fn handle_events_loaded(
        &mut self,
        account_id: String,
        account_label: String,
        result: Result<Vec<CalendarEvent>, String>,
    ) {
        match result {
            Ok(events) => {
                self.events.extend(events.into_iter().map(|event| AppletEvent {
                    account_id: account_id.clone(),
                    label: account_label.clone(),
                    event,
                }));

                self.sync.finish_one();

                if self.sync.pending_syncs == 0 {
                    self.events.sort_by_key(|item| item.event.start);
                }
            }
            Err(error) => {
                eprintln!(
                    "EventsLoaded error for {}/{}: {}",
                    account_id, account_label, error
                );
                self.sync.finish_one();
            }
        }
    }

    fn handle_day_selected(&mut self, day: u32) {
        self.calendar.select_day(day);
        self.form.visible = false;
        self.form.editing_event = None;
        self.form.error = None;
    }

    fn handle_edit_event(&mut self, index: usize) {
        if let Some(item) = self.events.get(index).cloned() {
            if let Some(start) = item.event.start {
                self.calendar.selected_date = start.with_timezone(&Local).date_naive();
            }
            self.form.begin_edit(&item);
        }
    }

    fn toggle_add_form(&mut self) {
        if self.form.visible && !self.form.is_editing() {
            self.form.visible = false;
            self.form.error = None;
        } else {
            self.form.begin_new_event(self.calendar.selected_date);
        }
    }

    fn cancel_form(&mut self) {
        self.form.cancel(self.calendar.selected_date);
    }

    fn submit_event(&mut self) -> Task<Message> {
        if self.form.is_submitting {
            return Task::none();
        }

        if self.form.title.trim().is_empty() {
            self.form.error = Some("Title required".into());
            return Task::none();
        }

        let start = parse_local_datetime(&self.form.start_date, &self.form.start_time);
        let end = parse_local_datetime(&self.form.end_date, &self.form.end_time);

        let (start, end) = match (start, end) {
            (Err(error), _) | (_, Err(error)) => {
                self.form.error = Some(error);
                return Task::none();
            }
            (Ok(start), Ok(end)) if end <= start => {
                self.form.error = Some("End date/time must be after start date/time".into());
                return Task::none();
            }
            (Ok(start), Ok(end)) => (start, end),
        };

        let title = self.form.title.clone();
        let location = self.form.location.clone();
        let description = self.form.description.clone();
        let reminder = self.form.parsed_reminder_minutes();

        if let Some(editing) = self.form.editing_event.clone() {
            let Some(account) = self.account_by_id(&editing.account_id).cloned() else {
                self.form.error = Some("Account not found for this event".into());
                return Task::none();
            };

            let client = self.client_for_account(&account);
            self.form.is_submitting = true;
            self.form.error = None;

            return Task::perform(
                async move {
                    client
                        .update_event(
                            &editing.event_href,
                            editing.event_etag.as_deref(),
                            &editing.uid,
                            &title,
                            start,
                            end,
                            &location,
                            &description,
                            reminder,
                        )
                        .await
                },
                |result| cosmic::Action::App(Message::EventUpdated(result)),
            );
        }

        let Some(account) = self.config.accounts.first().cloned() else {
            self.form.error = Some("No account configured".into());
            return Task::none();
        };

        let Some(calendar) = self.calendars.first().cloned() else {
            self.form.error = Some("No calendar found".into());
            return Task::none();
        };

        let client = self.client_for_account(&account);
        let calendar_href = calendar.href.clone();
        self.form.is_submitting = true;
        self.form.error = None;

        Task::perform(
            async move {
                client
                    .create_event(
                        &calendar_href,
                        &title,
                        start,
                        end,
                        &location,
                        &description,
                        reminder,
                    )
                    .await
            },
            |result| cosmic::Action::App(Message::EventCreated(result)),
        )
    }

    fn delete_editing_event(&mut self) -> Task<Message> {
        if self.form.is_submitting {
            return Task::none();
        }

        let Some(editing) = self.form.editing_event.clone() else {
            return Task::none();
        };

        let Some(account) = self.account_by_id(&editing.account_id).cloned() else {
            self.form.error = Some("Account not found for this event".into());
            return Task::none();
        };

        let client = self.client_for_account(&account);
        self.form.is_submitting = true;
        self.form.error = None;

        Task::perform(
            async move { client.delete_event(&editing.event_href, editing.event_etag.as_deref()).await },
            |result| cosmic::Action::App(Message::EventDeleted(result)),
        )
    }

    fn finish_event_mutation(&mut self, result: Result<(), String>) -> Task<Message> {
        match result {
            Ok(()) => {
                self.form.cancel(self.calendar.selected_date);
                self.refresh_accounts()
            }
            Err(error) => {
                self.form.is_submitting = false;
                self.form.error = Some(error);
                Task::none()
            }
        }
    }

    fn popup_header(&self) -> Element<'_, Message> {
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

        cosmic::iced::widget::row![
            text(format!("{} {}", self.calendar.month_name(), self.calendar.year)).size(16),
            container(text(" ")).width(Length::Fill),
            month_controls,
        ]
        .align_y(Alignment::Center)
        .padding([12, 20])
        .into()
    }

    fn calendar_grid(&self, today: NaiveDate) -> widget::Grid<'_, Message> {
        let mut calendar = grid().width(Length::Fill);

        let first_of_month = self.calendar.first_of_month();
        let first_weekday = first_of_month.weekday().num_days_from_sunday();
        let days_in_month = days_in_month(self.calendar.year, self.calendar.month);
        let visuals = self.month_day_visuals(first_of_month, days_in_month);

        for heading in ["Su", "Mo", "Tu", "We", "Th", "Fr", "Sa"] {
            calendar = calendar.push(
                text::caption(heading)
                    .apply(container)
                    .center_x(Length::Fixed(44.0)),
            );
        }
        calendar = calendar.insert_row();

        let total_cells = first_weekday + days_in_month;
        let total_rows = (total_cells + 6) / 7;
        let num_cells = total_rows * 7;
        let mut day = 1 - first_weekday as i32;

        for index in 0..num_cells {
            if index > 0 && index % 7 == 0 {
                calendar = calendar.insert_row();
            }

            let current_day = day;
            day += 1;

            if current_day < 1 || current_day as u32 > days_in_month {
                calendar = calendar.push(
                    container(text(""))
                        .width(Length::Fixed(44.0))
                        .height(Length::Fixed(44.0)),
                );
                continue;
            }

            let current_day = current_day as u32;
            let day_date = NaiveDate::from_ymd_opt(
                self.calendar.year,
                self.calendar.month,
                current_day,
            )
            .unwrap_or_default();

            let is_selected = self.calendar.selected_date == day_date;
            let is_today = today == day_date;

            calendar = calendar.push(date_button(
                current_day,
                is_selected,
                is_today,
                visuals[(current_day - 1) as usize],
            ));
        }

        calendar
    }

    fn events_list(&self) -> Element<'_, Message> {
        let day_events = self.selected_day_events();
        let mut column = widget::column::with_capacity(5).spacing(4).padding([8, 12]);

        if self.sync.loading {
            column = column.push(text::body("Loading events..."));
        } else if self.config.accounts.is_empty() {
            column = column.push(text::body("No accounts configured"));
        } else if day_events.is_empty() {
            column = column.push(text::body("No events this day"));
        } else {
            let show_account_label = self.config.accounts.len() > 1;

            for (index, item) in day_events {
                let time_str = format_event_time_range(&item.event);

                let title = if show_account_label {
                    format!("{}: {}", item.label, item.event.summary)
                } else {
                    item.event.summary.clone()
                };

                let title = match item.event.location.as_deref().filter(|value| !value.is_empty()) {
                    Some(location) => format!("{} ({})", title, location),
                    None => title,
                };

                let row = cosmic::iced::widget::row![
                    container(text::body(time_str)).width(Length::Shrink),
                    container(text::body(title)).width(Length::Fill),
                ]
                .spacing(8)
                .align_y(Alignment::Start);

                column = column.push(
                    button::custom(row)
                        .width(Length::Fill)
                        .class(cosmic::theme::Button::Text)
                        .on_press(Message::EditEvent(index)),
                );
            }
        }

        column.into()
    }

    fn add_event_section(&self) -> Element<'_, Message> {
        if !self.form.visible {
            return cosmic::iced::widget::Space::new().into();
        }

        let date_label = self.calendar.selected_date.format("%-d %b %Y").to_string();
        let title = if self.form.is_editing() {
            format!("Edit event — {}", date_label)
        } else {
            format!("New event — {}", date_label)
        };

        let mut column = widget::column::with_capacity(8)
            .spacing(6)
            .padding([4, 12, 8, 12]);

        column = column.push(text::body(title));
        column = column.push(
            widget::text_input("Title *", &self.form.title)
                .on_input(Message::FormTitleChanged),
        );

        column = column.push(text::caption("Start (YYYY-MM-DD, HH:MM)"));
        column = column.push(
            cosmic::iced::widget::row![
                widget::text_input("2026-01-15", &self.form.start_date)
                    .on_input(Message::FormStartDateChanged)
                    .width(Length::Fixed(120.0)),
                widget::text_input("09:00", &self.form.start_time)
                    .on_input(Message::FormStartTimeChanged)
                    .width(Length::Fixed(80.0)),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        );

        column = column.push(text::caption("End (YYYY-MM-DD, HH:MM)"));
        column = column.push(
            cosmic::iced::widget::row![
                widget::text_input("2026-01-15", &self.form.end_date)
                    .on_input(Message::FormEndDateChanged)
                    .width(Length::Fixed(120.0)),
                widget::text_input("10:00", &self.form.end_time)
                    .on_input(Message::FormEndTimeChanged)
                    .width(Length::Fixed(80.0)),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        );

        column = column.push(text::caption("Location"));
        column = column.push(
            widget::text_input("Location", &self.form.location)
                .on_input(Message::FormLocationChanged),
        );

        column = column.push(text::caption("Description"));
        column = column.push(
            widget::text_input("Notes", &self.form.description)
                .on_input(Message::FormDescriptionChanged),
        );

        column = column.push(text::caption("Reminder (minutes before)"));
        column = column.push(
            widget::text_input("15", &self.form.reminder)
                .on_input(Message::FormReminderChanged),
        );

        if let Some(error) = &self.form.error {
            column = column.push(text::body(error.clone()));
        }

        let mut save_button = button::custom(text::body(self.form.save_label()))
            .padding([6, 24])
            .class(cosmic::theme::Button::Suggested);

        if !self.form.is_submitting {
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

        if self.form.is_editing() {
            let mut delete_button = button::custom(text::body("Delete"))
                .padding([6, 24])
                .class(cosmic::theme::Button::Standard);

            if !self.form.is_submitting {
                delete_button = delete_button.on_press(Message::DeleteEditingEvent);
            }

            actions = actions.push(delete_button);
        }

        actions = actions.push(save_button);
        column = column.push(actions);

        column.into()
    }

    fn selected_day_events(&self) -> Vec<(usize, &AppletEvent)> {
        self.events
            .iter()
            .enumerate()
            .filter_map(|(index, item)| {
                if event_overlaps_date(&item.event, self.calendar.selected_date) {
                    Some((index, item))
                } else {
                    None
                }
            })
            .collect()
    }

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
                .map(|date_time| date_time.with_timezone(&Local))
                .filter(|end| *end > start_local)
                .unwrap_or(start_local + chrono::Duration::minutes(1));

            let start_date = start_local.date_naive();
            let mut end_date = end_local.date_naive();

            if end_local.time() == chrono::NaiveTime::MIN {
                if let Some(previous_day) = end_date.pred_opt() {
                    end_date = previous_day;
                }
            }

            let is_multiday = end_date > start_date;

            for offset in 0..days {
                let Some(date) = first_of_month.checked_add_signed(chrono::Duration::days(offset as i64)) else {
                    continue;
                };

                if !event_overlaps_date(event, date) {
                    continue;
                }

                let visual = &mut visuals[offset as usize];
                visual.has_events = true;

                if !is_multiday {
                    visual.marker = merge_day_marker(visual.marker, DayMarker::Single);
                    continue;
                }

                visual.has_multiday = true;

                let marker = if date == start_date {
                    DayMarker::Start
                } else if date == end_date {
                    DayMarker::End
                } else if date > start_date && date < end_date {
                    DayMarker::Middle
                } else {
                    DayMarker::Single
                };

                visual.marker = merge_day_marker(visual.marker, marker);
            }
        }

        visuals
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
}

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
                let client = CalDavClient::new(
                    account.url.clone(),
                    account.username.clone(),
                    account.password.clone(),
                );

                Task::perform(
                    async move { client.get_calendars().await },
                    move |result| {
                        cosmic::Action::App(Message::CalendarsLoaded {
                            account_id,
                            result,
                        })
                    },
                )
            })
            .collect::<Vec<_>>(),
    )
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

    let label_col = widget::column::with_capacity(2)
        .width(Length::Fill)
        .align_x(Alignment::Center)
        .push(text::body(format!("{day}")).apply(container).center(Length::Fill))
        .push(text::caption(day_visual.marker()).apply(container).center(Length::Fill));

    button::custom(label_col)
        .class(style)
        .width(Length::Fixed(44.0))
        .height(Length::Fixed(48.0))
        .on_press(Message::SelectDay(day))
}

fn days_in_month(year: i32, month: u32) -> u32 {
    let next_month = if month == 12 {
        NaiveDate::from_ymd_opt(year + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1)
    };

    next_month
        .and_then(|date| date.pred_opt())
        .map(|date| date.day())
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
        .map(|date_time| date_time.with_timezone(&Local))
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

    match event.end.map(|date_time| date_time.with_timezone(&Local)) {
        Some(end_local) if end_local.date_naive() != start_local.date_naive() => format!(
            "{} – {}",
            start_local.format("%b %-d %H:%M"),
            end_local.format("%b %-d %H:%M")
        ),
        Some(end_local) => format!(
            "{}–{}",
            start_local.format("%H:%M"),
            end_local.format("%H:%M")
        ),
        None => start_local.format("%H:%M").to_string(),
    }
}
