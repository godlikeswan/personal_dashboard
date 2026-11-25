#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod calendar;
mod config;
mod demo;
mod logging;
mod oauth;
mod tokens;
mod watch;
mod weather;

use std::env;
use std::time::Duration;

use chrono::{DateTime, Duration as ChronoDuration, Local, Timelike, Utc};
use eframe::egui;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio_tungstenite::tungstenite::Message;

use calendar::{CalendarEvent, EventColor};
use tokens::TokenManager;
use watch::WatchConfig;
use weather::Weather;

const WATCH_CHECK_SECONDS: u64 = 60 * 60;
const WEATHER_CHECK_SECONDS: u64 = 15 * 60;
const CLOCK_FONT_SIZE: f32 = 56.0;
const DATE_FONT_SIZE: f32 = 32.0;
const SCHEDULE_ROW_FACTOR: f32 = 1.45;
const BOTTOM_STRIP_HEIGHT: f32 = 92.0;

enum AppEvent {
    Status(String),
    Message(String),
    FetchStatus(String),
    FetchError(String),
    WatchStatus(String),
    AuthStatus(String),
    AuthFinished,
    SignedIn,
    RefreshAfterLogin,
    TodoEvents(Vec<CalendarEvent>),
    ImportantEvents(Vec<CalendarEvent>),
    GeneralEvents(Vec<CalendarEvent>),
    Weather(Weather),
    WeatherError(String),
}

struct DashboardApp {
    ctx: egui::Context,
    tx: UnboundedSender<AppEvent>,
    rx: UnboundedReceiver<AppEvent>,
    status: String,
    fetch_status: String,
    watch_status: String,
    auth_status: String,
    signing_in: bool,
    signed_in: bool,
    demo_mode: bool,
    latest_signal: String,
    todo_events: Vec<CalendarEvent>,
    important_events: Vec<CalendarEvent>,
    general_events: Vec<CalendarEvent>,
    weather: Option<Weather>,
    weather_status: String,
    tokens: TokenManager,
    watch_configs: Vec<WatchConfig>,
    // Kept alive so the spawned tasks keep running.
    _runtime: tokio::runtime::Runtime,
}

impl DashboardApp {
    fn new(ctx: egui::Context) -> Self {
        let ws_url = config::dashboard_ws_url();
        let fallback_minutes = env::var("FALLBACK_SYNC_MINUTES")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(15)
            .max(1);
        let tokens = TokenManager::from_env();
        let has_refresh_token = config::env_nonempty("GOOGLE_REFRESH_TOKEN").is_some();
        let has_client_id = config::google_client_id().is_some();
        let demo_mode = demo::enabled();

        let (tx, rx) = mpsc::unbounded_channel();

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to start tokio runtime");

        let watch_configs = [
            ("primary", ""),
            ("todo", "_TODO"),
            ("important", "_IMPORTANT"),
        ]
        .into_iter()
        .map(|(alias, suffix)| WatchConfig::for_calendar(alias, suffix))
        .collect::<Vec<_>>();

        if demo_mode {
            let now = demo::now();
            logging::log("Demo mode enabled");

            return Self {
                ctx,
                tx,
                rx,
                status: "Demo mode".to_string(),
                fetch_status: "Mock calendars".to_string(),
                watch_status: "Mock data".to_string(),
                auth_status: "Demo data".to_string(),
                signing_in: false,
                signed_in: true,
                demo_mode: true,
                latest_signal: "Mock signal".to_string(),
                todo_events: demo::todo_events(now),
                important_events: demo::important_events(now),
                general_events: demo::general_events(now),
                weather: Some(demo::weather()),
                weather_status: "Demo weather".to_string(),
                tokens,
                watch_configs,
                _runtime: runtime,
            };
        }

        let task_ctx = ctx.clone();
        let ws_tx = tx.clone();
        let task_url = ws_url.clone();
        runtime.spawn(async move {
            websocket_loop(task_ctx, ws_tx, task_url).await;
        });

        let fallback_ctx = ctx.clone();
        let fallback_tx = tx.clone();
        let fallback_tokens = tokens.clone();
        runtime.spawn(async move {
            fallback_loop(fallback_ctx, fallback_tx, fallback_tokens, fallback_minutes).await;
        });

        let weather_ctx = ctx.clone();
        let weather_tx = tx.clone();
        runtime.spawn(async move {
            weather_loop(weather_ctx, weather_tx).await;
        });

        for config_values in &watch_configs {
            let watch_ctx = ctx.clone();
            let watch_tx = tx.clone();
            let watch_tokens = tokens.clone();
            let watch_config = config_values.clone();
            runtime.spawn(async move {
                watch_loop(watch_ctx, watch_tx, watch_tokens, watch_config).await;
            });
        }

        let signed_in = has_refresh_token;
        let auto_login = !signed_in && has_client_id;

        if auto_login {
            spawn_login_task(&runtime, ctx.clone(), tx.clone(), tokens.clone());
        } else if !has_refresh_token {
            let _ = tx.send(AppEvent::AuthStatus(
                "Set GOOGLE_CLIENT_ID_OVERRIDE in dashboard-settings.txt, then click Sign in"
                    .to_string(),
            ));
        }

        let auth_status = if auto_login {
            "Opening browser for Google sign-in...".to_string()
        } else if signed_in {
            "Google sign-in ready".to_string()
        } else {
            "Sign in required".to_string()
        };

        let app = Self {
            ctx,
            tx,
            rx,
            status: "Connecting...".to_string(),
            fetch_status: "Loading calendars...".to_string(),
            watch_status: "Checking webhook channels...".to_string(),
            auth_status,
            signing_in: auto_login,
            signed_in,
            demo_mode: false,
            latest_signal: "No signal yet".to_string(),
            todo_events: Vec::new(),
            important_events: Vec::new(),
            general_events: Vec::new(),
            weather: None,
            weather_status: "Loading weather...".to_string(),
            tokens,
            watch_configs,
            _runtime: runtime,
        };

        app.spawn_fetch_all();
        app
    }

    fn spawn_fetch_all(&self) {
        let ctx = self.ctx.clone();
        let tx = self.tx.clone();
        let tokens = self.tokens.clone();

        send(
            &ctx,
            &tx,
            AppEvent::FetchStatus("Fetching calendars...".to_string()),
        );

        self._runtime.spawn(async move {
            fetch_all_events(&ctx, &tx, &tokens).await;
        });
    }

