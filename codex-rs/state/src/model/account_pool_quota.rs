use super::AccountHealthState;
use chrono::DateTime;
use chrono::Utc;

/// Durable account quota facts for one limit family.
#[derive(Debug, Clone, PartialEq)]
pub struct AccountQuotaStateRecord {
    pub account_id: String,
    pub limit_id: String,
    pub primary_used_percent: Option<f64>,
    pub primary_resets_at: Option<DateTime<Utc>>,
    pub secondary_used_percent: Option<f64>,
    pub secondary_resets_at: Option<DateTime<Utc>>,
    pub observed_at: DateTime<Utc>,
    pub exhausted_windows: QuotaExhaustedWindows,
    pub predicted_blocked_until: Option<DateTime<Utc>>,
    pub next_probe_after: Option<DateTime<Utc>>,
    pub probe_backoff_level: i64,
    pub last_probe_result: Option<QuotaProbeResult>,
}

impl AccountQuotaStateRecord {
    pub fn compatibility_health_state(&self) -> AccountHealthState {
        if self.exhausted_windows.is_exhausted() {
            AccountHealthState::RateLimited
        } else {
            AccountHealthState::Healthy
        }
    }
}

/// Latest known exhausted quota windows for one family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaExhaustedWindows {
    None,
    Primary,
    Secondary,
    Both,
    Unknown,
}

impl QuotaExhaustedWindows {
    pub fn is_exhausted(self) -> bool {
        !matches!(self, Self::None)
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Primary => "primary",
            Self::Secondary => "secondary",
            Self::Both => "both",
            Self::Unknown => "unknown",
        }
    }
}

impl TryFrom<&str> for QuotaExhaustedWindows {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> std::result::Result<Self, Self::Error> {
        match value {
            "none" => Ok(Self::None),
            "primary" => Ok(Self::Primary),
            "secondary" => Ok(Self::Secondary),
            "both" => Ok(Self::Both),
            "unknown" => Ok(Self::Unknown),
            other => Err(anyhow::anyhow!("unknown quota exhausted windows: {other}")),
        }
    }
}

/// Most recent coordinated reprobe outcome for one family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaProbeResult {
    Success,
    StillBlocked,
    Ambiguous,
}

impl QuotaProbeResult {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::StillBlocked => "still_blocked",
            Self::Ambiguous => "ambiguous",
        }
    }
}

impl TryFrom<&str> for QuotaProbeResult {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> std::result::Result<Self, Self::Error> {
        match value {
            "success" => Ok(Self::Success),
            "still_blocked" => Ok(Self::StillBlocked),
            "ambiguous" => Ok(Self::Ambiguous),
            other => Err(anyhow::anyhow!("unknown quota probe result: {other}")),
        }
    }
}
