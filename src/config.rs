//! The server's JSON configuration and the checks it gets at startup.

use crate::app::now_ms;
use crate::game::{self, Week};
use crate::store::Error;
use crate::{admin, oauth, privacy};
use chrono::Datelike;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Deserialize, Clone, Default)]
pub(crate) struct UserConfig {
    pub(crate) display: Option<String>,
    pub(crate) ntfy_topic: Option<String>,
    pub(crate) token: Option<String>,
    pub(crate) token_file: Option<PathBuf>,
}

#[derive(Deserialize, Clone)]
pub(crate) struct Config {
    #[serde(default = "default_addr")]
    pub(crate) addr: String,
    pub(crate) sync_base: Option<PathBuf>,
    #[serde(default = "default_state")]
    pub(crate) state_dir: PathBuf,
    pub(crate) ntfy: Option<String>,
    #[serde(default, rename = "remind_hour")]
    pub(crate) legacy_remind_hour: Option<i64>,
    pub(crate) public_url: Option<String>,
    #[serde(default)]
    pub(crate) private_site: bool,
    pub(crate) site_password_file: Option<PathBuf>,
    #[serde(default)]
    pub(crate) site_trust_proxy: bool,
    #[serde(default = "default_week_timezone")]
    pub(crate) week_timezone: String,
    #[serde(default = "default_week_rollover_hour")]
    pub(crate) week_rollover_hour: u32,
    #[serde(default)]
    pub(crate) competition_start_date: Option<String>,
    #[serde(default)]
    pub(crate) review_weighting: game::ReviewWeighting,
    #[serde(default = "default_log_retention_days")]
    pub(crate) log_retention_days: u32,
    /// Review minutes a day needs to count toward the streak; 0 lets any review count.
    #[serde(default)]
    pub(crate) streak_minutes: u32,
    #[serde(default)]
    pub(crate) users: HashMap<String, UserConfig>,
    /// Lets anyone create an account with a username and password.
    #[serde(default)]
    pub(crate) registration: bool,
    /// Players who can open the dev console: announcements, statistics and recent errors.
    #[serde(default)]
    pub(crate) devs: Vec<String>,
    /// Who runs this server, for the privacy notice.
    #[serde(default)]
    pub(crate) privacy: privacy::Notice,
    /// Sign-in with Google, GitHub and GitLab, each off until configured.
    #[serde(default)]
    pub(crate) oauth: oauth::Settings,
}

fn default_log_retention_days() -> u32 {
    admin::DEFAULT_LOG_RETENTION_DAYS
}

fn default_week_timezone() -> String {
    "UTC".into()
}

fn default_week_rollover_hour() -> u32 {
    4
}

fn default_addr() -> String {
    "127.0.0.1:8097".into()
}

fn default_state() -> PathBuf {
    "state".into()
}

impl Config {
    pub(crate) fn settings(&self) -> admin::Settings {
        admin::Settings {
            review_weighting: self.review_weighting,
            log_retention_days: self.log_retention_days,
            streak_minutes: self.streak_minutes,
        }
    }

    pub(crate) fn read(path: &str) -> Result<Self, Error> {
        Ok(serde_json::from_slice(
            &std::fs::read(path).map_err(|e| format!("cannot read config {path}: {e}"))?,
        )?)
    }

    /// Warns about superseded settings and rejects a competition start outside 1970 to today.
    pub(crate) fn check(&self) -> Result<(), Error> {
        if self.legacy_remind_hour.is_some() {
            eprintln!("remind_hour is superseded by personal reminder preferences at /community.");
        }
        if let Some(date) = &self.competition_start_date {
            let date = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
                .map_err(|_| "competition_start_date must be YYYY-MM-DD")?;
            if date.year() < 1970
                || date
                    > chrono::DateTime::from_timestamp_millis(now_ms())
                        .unwrap()
                        .date_naive()
            {
                return Err("competition_start_date must be between 1970 and today".into());
            }
        }
        Ok(())
    }

    /// Reads each `token_file` into its user's token.
    pub(crate) fn load_tokens(&mut self) -> Result<(), Error> {
        for (name, user) in &mut self.users {
            if let Some(file) = &user.token_file {
                let token = std::fs::read_to_string(file)
                    .map_err(|e| format!("cannot read token for {name}: {e}"))?;
                user.token = Some(token.trim().to_string());
            }
        }
        Ok(())
    }

    /// The competition week from `week_timezone` and `week_rollover_hour`.
    pub(crate) fn week(&self) -> Result<Week, Error> {
        let week = Week {
            tz: self
                .week_timezone
                .parse()
                .map_err(|e| format!("week_timezone {:?}: {e}", self.week_timezone))?,
            rollover_hour: self.week_rollover_hour,
        };
        if week.rollover_hour > 23 {
            return Err("week_rollover_hour must be between 0 and 23".into());
        }
        Ok(week)
    }
}