    fn spawn_login(&mut self) {
        if self.signing_in {
            return;
        }

        self.signing_in = true;
        spawn_login_task(
            &self._runtime,
            self.ctx.clone(),
            self.tx.clone(),
            self.tokens.clone(),
        );
    }

    fn spawn_watch_check_once(&self) {
        for config_values in &self.watch_configs {
            let ctx = self.ctx.clone();
            let tx = self.tx.clone();
            let tokens = self.tokens.clone();
            let watch_config = config_values.clone();

            self._runtime.spawn(async move {
                match watch::ensure_channel(&tokens, &watch_config).await {
                    Ok(status) => send(&ctx, &tx, AppEvent::WatchStatus(status)),
                    Err(error) => send(
                        &ctx,
                        &tx,
                        AppEvent::WatchStatus(format!("Watch error: {error}")),
                    ),
                }
            });
        }
    }

    fn drain_events(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                AppEvent::Status(status) => self.status = status,
                AppEvent::Message(text) => self.record_message(text),
                AppEvent::FetchStatus(status) => self.fetch_status = status,
                AppEvent::FetchError(error) => self.fetch_status = error,
                AppEvent::WatchStatus(status) => self.watch_status = status,
                AppEvent::AuthStatus(status) => self.auth_status = status,
                AppEvent::AuthFinished => self.signing_in = false,
                AppEvent::SignedIn => self.signed_in = true,
                AppEvent::RefreshAfterLogin => {
                    self.spawn_fetch_all();
                    self.spawn_watch_check_once();
                }
                AppEvent::TodoEvents(events) => self.todo_events = events,
                AppEvent::ImportantEvents(events) => self.important_events = events,
                AppEvent::GeneralEvents(events) => self.general_events = events,
                AppEvent::Weather(weather) => self.weather = Some(weather),
                AppEvent::WeatherError(error) => self.weather_status = error,
            }
        }
    }

    fn record_message(&mut self, text: String) {
        let mut should_fetch = false;

        let summary = match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(value) => {
                let kind = value
                    .get("type")
                    .and_then(|value| value.as_str())
                    .unwrap_or("unknown");
                let state = value
                    .get("state")
                    .and_then(|value| value.as_str())
                    .unwrap_or("-");

                if kind == "calendar-changed" {
                    let channel_id = value
                        .get("channelId")
                        .and_then(|value| value.as_str())
                        .unwrap_or("");
                    if known_channel_ids().iter().any(|known| known == channel_id) {
                        should_fetch = true;
                        format!("{kind} | state: {state}")
                    } else {
                        format!("{kind} | ignored (unknown channel)")
                    }
                } else {
                    format!("{kind} | state: {state}")
                }
            }
            Err(_) => text.clone(),
        };

        self.latest_signal = summary;

        if should_fetch {
            self.spawn_fetch_all();
        }
    }
}

fn known_channel_ids() -> Vec<String> {
    [
        "GOOGLE_CHANNEL_ID",
        "GOOGLE_CHANNEL_ID_TODO",
        "GOOGLE_CHANNEL_ID_IMPORTANT",
    ]
    .into_iter()
    .filter_map(config::file_nonempty)
    .collect()
}

impl eframe::App for DashboardApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.drain_events();
        ui.ctx().request_repaint_after(Duration::from_secs(1));

        let now = if self.demo_mode {
            demo::now()
        } else {
            Local::now()
        };
        let todo = self.todo_events.clone();
        let important = self.important_events.clone();
        let general = self.general_events.clone();
        let weather = self.weather.clone();
        let weather_status = self.weather_status.clone();
        let signing_in = self.signing_in;
        let signed_in = self.signed_in;

        // Top row: clock and date on the left, weather on the right.
        let weather_font_size = 56.0;
        let weather_icon_size = 52.0;
        let weather_icon_y_offset = ui.ctx().fonts_mut(|fonts| {
            fonts.row_height(&egui::FontId::proportional(weather_font_size)) - weather_icon_size
        }) * 0.5;

        ui.columns(2, |columns| {
            draw_clock_date(&mut columns[0], now);

            columns[1].with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                if let Some(current) = &weather {
                    draw_weather_icon(ui, current.code, weather_icon_size, weather_icon_y_offset);
                    ui.label(
                        egui::RichText::new(format!("{:.0}°C", current.temperature))
                            .size(weather_font_size)
                            .strong()
                            .color(egui::Color32::WHITE),
                    );
                } else {
                    ui.label(weather_status.as_str());
                }

                if !signed_in && !signing_in && ui.button("Sign in").clicked() {
                    self.spawn_login();
                }
            });
        });

        ui.add_space(8.0);
        ui.separator();

        // Anchor the bottom strip to the screen so countdowns are always visible.
        let bottom_height = BOTTOM_STRIP_HEIGHT;
        let screen = ui.ctx().viewport_rect();
        let bottom_top = screen.max.y - bottom_height;
        let main_top = ui.cursor().min.y;
        let main_height = (bottom_top - main_top - 10.0).max(160.0);
        let section_gap = 12.0;
        let available_height = (main_height - section_gap).max(0.0);
        let tasks_schedule_height = (available_height * 0.62)
            .max(90.0)
            .min(available_height * 0.72);
        let calendar_height = available_height - tasks_schedule_height;
        let half_width = screen.width() * 0.5;
        let panel_padding = 10.0;
        let calendar_top = main_top + tasks_schedule_height + section_gap;

        let tasks_rect = egui::Rect::from_min_max(
            egui::pos2(screen.min.x + panel_padding, main_top),
            egui::pos2(
                screen.min.x + half_width - panel_padding,
                main_top + tasks_schedule_height,
            ),
        );
        let schedule_rect = egui::Rect::from_min_max(
            egui::pos2(screen.min.x + half_width + panel_padding, main_top),
            egui::pos2(
                screen.max.x - panel_padding,
                main_top + tasks_schedule_height,
            ),
        );
        let calendar_rect = egui::Rect::from_min_max(
            egui::pos2(screen.min.x, calendar_top),
            egui::pos2(screen.max.x, calendar_top + calendar_height),
        );

        let tasks = visible_tasks(&todo, now);
        let task_font = task_font_size(ui, &tasks, half_width - 24.0);
        let task_row_height = task_font * 1.55;

        let (schedule_rows, number_by_index) = numbered_schedule(&general, now);
        let schedule_font = schedule_font_size(
            ui,
            &schedule_rows,
            half_width - 24.0,
            tasks_schedule_height - 8.0,
        );
        let schedule_row_height = schedule_font * SCHEDULE_ROW_FACTOR;

        draw_task_panel(ui, tasks_rect, &tasks, task_font, task_row_height);
        draw_schedule_panel(
            ui,
            schedule_rect,
            &schedule_rows,
            schedule_font,
            schedule_row_height,
        );

        ui.painter().vline(
            screen.min.x + half_width,
            egui::Rangef::new(main_top, tasks_rect.max.y),
            ui.visuals().widgets.noninteractive.bg_stroke,
        );
        ui.painter().hline(
            egui::Rangef::new(screen.min.x, screen.max.x),
            calendar_top - section_gap * 0.5,
            ui.visuals().widgets.noninteractive.bg_stroke,
        );

        draw_five_day_timeline(ui, calendar_rect, now, &general, &number_by_index);

        let debug_status = cfg!(debug_assertions).then(|| {
            format!(
                "{}  |  {}  |  {}  |  {}",
                self.status, self.fetch_status, self.watch_status, self.auth_status
            )
        });
        draw_bottom_strip(ui, now, &important, debug_status.as_deref());
    }
}

