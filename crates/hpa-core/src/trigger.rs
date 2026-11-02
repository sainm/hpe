//! Trigger contract and the cron engine.
//!
//! The IR declares `triggers`, but the runtime needs a stricter shape than a
//! free-form `config` object. This module turns each trigger into a typed spec
//! and provides a self-contained 5-field cron implementation with fixed-offset
//! timezones (no timezone database in this build).

use serde::Deserialize;
use serde_json::Value;
use time::{OffsetDateTime, UtcOffset};

use crate::ir::{Trigger, TriggerType};

/// How many catch-up runs a single tick may perform for one trigger.
pub const MAX_CATCH_UP: u32 = 10;
/// Refuse to search further than this when computing the next fire time.
const MAX_LOOKAHEAD_DAYS: i64 = 4 * 366;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MisfirePolicy {
    /// Fire once at the next slot; missed slots are dropped (default).
    #[default]
    Skip,
    /// Fire once immediately when a slot was missed, then continue.
    RunOnce,
    /// Fire every missed slot, capped by `maxCatchUp`.
    CatchUp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleSpec {
    pub cron_raw: String,
    pub cron: CronSpec,
    pub offset: UtcOffset,
    pub offset_text: String,
    pub misfire: MisfirePolicy,
    pub max_catch_up: u32,
    pub input: Value,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookSpec {
    pub secret: Option<String>,
    pub slug: Option<String>,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventSpec {
    pub topic: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TriggerSpec {
    Schedule(Box<ScheduleSpec>),
    Webhook(WebhookSpec),
    Event(EventSpec),
    /// Invoked through the MCP `workflow.execute` tool; no separate runtime.
    Mcp,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ScheduleConfig {
    cron: String,
    #[serde(default)]
    timezone: Option<String>,
    #[serde(default)]
    misfire: Option<MisfirePolicy>,
    #[serde(default)]
    max_catch_up: Option<u32>,
    #[serde(default)]
    input: Option<Value>,
    #[serde(default)]
    enabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WebhookConfig {
    #[serde(default)]
    secret: Option<String>,
    #[serde(default)]
    slug: Option<String>,
    #[serde(default)]
    enabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EventConfig {
    #[serde(default)]
    topic: Option<String>,
    #[serde(default)]
    enabled: Option<bool>,
}

const MIN_SECRET_LEN: usize = 16;

pub fn parse_trigger(trigger: &Trigger) -> Result<TriggerSpec, String> {
    match trigger.trigger_type {
        TriggerType::Schedule => {
            let config: ScheduleConfig = serde_json::from_value(trigger.config.clone())
                .map_err(|error| format!("schedule config is invalid: {error}"))?;
            let cron = CronSpec::parse(&config.cron)?;
            let (offset, offset_text) = match config.timezone.as_deref() {
                None => (UtcOffset::UTC, "UTC".to_string()),
                Some(text) => (parse_offset(text)?, text.trim().to_string()),
            };
            let max_catch_up = config.max_catch_up.unwrap_or(MAX_CATCH_UP);
            if max_catch_up == 0 || max_catch_up > MAX_CATCH_UP {
                return Err(format!("maxCatchUp must be between 1 and {MAX_CATCH_UP}"));
            }
            if config.misfire == Some(MisfirePolicy::RunOnce) && config.max_catch_up.is_some() {
                return Err("maxCatchUp only applies to the catch_up misfire policy".to_string());
            }
            Ok(TriggerSpec::Schedule(Box::new(ScheduleSpec {
                cron_raw: config.cron.trim().to_string(),
                cron,
                offset,
                offset_text,
                misfire: config.misfire.unwrap_or_default(),
                max_catch_up,
                input: config
                    .input
                    .unwrap_or_else(|| Value::Object(Default::default())),
                enabled: config.enabled.unwrap_or(true),
            })))
        }
        TriggerType::Webhook => {
            let config: WebhookConfig = serde_json::from_value(trigger.config.clone())
                .map_err(|error| format!("webhook config is invalid: {error}"))?;
            if let Some(secret) = config.secret.as_deref() {
                if secret.len() < MIN_SECRET_LEN {
                    return Err(format!(
                        "webhook secret must be at least {MIN_SECRET_LEN} characters"
                    ));
                }
                if secret.len() > 256 {
                    return Err("webhook secret must be at most 256 characters".to_string());
                }
            }
            if let Some(slug) = config.slug.as_deref()
                && !is_slug(slug)
            {
                return Err("webhook slug must match [A-Za-z0-9_-]{1,64} or be omitted".to_string());
            }
            Ok(TriggerSpec::Webhook(WebhookSpec {
                secret: config.secret,
                slug: config.slug,
                enabled: config.enabled.unwrap_or(true),
            }))
        }
        TriggerType::Event => {
            let config: EventConfig = serde_json::from_value(trigger.config.clone())
                .map_err(|error| format!("event config is invalid: {error}"))?;
            let topic = config.topic.unwrap_or_else(|| trigger.id.clone());
            validate_event_topic(&topic)?;
            Ok(TriggerSpec::Event(EventSpec {
                topic,
                enabled: config.enabled.unwrap_or(true),
            }))
        }
        TriggerType::Mcp => Ok(TriggerSpec::Mcp),
    }
}

pub fn validate_event_topic(topic: &str) -> Result<(), String> {
    if topic.is_empty()
        || topic.len() > 128
        || !topic
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':'))
    {
        return Err(
            "event topic must match [A-Za-z0-9._:-]{1,128} and must not be empty".to_string(),
        );
    }
    Ok(())
}

fn is_slug(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 64
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// Parses `UTC`, `Z` or a fixed offset such as `+08:00`, `-0530`, `+9`.
///
/// IANA names are rejected on purpose: this build has no timezone database, and
/// silently treating `Asia/Shanghai` as a fixed offset would be wrong across
/// DST boundaries.
pub fn parse_offset(text: &str) -> Result<UtcOffset, String> {
    let trimmed = text.trim();
    if trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("utc")
        || trimmed.eq_ignore_ascii_case("z")
        || trimmed.eq_ignore_ascii_case("gmt")
    {
        return Ok(UtcOffset::UTC);
    }
    let (sign, rest) = match trimmed.as_bytes().first() {
        Some(b'+') => (1i32, &trimmed[1..]),
        Some(b'-') => (-1i32, &trimmed[1..]),
        _ => {
            return Err(format!(
                "timezone `{trimmed}` is not supported; use UTC or a fixed offset like +08:00"
            ));
        }
    };
    let (hours, minutes) = if let Some((hour, minute)) = rest.split_once(':') {
        (hour, minute)
    } else if rest.len() == 4 {
        (&rest[..2], &rest[2..])
    } else if rest.len() <= 2 {
        (rest, "0")
    } else {
        return Err(format!("timezone offset `{trimmed}` is malformed"));
    };
    let hours: i32 = hours
        .parse()
        .map_err(|_| format!("timezone offset `{trimmed}` has an invalid hour"))?;
    let minutes: i32 = minutes
        .parse()
        .map_err(|_| format!("timezone offset `{trimmed}` has an invalid minute"))?;
    if !(0..=23).contains(&hours) || !(0..=59).contains(&minutes) {
        return Err(format!("timezone offset `{trimmed}` is out of range"));
    }
    UtcOffset::from_whole_seconds(sign * (hours * 3600 + minutes * 60))
        .map_err(|_| format!("timezone offset `{trimmed}` is out of range"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronSpec {
    minutes: [bool; 60],
    hours: [bool; 24],
    days: [bool; 32],
    months: [bool; 13],
    weekdays: [bool; 7],
    dom_restricted: bool,
    dow_restricted: bool,
    text: String,
}

impl CronSpec {
    pub fn parse(text: &str) -> Result<Self, String> {
        let trimmed = text.trim();
        let fields: Vec<&str> = trimmed.split_whitespace().collect();
        if fields.len() != 5 {
            return Err(format!(
                "cron `{trimmed}` must have 5 fields (minute hour day-of-month month day-of-week)"
            ));
        }
        let (minutes, _) = parse_field(fields[0], 0, 59, None, 0)?;
        let (hours, _) = parse_field(fields[1], 0, 23, None, 0)?;
        let (days, dom_restricted) = parse_field(fields[2], 1, 31, None, 0)?;
        let (months, _) = parse_field(fields[3], 1, 12, Some(MONTH_NAMES), 1)?;
        let (dow_raw, dow_restricted) = parse_field(fields[4], 0, 7, Some(DOW_NAMES), 0)?;
        let mut minutes_out = [false; 60];
        let mut hours_out = [false; 24];
        let mut days_out = [false; 32];
        let mut months_out = [false; 13];
        let mut weekdays_out = [false; 7];
        for (index, value) in minutes.iter().enumerate() {
            minutes_out[index] = *value;
        }
        for (index, value) in hours.iter().enumerate() {
            hours_out[index] = *value;
        }
        for (index, value) in days.iter().enumerate() {
            days_out[index] = *value;
        }
        for (index, value) in months.iter().enumerate() {
            months_out[index] = *value;
        }
        for index in 0..7 {
            weekdays_out[index] = dow_raw[index] || (index == 0 && dow_raw[7]);
        }
        Ok(Self {
            minutes: minutes_out,
            hours: hours_out,
            days: days_out,
            months: months_out,
            weekdays: weekdays_out,
            dom_restricted,
            dow_restricted,
            text: trimmed.to_string(),
        })
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn matches(&self, moment: OffsetDateTime) -> bool {
        let month = moment.month() as u8 as usize;
        if !self.months[month] {
            return false;
        }
        if !self.hours[moment.hour() as usize] || !self.minutes[moment.minute() as usize] {
            return false;
        }
        let dom_match = self.days[moment.day() as usize];
        let dow_match = self.weekdays[moment.weekday().number_days_from_sunday() as usize];
        match (self.dom_restricted, self.dow_restricted) {
            // Vixie cron: when both are restricted the match is a union.
            (true, true) => dom_match || dow_match,
            (true, false) => dom_match,
            (false, true) => dow_match,
            (false, false) => true,
        }
    }

    /// First matching minute strictly after `from_unix`, in the given offset.
    pub fn next_after(&self, from_unix: i64, offset: UtcOffset) -> Option<i64> {
        let mut candidate = (from_unix.div_euclid(60) + 1) * 60;
        let limit = from_unix + MAX_LOOKAHEAD_DAYS * 86_400;
        let mut guard = 0usize;
        while candidate <= limit {
            guard += 1;
            if guard > 200_000 {
                return None;
            }
            let Ok(moment) = OffsetDateTime::from_unix_timestamp(candidate) else {
                return None;
            };
            let local = moment.to_offset(offset);
            let month_ok = self.months[local.month() as u8 as usize];
            let day_ok = self.day_matches(local);
            if !month_ok || !day_ok {
                // Jump to the next local day boundary.
                let seconds_into_day =
                    local.hour() as i64 * 3600 + local.minute() as i64 * 60 + local.second() as i64;
                candidate += 86_400 - seconds_into_day;
                continue;
            }
            if !self.hours[local.hour() as usize] {
                let seconds_into_hour = local.minute() as i64 * 60 + local.second() as i64;
                candidate += 3600 - seconds_into_hour;
                continue;
            }
            if self.minutes[local.minute() as usize] {
                return Some(candidate);
            }
            candidate += 60;
        }
        None
    }

    fn day_matches(&self, local: OffsetDateTime) -> bool {
        let dom_match = self.days[local.day() as usize];
        let dow_match = self.weekdays[local.weekday().number_days_from_sunday() as usize];
        match (self.dom_restricted, self.dow_restricted) {
            (true, true) => dom_match || dow_match,
            (true, false) => dom_match,
            (false, true) => dow_match,
            (false, false) => true,
        }
    }
}

const MONTH_NAMES: &[&str] = &[
    "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
];

const DOW_NAMES: &[&str] = &["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"];

/// Parses one cron field into a boolean set. Returns the set and whether the
/// field was restricted (i.e. not a leading `*`).
fn parse_field(
    text: &str,
    min: usize,
    max: usize,
    names: Option<&[&str]>,
    name_base: usize,
) -> Result<(Vec<bool>, bool), String> {
    let mut set = vec![false; max + 1];
    let mut restricted = true;
    for part in text.split(',') {
        let part = part.trim();
        if part.is_empty() {
            return Err(format!("cron field `{text}` has an empty element"));
        }
        let (range_text, step) = match part.split_once('/') {
            Some((range, step)) => {
                let step: usize = step
                    .parse()
                    .map_err(|_| format!("cron field `{part}` has an invalid step"))?;
                if step == 0 {
                    return Err(format!("cron field `{part}` has a zero step"));
                }
                (range, step)
            }
            None => (part, 1),
        };
        let (start, end) = if range_text == "*" {
            if part.starts_with('*') && text.trim() == "*" {
                restricted = false;
            }
            (min, max)
        } else if let Some((left, right)) = range_text.split_once('-') {
            (
                parse_value(left, min, max, names, name_base)?,
                parse_value(right, min, max, names, name_base)?,
            )
        } else {
            let value = parse_value(range_text, min, max, names, name_base)?;
            // `n/step` means "from n to max".
            (value, if step > 1 { max } else { value })
        };
        if start > end {
            return Err(format!("cron range `{part}` is reversed"));
        }
        let mut value = start;
        while value <= end {
            set[value] = true;
            value += step;
        }
    }
    Ok((set, restricted))
}

fn parse_value(
    text: &str,
    min: usize,
    max: usize,
    names: Option<&[&str]>,
    name_base: usize,
) -> Result<usize, String> {
    let trimmed = text.trim();
    let upper = trimmed.to_ascii_uppercase();
    let value = if let Some(names) = names
        && let Some(index) = names.iter().position(|name| *name == upper)
    {
        index + name_base
    } else {
        trimmed
            .parse::<usize>()
            .map_err(|_| format!("cron value `{trimmed}` is not a number"))?
    };
    // Day-of-week accepts 7 as Sunday.
    let upper_bound = if max == 7 { 7 } else { max };
    if value < min || value > upper_bound {
        return Err(format!(
            "cron value `{trimmed}` is out of range {min}-{upper_bound}"
        ));
    }
    Ok(value)
}

/// Convenience used by the scheduler to describe the next fire time.
pub fn next_fire_at(spec: &ScheduleSpec, from_unix: i64) -> Option<i64> {
    spec.cron.next_after(from_unix, spec.offset)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn spec(cron: &str) -> CronSpec {
        CronSpec::parse(cron).expect("cron parses")
    }

    fn at(text: &str) -> i64 {
        OffsetDateTime::parse(text, &time::format_description::well_known::Rfc3339)
            .expect("timestamp")
            .unix_timestamp()
    }

    #[test]
    fn every_five_minutes_walks_forward() {
        let cron = spec("*/5 * * * *");
        let start = at("2026-11-01T10:02:30Z");
        assert_eq!(
            cron.next_after(start, UtcOffset::UTC),
            Some(at("2026-11-01T10:05:00Z"))
        );
        assert_eq!(
            cron.next_after(at("2026-11-01T10:05:00Z"), UtcOffset::UTC),
            Some(at("2026-11-01T10:10:00Z"))
        );
    }

    #[test]
    fn fixed_offset_moves_the_wall_clock() {
        let cron = spec("0 9 * * *");
        let offset = parse_offset("+08:00").expect("offset");
        // 01:00Z is 09:00 +08:00, so the next 09:00 local is one day later.
        assert_eq!(
            cron.next_after(at("2026-11-01T01:00:00Z"), offset),
            Some(at("2026-11-02T01:00:00Z"))
        );
        assert_eq!(
            cron.next_after(at("2026-11-01T00:30:00Z"), offset),
            Some(at("2026-11-01T01:00:00Z"))
        );
    }

    #[test]
    fn day_of_month_and_weekday_are_a_union() {
        let cron = spec("0 0 1 * MON");
        // 2026-11-01 is a Sunday and also the 1st: both rules agree.
        assert!(
            cron.matches(OffsetDateTime::from_unix_timestamp(at("2026-11-01T00:00:00Z")).unwrap())
        );
        // 2026-11-02 is a Monday: the weekday rule matches.
        assert!(
            cron.matches(OffsetDateTime::from_unix_timestamp(at("2026-11-02T00:00:00Z")).unwrap())
        );
        // 2026-11-03 is a Tuesday and not the 1st.
        assert!(
            !cron.matches(OffsetDateTime::from_unix_timestamp(at("2026-11-03T00:00:00Z")).unwrap())
        );
    }

    #[test]
    fn month_names_and_lists_are_supported() {
        let cron = spec("0 0 1 JAN,JUL *");
        assert_eq!(
            cron.next_after(at("2026-11-05T00:00:00Z"), UtcOffset::UTC),
            Some(at("2027-01-01T00:00:00Z"))
        );
    }

    #[test]
    fn sunday_can_be_zero_or_seven() {
        let cron = spec("0 12 * * 7");
        assert!(
            cron.matches(OffsetDateTime::from_unix_timestamp(at("2026-11-01T12:00:00Z")).unwrap())
        );
    }

    #[test]
    fn invalid_cron_expressions_are_rejected() {
        for bad in [
            "",
            "* * * *",
            "* * * * * *",
            "60 * * * *",
            "*/0 * * * *",
            "5-1 * * * *",
            "a b c d e",
            "0 0 32 * *",
        ] {
            assert!(CronSpec::parse(bad).is_err(), "`{bad}` must be rejected");
        }
    }

    #[test]
    fn offsets_and_timezones_are_strict() {
        assert_eq!(parse_offset("UTC").unwrap(), UtcOffset::UTC);
        assert_eq!(
            parse_offset("+08:00").unwrap(),
            UtcOffset::from_whole_seconds(28_800).unwrap()
        );
        assert_eq!(
            parse_offset("-0530").unwrap(),
            UtcOffset::from_whole_seconds(-19_800).unwrap()
        );
        assert_eq!(
            parse_offset("+9").unwrap(),
            UtcOffset::from_whole_seconds(32_400).unwrap()
        );
        assert!(parse_offset("Asia/Shanghai").is_err());
        assert!(parse_offset("+24:00").is_err());
        assert!(parse_offset("+08:99").is_err());
    }

    fn trigger(kind: TriggerType, config: Value) -> Trigger {
        Trigger {
            id: "t1".to_string(),
            trigger_type: kind,
            config,
        }
    }

    #[test]
    fn schedule_spec_parses_and_defaults() {
        let parsed = parse_trigger(&trigger(
            TriggerType::Schedule,
            json!({"cron": "0 9 * * *", "timezone": "+08:00", "input": {"a": 1}}),
        ))
        .expect("parses");
        match parsed {
            TriggerSpec::Schedule(spec) => {
                assert_eq!(spec.offset_text, "+08:00");
                assert_eq!(spec.misfire, MisfirePolicy::Skip);
                assert_eq!(spec.max_catch_up, MAX_CATCH_UP);
                assert!(spec.enabled);
                assert_eq!(spec.input, json!({"a": 1}));
            }
            other => panic!("unexpected spec: {other:?}"),
        }
    }

    #[test]
    fn schedule_spec_rejects_unknown_and_invalid_fields() {
        assert!(
            parse_trigger(&trigger(
                TriggerType::Schedule,
                json!({"cron": "0 9 * * *", "tzz": "+08:00"})
            ))
            .is_err()
        );
        assert!(parse_trigger(&trigger(TriggerType::Schedule, json!({"cron": "nope"}))).is_err());
        assert!(
            parse_trigger(&trigger(
                TriggerType::Schedule,
                json!({"cron": "0 9 * * *", "misfire": "run_once", "maxCatchUp": 3})
            ))
            .is_err()
        );
        assert!(
            parse_trigger(&trigger(
                TriggerType::Schedule,
                json!({"cron": "0 9 * * *", "maxCatchUp": 0})
            ))
            .is_err()
        );
    }

    #[test]
    fn webhook_spec_requires_a_long_enough_secret() {
        assert!(parse_trigger(&trigger(TriggerType::Webhook, json!({}))).is_ok());
        assert!(parse_trigger(&trigger(TriggerType::Webhook, json!({"secret": "short"}))).is_err());
        assert!(
            parse_trigger(&trigger(
                TriggerType::Webhook,
                json!({"secret": "0123456789abcdef", "slug": "gh-release"})
            ))
            .is_ok()
        );
        assert!(
            parse_trigger(&trigger(TriggerType::Webhook, json!({"slug": "bad slug"}))).is_err()
        );
    }
}
