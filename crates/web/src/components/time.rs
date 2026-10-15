use chrono::{DateTime, Utc};
use dioxus::prelude::*;

/// Human relative text such as "just now", "5 minutes ago" or "2 days ago".
/// Older than 30 days falls back to the calendar date. Future times read as "just now".
pub fn relative_text(at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let secs = (now - at).num_seconds();
    if secs < 60 {
        return "just now".to_string();
    }
    let mins = secs / 60;
    if mins < 60 {
        return plural(mins, "minute");
    }
    let hours = mins / 60;
    if hours < 24 {
        return plural(hours, "hour");
    }
    let days = hours / 24;
    if days <= 30 {
        return plural(days, "day");
    }
    at.format("%Y-%m-%d").to_string()
}

fn plural(n: i64, unit: &str) -> String {
    if n == 1 {
        format!("1 {unit} ago")
    } else {
        format!("{n} {unit}s ago")
    }
}

/// `<time>` element whose text is computed at render time and refreshed on the client every 60 s.
#[component]
pub fn RelativeTime(at: DateTime<Utc>) -> Element {
    let text = use_signal(|| relative_text(at, Utc::now()));
    let datetime = at.to_rfc3339();
    #[cfg(target_arch = "wasm32")]
    use_effect(move || {
        let mut text = text;
        spawn(async move {
            loop {
                gloo_timers::future::TimeoutFuture::new(60_000).await;
                text.set(relative_text(at, Utc::now()));
            }
        });
    });
    rsx! {
        time { datetime: "{datetime}", "{text}" }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-10T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn recent_times_read_as_just_now() {
        assert_eq!(
            relative_text(now() - Duration::seconds(30), now()),
            "just now"
        );
    }

    #[test]
    fn hours_and_singular_units() {
        assert_eq!(
            relative_text(now() - Duration::hours(1), now()),
            "1 hour ago"
        );
        assert_eq!(
            relative_text(now() - Duration::hours(2), now()),
            "2 hours ago"
        );
        assert_eq!(
            relative_text(now() - Duration::minutes(5), now()),
            "5 minutes ago"
        );
    }

    #[test]
    fn old_times_fall_back_to_date() {
        let at = now() - Duration::days(40);
        assert_eq!(relative_text(at, now()), "2026-08-31");
    }
}