fn visible_tasks(events: &[CalendarEvent], now: DateTime<Local>) -> Vec<(bool, String)> {
    let window = ChronoDuration::hours(24);

    events
        .iter()
        .filter_map(|event| {
            let start = event.starts_at?;
            let delta = start.signed_duration_since(now);
            if delta < -window || delta > window {
                return None;
            }
            Some(clean_task_title(&event.summary))
        })
        .collect()
}

fn clean_task_title(summary: &str) -> (bool, String) {
    let raw = summary.trim();
    let checked = raw.to_ascii_lowercase().starts_with("[x]");
    let unchecked = raw.starts_with("[ ]");
    let title = if checked || unchecked { &raw[3..] } else { raw };
    (checked, title.trim_start().to_string())
}

fn task_font_size(ui: &egui::Ui, tasks: &[(bool, String)], max_width: f32) -> f32 {
    if tasks.is_empty() {
        return 18.0;
    }

    let reference_size = 12.0;
    let reference_font = egui::FontId::proportional(reference_size);
    let color = egui::Color32::WHITE;
    let longest_title = ui.ctx().fonts_mut(|fonts| {
        tasks
            .iter()
            .map(|(_, title)| {
                fonts
                    .layout_no_wrap(title.clone(), reference_font.clone(), color)
                    .size()
                    .x
            })
            .fold(0.0_f32, f32::max)
    });

    let checkbox_width = reference_size * 0.82 + 10.0;
    let available = (max_width - checkbox_width).max(1.0);
    (reference_size * available / longest_title.max(1.0)).clamp(18.0, 36.0)
}

fn draw_clock_date(ui: &mut egui::Ui, now: DateTime<Local>) {
    let time_font = egui::FontId::proportional(CLOCK_FONT_SIZE);
    let date_font = egui::FontId::proportional(DATE_FONT_SIZE);
    let time = now.format("%H:%M:%S").to_string();
    let date = now.format("%A, %B %d, %Y").to_string();
    let color = egui::Color32::WHITE;

    let (time_width, date_width, time_height) = ui.ctx().fonts_mut(|fonts| {
        let time_width = fonts
            .layout_no_wrap(time.clone(), time_font.clone(), color)
            .size()
            .x;
        let date_width = fonts
            .layout_no_wrap(date.clone(), date_font.clone(), color)
            .size()
            .x;
        let time_height = fonts.row_height(&time_font);
        (time_width, date_width, time_height)
    });

    let gap = 12.0;
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(time_width + gap + date_width, time_height),
        egui::Sense::hover(),
    );
    let painter = ui.painter();

    // Offset the smaller text so both strings sit on the same visual baseline.
    let date_top_shift = (CLOCK_FONT_SIZE - DATE_FONT_SIZE) * 0.93;

    painter.text(
        egui::pos2(rect.min.x, rect.min.y),
        egui::Align2::LEFT_TOP,
        time,
        time_font,
        color,
    );
    painter.text(
        egui::pos2(rect.min.x + time_width + gap, rect.min.y + date_top_shift),
        egui::Align2::LEFT_TOP,
        date,
        date_font,
        color,
    );
}

#[derive(Clone)]
struct ScheduleRow {
    number: usize,
    summary: String,
    time: String,
}

fn schedule_line(row: &ScheduleRow) -> String {
    format!("{}. {} {}", row.number, row.summary, row.time)
}

fn schedule_time(event: &CalendarEvent) -> String {
    if event.all_day {
        return "all day".to_string();
    }

    match (event.starts_at, event.ends_at) {
        (Some(start), Some(end)) => format!("{}-{}", start.format("%H:%M"), end.format("%H:%M")),
        (Some(start), None) => {
            let end = start + ChronoDuration::hours(1);
            format!("{}-{}", start.format("%H:%M"), end.format("%H:%M"))
        }
        _ => "time TBD".to_string(),
    }
}

fn effective_end(event: &CalendarEvent, start: DateTime<Local>) -> DateTime<Local> {
    let mut end = event
        .ends_at
        .unwrap_or_else(|| start + ChronoDuration::hours(1));

    if end <= start {
        end = start + ChronoDuration::minutes(15);
    }

    end
}

fn calendar_days(event: &CalendarEvent, now: DateTime<Local>) -> Option<(i64, i64)> {
    let start = event.starts_at?;
    let end = effective_end(event, start);
    let first_day = (start.date_naive() - now.date_naive()).num_days().max(0);
    let last_day = ((end - ChronoDuration::nanoseconds(1)).date_naive() - now.date_naive())
        .num_days()
        .min(4);

    (first_day <= last_day).then_some((first_day, last_day))
}

fn numbered_schedule(
    events: &[CalendarEvent],
    now: DateTime<Local>,
) -> (Vec<ScheduleRow>, Vec<Option<usize>>) {
    let mut candidates = events
        .iter()
        .enumerate()
        .filter(|(_, event)| calendar_days(event, now).is_some())
        .collect::<Vec<_>>();

    candidates.sort_by(|(left_index, left), (right_index, right)| {
        left.starts_at
            .cmp(&right.starts_at)
            .then_with(|| left_index.cmp(right_index))
    });

    let mut numbers = vec![None; events.len()];
    let rows = candidates
        .into_iter()
        .enumerate()
        .map(|(position, (index, event))| {
            let number = position + 1;
            numbers[index] = Some(number);
            ScheduleRow {
                number,
                summary: event.summary.clone(),
                time: schedule_time(event),
            }
        })
        .collect();

    (rows, numbers)
}

