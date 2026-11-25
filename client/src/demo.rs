use chrono::{DateTime, Datelike, Duration as ChronoDuration, Local, Timelike};

use crate::calendar::{CalendarEvent, EventColor};
use crate::config;
use crate::weather::Weather;

const DEMO_YEAR: i32 = 2077;

const CACTUS_GREEN: EventColor = EventColor {
    background: [123, 196, 143],
    foreground: [18, 52, 29],
};
const DUCK_YELLOW: EventColor = EventColor {
    background: [244, 200, 86],
    foreground: [62, 44, 8],
};
const SPOON_BLUE: EventColor = EventColor {
    background: [137, 178, 232],
    foreground: [24, 49, 82],
};
const HAT_PURPLE: EventColor = EventColor {
    background: [196, 160, 220],
    foreground: [51, 27, 66],
};
const BANANA_YELLOW: EventColor = EventColor {
    background: [240, 220, 104],
    foreground: [57, 49, 11],
};
const CHAIR_CORAL: EventColor = EventColor {
    background: [235, 151, 136],
    foreground: [72, 27, 20],
};
const SOCK_LILAC: EventColor = EventColor {
    background: [170, 166, 218],
    foreground: [38, 35, 67],
};

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
        timed(now, 0, 7, 15, 15, "[x] Water cactus"),
        timed(now, 0, 9, 30, 20, "[ ] Feed rubber duck"),
        timed(now, 0, 12, 0, 15, "[ ] Count spoons"),
        timed(now, 0, 16, 45, 10, "[ ] Wear hat"),
        timed(now, 0, 21, 0, 10, "[ ] Pet rock"),
    ]
}

pub fn important_events(now: DateTime<Local>) -> Vec<CalendarEvent> {
    vec![
        timed(now, 1, 12, 0, 60, "Duck day"),
        timed(now, 4, 9, 0, 60, "Sock day"),
        timed(now, 12, 18, 0, 120, "Cactus day"),
        timed(now, 35, 0, 0, 60, "Hat day"),
    ]
}

pub fn general_events(now: DateTime<Local>) -> Vec<CalendarEvent> {
    vec![
        colored_timed(now, 0, 8, 30, 30, "Cactus party", CACTUS_GREEN),
        colored_timed(now, 0, 10, 30, 60, "Duck meeting", DUCK_YELLOW),
        colored_timed(now, 0, 13, 0, 60, "Spoon inspection", SPOON_BLUE),
        colored_timed(now, 0, 13, 45, 30, "Hat review", HAT_PURPLE),
        timed(now, 1, 9, 0, 120, "Rock lecture"),
        timed(now, 1, 12, 0, 60, "Sock walk"),
        colored_all_day(now, 2, "Sock holiday", SOCK_LILAC),
        colored_timed(now, 3, 18, 0, 120, "Banana meeting", BANANA_YELLOW),
        colored_timed(now, 4, 11, 0, 60, "Chair convention", CHAIR_CORAL),
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
        color: None,
    }
}

fn all_day(now: DateTime<Local>, day_offset: i64, summary: &str) -> CalendarEvent {
    let starts_at = local_at(now, day_offset, 0, 0);

    CalendarEvent {
        summary: summary.to_string(),
        all_day: true,
        starts_at: Some(starts_at),
        ends_at: Some(starts_at + ChronoDuration::days(1)),
        color: None,
    }
}

fn colored_timed(
    now: DateTime<Local>,
    day_offset: i64,
    hour: u32,
    minute: u32,
    minutes: i64,
    summary: &str,
    color: EventColor,
) -> CalendarEvent {
    let mut event = timed(now, day_offset, hour, minute, minutes, summary);
    event.color = Some(color);
    event
}

fn colored_all_day(
    now: DateTime<Local>,
    day_offset: i64,
    summary: &str,
    color: EventColor,
) -> CalendarEvent {
    let mut event = all_day(now, day_offset, summary);
    event.color = Some(color);
    event
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

    #[test]
    fn general_events_mix_colored_and_fallback_bars() {
        let events = general_events(now());

        assert!(events.iter().any(|event| event.color.is_some()));
        assert!(events.iter().any(|event| event.color.is_none()));
    }
}
