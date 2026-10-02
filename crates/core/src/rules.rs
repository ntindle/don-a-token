//! User donation rules: when the harness may spend plan capacity.
//!
//! The headline knobs from onboarding:
//! - `max_pct_of_remaining` — donate at most X% of remaining usage (default 10).
//! - `reserve_floor_pct` — always leave at least Y% untouched (default 5).
//!
//! Percent-of-limit enforcement needs usage telemetry that has no
//! programmatic API today (users see it at ChatGPT usage settings), so the
//! scheduler treats the percent knobs as job-pacing budgets calibrated from
//! observed per-job token burn, and they hard-stop on usage-limit errors.
//! Pause / skip-week / quiet-hours are enforced exactly, here.

use chrono::{DateTime, Datelike, Timelike, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QuietHours {
    /// Inclusive start hour, UTC.
    pub start_hour: u8,
    /// Exclusive end hour, UTC. May wrap past midnight.
    pub end_hour: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DonationRules {
    pub enabled: bool,
    pub max_pct_of_remaining: u8,
    pub reserve_floor_pct: u8,
    pub max_minutes_per_job: u32,
    pub cooldown_minutes_between_jobs: u32,
    pub quiet_hours: Option<QuietHours>,
    pub paused_until: Option<DateTime<Utc>>,
    /// Skip all donations during the ISO week named by `skip_week_of`.
    pub skip_this_week: bool,
    pub skip_week_of: Option<String>,
}

impl Default for DonationRules {
    fn default() -> Self {
        Self {
            enabled: true,
            max_pct_of_remaining: 10,
            reserve_floor_pct: 5,
            max_minutes_per_job: 30,
            cooldown_minutes_between_jobs: 15,
            quiet_hours: None,
            paused_until: None,
            skip_this_week: false,
            skip_week_of: None,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Verdict {
    pub run: bool,
    pub reason: &'static str,
}

impl DonationRules {
    /// Pure run/skip verdict for `now`. Percent-budget pacing is applied by
    /// the scheduler on top of this verdict.
    pub fn should_run(&self, now: DateTime<Utc>) -> Verdict {
        if !self.enabled {
            return Verdict {
                run: false,
                reason: "donations-disabled",
            };
        }
        if self.skip_this_week {
            let current = iso_week(now);
            let applies = self.skip_week_of.as_deref().is_none_or(|w| w == current);
            if applies {
                return Verdict {
                    run: false,
                    reason: "skip-week",
                };
            }
        }
        if let Some(until) = self.paused_until {
            if now < until {
                return Verdict {
                    run: false,
                    reason: "paused",
                };
            }
        }
        if let Some(q) = &self.quiet_hours {
            if in_quiet_hours(now.hour() as u8, q) {
                return Verdict {
                    run: false,
                    reason: "quiet-hours",
                };
            }
        }
        Verdict {
            run: true,
            reason: "ok",
        }
    }
}

fn iso_week(now: DateTime<Utc>) -> String {
    format!("{}-W{:02}", now.iso_week().year(), now.iso_week().week())
}

fn in_quiet_hours(hour: u8, q: &QuietHours) -> bool {
    if q.start_hour == q.end_hour {
        return false;
    }
    if q.start_hour < q.end_hour {
        (q.start_hour..q.end_hour).contains(&hour)
    } else {
        hour >= q.start_hour || hour < q.end_hour
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn noon() -> DateTime<Utc> {
        // Thursday 2026-10-01 12:00 UTC.
        Utc.with_ymd_and_hms(2026, 10, 1, 12, 0, 0).unwrap()
    }

    #[test]
    fn defaults_match_onboarding() {
        let r = DonationRules::default();
        assert_eq!(r.max_pct_of_remaining, 10);
        assert_eq!(r.reserve_floor_pct, 5);
        assert!(r.should_run(noon()).run);
    }

    #[test]
    fn disabled_never_runs() {
        let mut r = DonationRules::default();
        r.enabled = false;
        let v = r.should_run(noon());
        assert!(!v.run);
        assert_eq!(v.reason, "donations-disabled");
    }

    #[test]
    fn skip_week_applies_only_to_its_week() {
        let mut r = DonationRules::default();
        r.skip_this_week = true;
        r.skip_week_of = Some(iso_week(noon()));
        assert_eq!(r.should_run(noon()).reason, "skip-week");

        let next_week = noon() + chrono::Duration::days(7);
        assert!(r.should_run(next_week).run);
    }

    #[test]
    fn pause_expires() {
        let mut r = DonationRules::default();
        r.paused_until = Some(noon() + chrono::Duration::hours(1));
        assert_eq!(r.should_run(noon()).reason, "paused");
        assert!(r.should_run(noon() + chrono::Duration::hours(2)).run);
    }

    #[test]
    fn quiet_hours_wrap_midnight() {
        let mut r = DonationRules::default();
        r.quiet_hours = Some(QuietHours {
            start_hour: 22,
            end_hour: 7,
        });
        let night = Utc.with_ymd_and_hms(2026, 10, 1, 23, 0, 0).unwrap();
        let morning = Utc.with_ymd_and_hms(2026, 10, 2, 6, 0, 0).unwrap();
        assert_eq!(r.should_run(night).reason, "quiet-hours");
        assert_eq!(r.should_run(morning).reason, "quiet-hours");
        assert!(r.should_run(noon()).run);
    }
}