fn schedule_font_size(ui: &egui::Ui, rows: &[ScheduleRow], max_width: f32, max_height: f32) -> f32 {
    if rows.is_empty() {
        return 22.0;
    }

    let reference_size = 12.0;
    let reference_font = egui::FontId::proportional(reference_size);
    let color = egui::Color32::WHITE;
    let longest = ui.ctx().fonts_mut(|fonts| {
        rows.iter()
            .map(|row| {
                fonts
                    .layout_no_wrap(schedule_line(row), reference_font.clone(), color)
                    .size()
                    .x
            })
            .fold(0.0_f32, f32::max)
    });

    let available_width = (max_width - 8.0).max(1.0);
    let width_font = reference_size * available_width / longest.max(1.0);
    let height_font = (max_height / rows.len() as f32 / SCHEDULE_ROW_FACTOR).max(0.0);

    width_font.min(height_font).clamp(10.0, 34.0)
}

fn draw_task_panel(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    tasks: &[(bool, String)],
    font_size: f32,
    row_height: f32,
) {
    let painter = ui.painter();
    let color = egui::Color32::WHITE;

    if tasks.is_empty() {
        painter.text(
            egui::pos2(rect.min.x, rect.min.y + 4.0),
            egui::Align2::LEFT_TOP,
            "No tasks",
            egui::FontId::proportional(font_size.min(18.0)),
            color,
        );
        return;
    }

    let capacity = ((rect.height() - 8.0) / row_height).max(0.0).floor() as usize;

    for (index, (checked, title)) in tasks.iter().take(capacity).enumerate() {
        let y = rect.min.y + 4.0 + index as f32 * row_height;
        draw_task_row_at(
            painter, rect.min.x, y, row_height, *checked, title, font_size, color,
        );
    }
}

fn draw_task_row_at(
    painter: &egui::Painter,
    x: f32,
    y: f32,
    row_height: f32,
    checked: bool,
    title: &str,
    font_size: f32,
    color: egui::Color32,
) {
    let font_id = egui::FontId::proportional(font_size);
    let box_size = font_size * 0.82;
    let center_y = y + row_height * 0.5;
    let box_rect = egui::Rect::from_center_size(
        egui::pos2(x + box_size * 0.5, center_y),
        egui::vec2(box_size, box_size),
    )
    .shrink(1.0);
    let stroke = egui::Stroke::new(1.5, color);

    painter.rect_stroke(box_rect, 2.0, stroke, egui::StrokeKind::Inside);

    if checked {
        let a = egui::pos2(
            box_rect.min.x + box_size * 0.20,
            box_rect.min.y + box_size * 0.52,
        );
        let b = egui::pos2(
            box_rect.min.x + box_size * 0.42,
            box_rect.max.y - box_size * 0.18,
        );
        let c = egui::pos2(
            box_rect.max.x - box_size * 0.16,
            box_rect.min.y + box_size * 0.22,
        );
        let check_stroke = egui::Stroke::new(3.0, color);
        painter.line_segment([a, b], check_stroke);
        painter.line_segment([b, c], check_stroke);
    }

    painter.text(
        egui::pos2(x + box_size + 10.0, center_y),
        egui::Align2::LEFT_CENTER,
        title,
        font_id,
        color,
    );
}

fn draw_schedule_panel(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    rows: &[ScheduleRow],
    font_size: f32,
    row_height: f32,
) {
    let painter = ui.painter();
    let color = egui::Color32::WHITE;

    if rows.is_empty() {
        painter.text(
            egui::pos2(rect.min.x, rect.min.y + 4.0),
            egui::Align2::LEFT_TOP,
            "No upcoming events",
            egui::FontId::proportional(font_size.min(18.0)),
            color,
        );
        return;
    }

    for (index, row) in rows.iter().enumerate() {
        let y = rect.min.y + 4.0 + index as f32 * row_height;

        painter.text(
            egui::pos2(rect.min.x, y),
            egui::Align2::LEFT_TOP,
            schedule_line(row),
            egui::FontId::proportional(font_size),
            color,
        );
    }
}

enum WeatherIcon {
    Sun,
    PartlyCloudy,
    Cloud,
    Fog,
    Rain,
    Snow,
    Thunder,
}

fn weather_icon(code: u32) -> WeatherIcon {
    match code {
        0 => WeatherIcon::Sun,
        1..=2 => WeatherIcon::PartlyCloudy,
        3 => WeatherIcon::Cloud,
        45 | 48 => WeatherIcon::Fog,
        51..=67 | 80..=82 => WeatherIcon::Rain,
        71..=77 | 85 | 86 => WeatherIcon::Snow,
        95 => WeatherIcon::Thunder,
        _ => WeatherIcon::Cloud,
    }
}

