use chrono::{DateTime, Datelike, Duration as ChronoDuration, Local, Timelike};

use crate::calendar::CalendarEvent;
use crate::config;
use crate::weather::Weather;

const DEMO_YEAR: i32 = 2077;

pub fn enabled() -> bool {
    config::env_nonempty("DASHBOARD_DEMO")
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

pub fn now() -> DateTime<Local> {
    let now = Local::now();
    let Some(date) = now.date_naive().with_year(DEMO_YEAR) else {
        return now;
    };

    date.and_hms_opt(now.hour(), now.minute(), now.second())
        .and_then(|naive| naive.and_local_timezone(Local).earliest())
        .unwrap_or(now)
}

pub fn todo_events(now: DateTime<Local>) -> Vec<CalendarEvent> {
    vec![
        timed(now, 0, 7, 15, 15, "[x] Water plant"),
        timed(now, 0, 9, 30, 20, "[ ] Buy milk"),
        timed(now, 0, 12, 0, 15, "[ ] Eat sandwich"),
        timed(now, 0, 16, 45, 10, "[ ] Find socks"),
        timed(now, 0, 21, 0, 10, "[ ] Take nap"),
    ]
}

pub fn important_events(now: DateTime<Local>) -> Vec<CalendarEvent> {
    vec![
        timed(now, 1, 12, 0, 60, "Weekend"),
        timed(now, 4, 9, 0, 60, "Holiday"),
        timed(now, 12, 18, 0, 120, "Birthday"),
        timed(now, 35, 0, 0, 60, "New Year"),
    ]
}

pub fn general_events(now: DateTime<Local>) -> Vec<CalendarEvent> {
    vec![
        timed(now, 0, 8, 30, 30, "Drink coffee"),
        timed(now, 0, 10, 30, 60, "Team meeting"),
        timed(now, 0, 13, 0, 60, "Lunch"),
        timed(now, 0, 13, 45, 30, "Quick call"),
        timed(now, 1, 9, 0, 120, "Read book"),
        timed(now, 1, 12, 0, 60, "Walk"),
        all_day(now, 2, "Holiday"),
        timed(now, 3, 18, 0, 120, "Watch TV"),
        timed(now, 4, 11, 0, 60, "Sleep"),
    ]
}

pub fn weather() -> Weather {
    Weather {
        temperature: 67.0,
        code: 71,
        city: Some("Demo City".to_string()),
    }
}

fn timed(
    now: DateTime<Local>,
    day_offset: i64,
    hour: u32,
    minute: u32,
    minutes: i64,
    summary: &str,
) -> CalendarEvent {
    let starts_at = local_at(now, day_offset, hour, minute);

    CalendarEvent {
        summary: summary.to_string(),
        all_day: false,
        starts_at: Some(starts_at),
        ends_at: Some(starts_at + ChronoDuration::minutes(minutes)),
    }
}

fn all_day(now: DateTime<Local>, day_offset: i64, summary: &str) -> CalendarEvent {
    let starts_at = local_at(now, day_offset, 0, 0);

    CalendarEvent {
        summary: summary.to_string(),
        all_day: true,
        starts_at: Some(starts_at),
        ends_at: Some(starts_at + ChronoDuration::days(1)),
    }
}

fn local_at(now: DateTime<Local>, day_offset: i64, hour: u32, minute: u32) -> DateTime<Local> {
    let date = now.date_naive() + ChronoDuration::days(day_offset);

    date.and_hms_opt(hour, minute, 0)
        .and_then(|naive| naive.and_local_timezone(Local).earliest())
        .unwrap_or(now)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_time_uses_the_demo_year() {
        assert_eq!(now().year(), DEMO_YEAR);
    }

    #[test]
    fn todo_events_stay_inside_the_task_window() {
        let now = now();

        for event in todo_events(now) {
            let start = event.starts_at.expect("demo task needs a start time");
            let delta = start.signed_duration_since(now);
            assert!(delta >= ChronoDuration::hours(-24));
            assert!(delta <= ChronoDuration::hours(24));
        }
    }

    #[test]
    fn general_events_cover_the_five_day_calendar() {
        let events = general_events(now());

        assert!(events.iter().any(|event| event.all_day));
        assert!(events.len() > 5);
        assert!(events.iter().all(|event| event.starts_at.is_some()));
    }
}
