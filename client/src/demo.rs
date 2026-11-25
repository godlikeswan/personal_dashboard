use chrono::{DateTime, Duration as ChronoDuration, Local};

use crate::calendar::CalendarEvent;
use crate::config;
use crate::weather::Weather;

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

pub fn todo_events() -> Vec<CalendarEvent> {
    todo_events_at(Local::now())
}

pub fn important_events() -> Vec<CalendarEvent> {
    important_events_at(Local::now())
}

pub fn general_events() -> Vec<CalendarEvent> {
    general_events_at(Local::now())
}

pub fn weather() -> Weather {
    Weather {
        temperature: 21.5,
        code: 2,
        city: Some("Demo City".to_string()),
    }
}

fn todo_events_at(now: DateTime<Local>) -> Vec<CalendarEvent> {
    vec![
        timed(now, 0, 7, 15, 15, "[x] Water the cactus"),
        timed(now, 0, 9, 30, 20, "[ ] Buy more coffee"),
        timed(now, 0, 12, 0, 15, "[ ] Tame the inbox dragon"),
        timed(now, 0, 16, 45, 10, "[ ] Check if the cat approved the plan"),
        timed(now, 0, 21, 0, 10, "[ ] Delete one mysterious calendar"),
    ]
}

fn important_events_at(now: DateTime<Local>) -> Vec<CalendarEvent> {
    vec![
        timed(now, 1, 12, 0, 60, "Pizza Friday"),
        timed(now, 4, 9, 0, 60, "Socks day"),
        timed(now, 12, 18, 0, 120, "Cactus birthday"),
        timed(now, 35, 0, 0, 60, "New Year, new bugs"),
    ]
}

fn general_events_at(now: DateTime<Local>) -> Vec<CalendarEvent> {
    vec![
        timed(now, 0, 8, 30, 30, "Toast with the team"),
        timed(now, 0, 10, 30, 60, "Standup, the sequel"),
        timed(now, 0, 13, 0, 60, "Meeting that could've been an email"),
        timed(now, 0, 13, 45, 30, "Snack negotiation"),
        timed(now, 1, 9, 0, 120, "Deep work (allegedly)"),
        timed(now, 1, 12, 0, 60, "Lunch with the cactus"),
        all_day(now, 2, "Socks day"),
        timed(now, 3, 18, 0, 120, "Pizza o'clock"),
        timed(now, 4, 11, 0, 60, "Ship it (again)"),
    ]
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
    fn todo_events_stay_inside_the_task_window() {
        let now = Local::now();

        for event in todo_events_at(now) {
            let start = event.starts_at.expect("demo task needs a start time");
            let delta = start.signed_duration_since(now);
            assert!(delta >= ChronoDuration::hours(-24));
            assert!(delta <= ChronoDuration::hours(24));
        }
    }

    #[test]
    fn general_events_cover_the_five_day_calendar() {
        let now = Local::now();
        let events = general_events_at(now);

        assert!(events.iter().any(|event| event.all_day));
        assert!(events.len() > 5);
        assert!(events.iter().all(|event| event.starts_at.is_some()));
    }
}