fn draw_weather_icon(ui: &mut egui::Ui, code: u32, size: f32, y_offset: f32) {
    let (allocated_rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());

    let rect = allocated_rect.translate(egui::vec2(0.0, y_offset));
    let painter = ui.painter();
    let center = rect.center();
    let blue = egui::Color32::from_rgb(90, 160, 220);
    let cloud = egui::Color32::from_rgb(180, 190, 200);

    match weather_icon(code) {
        WeatherIcon::Sun => draw_sun(painter, center, size * 0.30),
        WeatherIcon::PartlyCloudy => {
            draw_sun(
                painter,
                egui::pos2(rect.min.x + size * 0.32, rect.min.y + size * 0.32),
                size * 0.20,
            );
            draw_cloud(
                painter,
                egui::pos2(rect.min.x + size * 0.62, rect.min.y + size * 0.62),
                size * 0.72,
                cloud,
            );
        }
        WeatherIcon::Cloud => draw_cloud(painter, center, size * 0.88, cloud),
        WeatherIcon::Fog => {
            draw_cloud(
                painter,
                egui::pos2(center.x, center.y - size * 0.10),
                size * 0.82,
                cloud,
            );
            for offset in [0.22_f32, 0.34, 0.46] {
                painter.hline(
                    egui::Rangef::new(rect.min.x + size * 0.16, rect.max.x - size * 0.16),
                    rect.min.y + size * offset,
                    egui::Stroke::new(1.5, cloud),
                );
            }
        }
        WeatherIcon::Rain => {
            draw_cloud(
                painter,
                egui::pos2(center.x, center.y - size * 0.14),
                size * 0.82,
                cloud,
            );
            for offset in [0.28_f32, 0.44] {
                let x = rect.min.x + size * (0.32 + offset * 0.5);
                painter.line_segment(
                    [
                        egui::pos2(x, rect.min.y + size * 0.56),
                        egui::pos2(x - size * 0.06, rect.min.y + size * 0.72),
                    ],
                    egui::Stroke::new(2.0, blue),
                );
            }
        }
        WeatherIcon::Snow => {
            draw_cloud(
                painter,
                egui::pos2(center.x, center.y - size * 0.14),
                size * 0.82,
                cloud,
            );
            for (x_factor, y_factor) in [(0.30_f32, 0.60_f32), (0.52, 0.70), (0.74, 0.58)] {
                painter.circle_filled(
                    egui::pos2(rect.min.x + size * x_factor, rect.min.y + size * y_factor),
                    size * 0.035,
                    egui::Color32::WHITE,
                );
            }
        }
        WeatherIcon::Thunder => {
            draw_cloud(
                painter,
                egui::pos2(center.x, center.y - size * 0.16),
                size * 0.82,
                cloud,
            );
            let bolt = egui::Color32::from_rgb(255, 210, 80);
            painter.line_segment(
                [
                    egui::pos2(rect.min.x + size * 0.52, rect.min.y + size * 0.48),
                    egui::pos2(rect.min.x + size * 0.38, rect.min.y + size * 0.70),
                ],
                egui::Stroke::new(2.5, bolt),
            );
            painter.line_segment(
                [
                    egui::pos2(rect.min.x + size * 0.38, rect.min.y + size * 0.70),
                    egui::pos2(rect.min.x + size * 0.52, rect.min.y + size * 0.70),
                ],
                egui::Stroke::new(2.5, bolt),
            );
            painter.line_segment(
                [
                    egui::pos2(rect.min.x + size * 0.52, rect.min.y + size * 0.70),
                    egui::pos2(rect.min.x + size * 0.42, rect.min.y + size * 0.90),
                ],
                egui::Stroke::new(2.5, bolt),
            );
        }
    }
}

fn draw_sun(painter: &egui::Painter, center: egui::Pos2, radius: f32) {
    let color = egui::Color32::from_rgb(255, 205, 80);
    painter.circle_filled(center, radius * 0.55, color);
    for index in 0..8 {
        let angle = index as f32 * std::f32::consts::PI / 4.0;
        let from = center + egui::vec2(angle.cos(), angle.sin()) * radius * 0.75;
        let to = center + egui::vec2(angle.cos(), angle.sin()) * radius * 1.25;
        painter.line_segment([from, to], egui::Stroke::new(1.5, color));
    }
}

fn draw_cloud(painter: &egui::Painter, center: egui::Pos2, width: f32, color: egui::Color32) {
    let height = width * 0.45;
    painter.circle_filled(
        egui::pos2(center.x - width * 0.26, center.y + height * 0.04),
        width * 0.22,
        color,
    );
    painter.circle_filled(
        egui::pos2(center.x, center.y - height * 0.12),
        width * 0.28,
        color,
    );
    painter.circle_filled(
        egui::pos2(center.x + width * 0.26, center.y + height * 0.04),
        width * 0.20,
        color,
    );
    painter.rect_filled(
        egui::Rect::from_center_size(
            egui::pos2(center.x, center.y + height * 0.22),
            egui::vec2(width * 0.88, height * 0.45),
        ),
        2.0,
        color,
    );
}

fn draw_bottom_strip(
    ui: &egui::Ui,
    now: DateTime<Local>,
    important: &[CalendarEvent],
    status: Option<&str>,
) {
    let height = BOTTOM_STRIP_HEIGHT;
    let screen = ui.ctx().viewport_rect();
    let rect =
        egui::Rect::from_min_max(egui::pos2(screen.min.x, screen.max.y - height), screen.max);
    let painter = ui.painter();
    let weak_color = ui.visuals().weak_text_color();

    painter.hline(
        egui::Rangef::new(rect.min.x, rect.max.x),
        rect.min.y,
        ui.visuals().widgets.noninteractive.bg_stroke,
    );

    let mut countdowns = important
        .iter()
        .filter_map(|event| {
            let days = days_until(event, now)?;
            Some((days, event.summary.clone()))
        })
        .collect::<Vec<_>>();
    countdowns.sort_by_key(|(days, _)| *days);

    let countdown_font_size = 32.0;
    let countdown_row_height = ui
        .ctx()
        .fonts_mut(|fonts| fonts.row_height(&egui::FontId::proportional(countdown_font_size)));
    let countdown_y = rect.center().y - countdown_row_height * 0.5;

    if countdowns.is_empty() {
        painter.text(
            egui::pos2(rect.min.x + 18.0, countdown_y),
            egui::Align2::LEFT_TOP,
            "No important events",
            egui::FontId::proportional(countdown_font_size),
            weak_color,
        );
    }

    let mut x = rect.min.x + 18.0;
    let y = countdown_y;
    let font = egui::FontId::proportional(countdown_font_size);
    for (days, summary) in countdowns {
        let text = countdown_label(days, &summary);

        let used = painter.text(
            egui::pos2(x, y),
            egui::Align2::LEFT_TOP,
            text,
            font.clone(),
            egui::Color32::WHITE,
        );
        x = used.max.x + 48.0;
        if x > rect.max.x - 180.0 {
            break;
        }
    }

    if let Some(status) = status {
        painter.text(
            egui::pos2(rect.min.x + 18.0, rect.max.y - 24.0),
            egui::Align2::LEFT_TOP,
            status,
            egui::FontId::proportional(11.0),
            weak_color,
        );
    }
}

fn rgba(rgb: [u8; 3], alpha: u8) -> egui::Color32 {
    egui::Color32::from_rgba_premultiplied(rgb[0], rgb[1], rgb[2], alpha)
}

fn segment_fill(color: Option<EventColor>, all_day: bool) -> egui::Color32 {
    let alpha = if all_day { 160 } else { 190 };
    match color {
        Some(color) => rgba(color.background, alpha),
        None if all_day => rgba([70, 160, 150], alpha),
        None => rgba([70, 120, 200], alpha),
    }
}

fn segment_head_fill(color: Option<EventColor>) -> egui::Color32 {
    match color {
        Some(color) => {
            let lighten = |channel: u8| ((u16::from(channel) * 3 + 255) / 4) as u8;
            rgba(
                [
                    lighten(color.background[0]),
                    lighten(color.background[1]),
                    lighten(color.background[2]),
                ],
                220,
            )
        }
        None => rgba([130, 180, 250], 220),
    }
}

fn segment_number_color(color: Option<EventColor>) -> egui::Color32 {
    color
        .map(|color| rgba(color.foreground, 255))
        .unwrap_or(egui::Color32::WHITE)
}

fn draw_five_day_timeline(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    now: DateTime<Local>,
    events: &[CalendarEvent],
    number_by_index: &[Option<usize>],
) {
    if rect.width() < 40.0 || rect.height() < 40.0 {
        return;
    }

    let row_height = rect.height() / 5.0;
    let plot_x = rect.min.x;
    let plot_width = rect.width().max(1.0);
    let text_color = ui.visuals().text_color();
    let grid_stroke = egui::Stroke::new(0.5, ui.visuals().widgets.noninteractive.bg_stroke.color);
    let painter = ui.painter();

    // Only the separators between rows. The bottom edge is the countdown strip's line.
    for index in 0..4 {
        let y = rect.min.y + (index as f32 + 1.0) * row_height;
        painter.hline(egui::Rangef::new(rect.min.x, rect.max.x), y, grid_stroke);
    }

    for hour in (0..=24).step_by(3) {
        let x = plot_x + (hour as f32 / 24.0) * plot_width;
        painter.vline(x, egui::Rangef::new(rect.min.y, rect.max.y), grid_stroke);
        painter.text(
            egui::pos2(x + 2.0, rect.max.y - 14.0),
            egui::Align2::LEFT_TOP,
            format!("{hour:02}"),
            egui::FontId::proportional(10.0),
            text_color,
        );
    }

    let now_seconds = now.hour() * 3600 + now.minute() * 60 + now.second();
    let now_x = plot_x + (now_seconds as f32 / 86_400.0) * plot_width;
    painter.vline(
        now_x,
        egui::Rangef::new(rect.min.y, rect.min.y + row_height),
        egui::Stroke::new(1.5, egui::Color32::from_rgb(255, 90, 90)),
    );

    #[derive(Clone)]
    struct Segment {
        day_index: usize,
        x0: f32,
        x1: f32,
        y: f32,
        height: f32,
        number: Option<usize>,
        color: Option<EventColor>,
        all_day: bool,
        is_head: bool,
    }

    let seconds_of_day = |time: DateTime<Local>| -> f32 {
        (time.hour() * 3600 + time.minute() * 60 + time.second()) as f32
    };

    let mut segments = Vec::new();

    for (index, event) in events.iter().enumerate() {
        let Some(start) = event.starts_at else {
            continue;
        };
        let Some((first_day, last_day)) = calendar_days(event, now) else {
            continue;
        };
        let end = effective_end(event, start);

        let number = number_by_index.get(index).copied().flatten();

        for day_index in first_day..=last_day {
            let day_index = day_index as usize;
            let day_date = now.date_naive() + ChronoDuration::days(day_index as i64);
            let Some(row_start) = day_date
                .and_hms_opt(0, 0, 0)
                .and_then(|naive| naive.and_local_timezone(Local).earliest())
            else {
                continue;
            };
            let row_end = row_start + ChronoDuration::days(1);
            let segment_start = start.max(row_start);
            let segment_end = end.min(row_end);
            if segment_start >= segment_end {
                continue;
            }

            if event.all_day {
                segments.push(Segment {
                    day_index,
                    x0: plot_x,
                    x1: rect.max.x,
                    y: 0.0,
                    height: 0.0,
                    number,
                    color: event.color,
                    all_day: true,
                    is_head: segment_start == start,
                });
                continue;
            }

            let x0 = plot_x + (seconds_of_day(segment_start) / 86_400.0) * plot_width;
            // An event that ends exactly at midnight has seconds_of_day == 0,
            // so use the row's right edge instead of wrapping to x = plot_x.
            let end_seconds = if segment_end == row_end {
                86_400.0
            } else {
                seconds_of_day(segment_end)
            };
            let x1 = (plot_x + (end_seconds / 86_400.0) * plot_width)
                .min(rect.max.x)
                .max(x0 + 1.0);

            segments.push(Segment {
                day_index,
                x0,
                x1,
                y: 0.0,
                height: 0.0,
                number,
                color: event.color,
                all_day: false,
                is_head: segment_start == start,
            });
        }
    }

    // Give overlapping events separate lanes. A row with one event gets one
    // full-height lane; concurrent events split the row evenly.
    let mut lane_ends = vec![Vec::<f32>::new(); 5];
    let mut lane_of = vec![0usize; segments.len()];
    let mut lane_counts = [1usize; 5];

    for day in 0..5 {
        let mut indices = segments
            .iter()
            .enumerate()
            .filter(|(_, segment)| segment.day_index == day)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        indices.sort_by(|&left, &right| {
            segments[left]
                .x0
                .total_cmp(&segments[right].x0)
                .then_with(|| segments[left].x1.total_cmp(&segments[right].x1))
        });

        for index in indices {
            let x0 = segments[index].x0;
            let mut available = None;
            for (candidate, &end) in lane_ends[day].iter().enumerate() {
                if end <= x0 + 1.0 {
                    available = Some(candidate);
                    break;
                }
            }

            let lane = match available {
                Some(found) => {
                    lane_ends[day][found] = segments[index].x1;
                    found
                }
                None => {
                    lane_ends[day].push(segments[index].x1);
                    lane_ends[day].len() - 1
                }
            };
            lane_of[index] = lane;
        }

        lane_counts[day] = lane_ends[day].len().max(1);
    }

    for (index, segment) in segments.iter_mut().enumerate() {
        let day = segment.day_index;
        let lane_height = row_height / lane_counts[day] as f32;
        segment.y =
            rect.min.y + day as f32 * row_height + lane_of[index] as f32 * lane_height + 1.0;
        segment.height = (lane_height - 2.0).max(3.0);
    }

    // Draw every bar first, then every number, so later events cannot paint over
    // an earlier event's number.
    for segment in &segments {
        let fill = segment_fill(segment.color, segment.all_day);
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(segment.x0, segment.y),
                egui::pos2(segment.x1, segment.y + segment.height),
            ),
            2.0,
            fill,
        );

        if segment.is_head && !segment.all_day {
            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(segment.x0, segment.y),
                    egui::pos2(segment.x0 + 3.0, segment.y + segment.height),
                ),
                0.0,
                segment_head_fill(segment.color),
            );
        }
    }

    for segment in &segments {
        let Some(number) = segment.number else {
            continue;
        };

        let font_size = (segment.height * 0.78).clamp(12.0, 34.0);
        let center = egui::pos2(
            (segment.x0 + segment.x1) * 0.5,
            segment.y + segment.height * 0.5,
        );

        painter.text(
            center,
            egui::Align2::CENTER_CENTER,
            number.to_string(),
            egui::FontId::proportional(font_size),
            segment_number_color(segment.color),
        );
    }

    ui.allocate_rect(rect, egui::Sense::hover());
}

fn days_until(event: &CalendarEvent, now: DateTime<Local>) -> Option<i64> {
    let start = event.starts_at?;
    Some((start.date_naive() - now.date_naive()).num_days())
}

fn countdown_label(days: i64, summary: &str) -> String {
    format!("{days}d  {summary}")
}

fn main() -> eframe::Result {
    config::load_dotenv();
    logging::log("Dashboard starting");
    logging::log(&format!(
        "Config directory: {}",
        config::config_dir().display()
    ));
    logging::log(&format!("Relay domain: {}", config::relay_domain()));
    logging::log(&format!(
        "Google client ID configured: {}",
        config::google_client_id().is_some()
    ));
    let renderer = eframe::Renderer::Glow;
    logging::log(&format!("Renderer: {renderer:?}"));

    let default_panic_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        logging::log(&format!("Dashboard panicked: {info}"));
        default_panic_hook(info);
    }));

    // Select exactly one Rustls crypto provider for HTTPS and WSS.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let viewport = egui::ViewportBuilder::default()
        .with_title("Personal Dashboard")
        .with_decorations(false)
        .with_fullscreen(true);

    let result = eframe::run_native(
        "Personal Dashboard",
        eframe::NativeOptions {
            viewport,
            renderer,
            ..Default::default()
        },
        Box::new(|cc| {
            logging::log("Creating dashboard app");
            let app = DashboardApp::new(cc.egui_ctx.clone());
            logging::log("Dashboard app ready");
            Ok(Box::new(app))
        }),
    );

    match &result {
        Ok(()) => logging::log("Dashboard window closed"),
        Err(error) => logging::log(&format!("Dashboard failed: {error}")),
    }

    result
}

fn subscription_message() -> String {
    let channel_ids = known_channel_ids();
    serde_json::json!({
        "type": "subscribe",
        "channelIds": channel_ids,
    })
    .to_string()
}

async fn websocket_loop(ctx: egui::Context, tx: UnboundedSender<AppEvent>, url: String) {
    loop {
        send(&ctx, &tx, AppEvent::Status("Connecting...".to_string()));

        match tokio_tungstenite::connect_async(url.as_str()).await {
            Ok((mut socket, _response)) => {
                send(&ctx, &tx, AppEvent::Status("Connected".to_string()));

                if let Err(error) = socket
                    .send(Message::Text(subscription_message().into()))
                    .await
                {
                    send(
                        &ctx,
                        &tx,
                        AppEvent::Status(format!("Subscribe failed: {error}")),
                    );
                    break;
                }

                let mut subscription_check = tokio::time::interval(Duration::from_secs(5));
                subscription_check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

                loop {
                    tokio::select! {
                        frame = socket.next() => match frame {
                            Some(Ok(Message::Text(text))) => {
                                send(&ctx, &tx, AppEvent::Message(text.to_string()));
                            }
                            Some(Ok(Message::Close(_))) | None => break,
                            Some(Ok(_)) => {}
                            Some(Err(error)) => {
                                send(
                                    &ctx,
                                    &tx,
                                    AppEvent::Status(format!("WebSocket error: {error}")),
                                );
                                break;
                            }
                        },
                        _ = subscription_check.tick() => {
                            // Re-send periodically so a socket that survived a
                            // Worker redeploy also gets its subscriptions stored.
                            if let Err(error) = socket
                                .send(Message::Text(subscription_message().into()))
                                .await
                            {
                                send(
                                    &ctx,
                                    &tx,
                                    AppEvent::Status(format!("Subscribe failed: {error}")),
                                );
                                break;
                            }
                        }
                    }
                }

                send(&ctx, &tx, AppEvent::Status("Disconnected".to_string()));
            }
            Err(error) => {
                send(
                    &ctx,
                    &tx,
                    AppEvent::Status(format!("Connection failed: {error}")),
                );
            }
        }

        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

async fn fetch_all_events(
    ctx: &egui::Context,
    tx: &UnboundedSender<AppEvent>,
    tokens: &TokenManager,
) {
    let now = Utc::now();

    match calendar::fetch_events_in_range(
        tokens,
        "primary",
        now,
        now + ChronoDuration::days(5),
        100,
    )
    .await
    {
        Ok(events) => {
            logging::log(&format!("Loaded {} general events", events.len()));
            send(ctx, tx, AppEvent::GeneralEvents(events));
        }
        Err(error) => send(ctx, tx, AppEvent::FetchError(format!("General: {error}"))),
    }

    match calendar::fetch_events_in_range(
        tokens,
        "todo",
        now - ChronoDuration::hours(24),
        now + ChronoDuration::hours(24),
        100,
    )
    .await
    {
        Ok(events) => {
            logging::log(&format!("Loaded {} todo events", events.len()));
            send(ctx, tx, AppEvent::TodoEvents(events));
        }
        Err(error) => send(ctx, tx, AppEvent::FetchError(format!("Todo: {error}"))),
    }

    match calendar::fetch_events_in_range(
        tokens,
        "important",
        now,
        now + ChronoDuration::days(365),
        100,
    )
    .await
    {
        Ok(events) => {
            logging::log(&format!("Loaded {} important events", events.len()));
            send(ctx, tx, AppEvent::ImportantEvents(events));
        }
        Err(error) => send(ctx, tx, AppEvent::FetchError(format!("Important: {error}"))),
    }
}

async fn fallback_loop(
    ctx: egui::Context,
    tx: UnboundedSender<AppEvent>,
    tokens: TokenManager,
    minutes: u64,
) {
    loop {
        tokio::time::sleep(Duration::from_secs(minutes * 60)).await;

        send(
            &ctx,
            &tx,
            AppEvent::FetchStatus("Fallback sync...".to_string()),
        );

        fetch_all_events(&ctx, &tx, &tokens).await;
    }
}

async fn weather_loop(ctx: egui::Context, tx: UnboundedSender<AppEvent>) {
    loop {
        match weather::fetch_weather().await {
            Ok(current) => {
                if let Some(city) = &current.city {
                    logging::log(&format!("Weather location: {city}"));
                }
                send(&ctx, &tx, AppEvent::Weather(current));
            }
            Err(error) => send(&ctx, &tx, AppEvent::WeatherError(error)),
        }

        tokio::time::sleep(Duration::from_secs(WEATHER_CHECK_SECONDS)).await;
    }
}

async fn watch_loop(
    ctx: egui::Context,
    tx: UnboundedSender<AppEvent>,
    tokens: TokenManager,
    config_values: WatchConfig,
) {
    loop {
        match watch::ensure_channel(&tokens, &config_values).await {
            Ok(status) => send(&ctx, &tx, AppEvent::WatchStatus(status)),
            Err(error) => send(
                &ctx,
                &tx,
                AppEvent::WatchStatus(format!("Watch error: {error}")),
            ),
        }

        tokio::time::sleep(Duration::from_secs(WATCH_CHECK_SECONDS)).await;
    }
}

fn spawn_login_task(
    runtime: &tokio::runtime::Runtime,
    ctx: egui::Context,
    tx: UnboundedSender<AppEvent>,
    tokens: TokenManager,
) {
    let task_ctx = ctx.clone();
    let task_tx = tx.clone();

    runtime.spawn(async move {
        send(
            &task_ctx,
            &task_tx,
            AppEvent::AuthStatus("Opening browser for Google sign-in...".to_string()),
        );

        match oauth::login_async().await {
            Ok(()) => {
                tokens.reload_from_env().await;
                send(
                    &task_ctx,
                    &task_tx,
                    AppEvent::AuthStatus("Signed in with Google".to_string()),
                );
                send(&task_ctx, &task_tx, AppEvent::AuthFinished);
                send(&task_ctx, &task_tx, AppEvent::SignedIn);
                send(&task_ctx, &task_tx, AppEvent::RefreshAfterLogin);
            }
            Err(error) => {
                send(
                    &task_ctx,
                    &task_tx,
                    AppEvent::AuthStatus(format!("Sign-in failed: {error}")),
                );
                send(&task_ctx, &task_tx, AppEvent::AuthFinished);
            }
        }
    });
}

fn send(ctx: &egui::Context, tx: &UnboundedSender<AppEvent>, event: AppEvent) {
    match &event {
        AppEvent::Status(text)
        | AppEvent::FetchStatus(text)
        | AppEvent::FetchError(text)
        | AppEvent::WatchStatus(text)
        | AppEvent::AuthStatus(text)
        | AppEvent::WeatherError(text) => logging::log(text),
        AppEvent::Message(text) => logging::log(&format!("signal: {text}")),
        AppEvent::AuthFinished
        | AppEvent::SignedIn
        | AppEvent::RefreshAfterLogin
        | AppEvent::TodoEvents(_)
        | AppEvent::ImportantEvents(_)
        | AppEvent::GeneralEvents(_)
        | AppEvent::Weather(_) => {}
    }

    let _ = tx.send(event);
    ctx.request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn schedule_time_formats_timed_event() {
        let start = Local::now() + ChronoDuration::hours(1);
        let end = start + ChronoDuration::hours(2);
        let event = CalendarEvent {
            summary: "Some Event".to_string(),
            all_day: false,
            starts_at: Some(start.clone()),
            ends_at: Some(end.clone()),
            color: None,
        };

        assert_eq!(
            schedule_time(&event),
            format!("{}-{}", start.format("%H:%M"), end.format("%H:%M"))
        );
    }

    #[test]
    fn schedule_numbers_events_in_start_order() {
        let now = Local::now();
        let first = CalendarEvent {
            summary: "First".to_string(),
            all_day: false,
            starts_at: Some(now + ChronoDuration::hours(1)),
            ends_at: Some(now + ChronoDuration::hours(2)),
            color: None,
        };
        let second = CalendarEvent {
            summary: "Second".to_string(),
            all_day: false,
            starts_at: Some(now + ChronoDuration::hours(3)),
            ends_at: Some(now + ChronoDuration::hours(4)),
            color: None,
        };

        let events = vec![second, first];
        let (rows, numbers) = numbered_schedule(&events, now);

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].summary, "First");
        assert_eq!(rows[1].summary, "Second");
        assert_eq!(numbers[0], Some(2));
        assert_eq!(numbers[1], Some(1));
    }

    #[test]
    fn schedule_includes_events_that_already_ended_today() {
        let now = Local
            .with_ymd_and_hms(2077, 6, 15, 12, 0, 0)
            .earliest()
            .expect("valid test time");
        let event = CalendarEvent {
            summary: "Morning".to_string(),
            all_day: false,
            starts_at: Some(
                Local
                    .with_ymd_and_hms(2077, 6, 15, 9, 0, 0)
                    .earliest()
                    .expect("valid test time"),
            ),
            ends_at: Some(
                Local
                    .with_ymd_and_hms(2077, 6, 15, 10, 0, 0)
                    .earliest()
                    .expect("valid test time"),
            ),
            color: None,
        };

        let (rows, numbers) = numbered_schedule(&[event], now);

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].summary, "Morning");
        assert_eq!(numbers[0], Some(1));
    }

    #[test]
    fn timeline_uses_event_colors() {
        let color = EventColor {
            background: [10, 20, 30],
            foreground: [200, 210, 220],
        };

        assert_eq!(
            segment_fill(Some(color), false),
            egui::Color32::from_rgba_premultiplied(10, 20, 30, 190)
        );
        assert_eq!(
            segment_number_color(Some(color)),
            egui::Color32::from_rgb(200, 210, 220)
        );
    }

    #[test]
    fn countdown_label_uses_numeric_days() {
        assert_eq!(countdown_label(0, "Thanksgiving"), "0d  Thanksgiving");
        assert_eq!(countdown_label(1, "New Year"), "1d  New Year");
        assert_eq!(countdown_label(35, "New Year"), "35d  New Year");
    }
}
