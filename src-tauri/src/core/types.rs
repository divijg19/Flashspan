use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
pub struct ClearScreen {
    pub session_id: u64,
    /// When set, indicates which flashed number index is being cleared.
    /// When None, represents a global clear (e.g. session start/stop/complete).
    pub index: Option<u32>,
    pub emitted_at_ms: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionComplete {
    pub session_id: u64,
    pub numbers: Vec<i64>,
    pub sum: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ShowNumber {
    pub session_id: u64,
    pub index: u32,
    pub total: u32,
    pub value: i64,
    pub running_sum: i64,
    pub emitted_at_ms: u64,
}

#[derive(Debug, Clone)]
pub struct AutoRepeatPlan {
    pub remaining: u32,
    pub delay_ms: u64,
    pub config: SessionConfig,
    pub awaiting_validation_session_id: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfigInput {
    pub digits_per_number: i64,
    pub number_duration_s: f64,
    pub total_numbers: i64,

    #[serde(default)]
    pub allow_negative_numbers: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionConfigEffective {
    pub digits_per_number: u32,
    pub number_duration_s: f64,
    pub total_numbers: u32,
    pub allow_negative_numbers: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    pub digits_per_number: u32,
    pub number_duration_ms: u64,
    pub total_numbers: u32,
    pub allow_negative_numbers: bool,
}

/// A single step in a deterministic session plan.
/// Each step includes the action to perform and the relative delay (ms) before the next step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SessionStep {
    /// Countdown tick: emit a numeric value ("3", "2", "1")
    CountdownTick {
        value: String,
        delay_ms_before_next: u64,
    },

    /// Show a number: display the flashed value
    ShowNumber {
        session_id: u64,
        index: u32,
        total: u32,
        value: i64,
        running_sum: i64,
        delay_ms_before_next: u64,
    },

    /// Clear the screen: hide the current display
    ClearScreen {
        session_id: u64,
        index: Option<u32>,
        delay_ms_before_next: u64,
    },

    /// Session complete: emit final results and numbers
    Complete {
        session_id: u64,
        numbers: Vec<i64>,
        sum: i64,
    },
}

impl SessionStep {
    /// The delay this step imposes before the next one (ms).
    ///
    /// Single source of truth for a plan's `total_duration_ms`: derived by
    /// summing over the steps themselves, so the advertised total cannot drift
    /// from the schedule that is actually executed.
    #[must_use]
    pub fn delay_ms(&self) -> u64 {
        match self {
            Self::CountdownTick {
                delay_ms_before_next,
                ..
            }
            | Self::ShowNumber {
                delay_ms_before_next,
                ..
            }
            | Self::ClearScreen {
                delay_ms_before_next,
                ..
            } => *delay_ms_before_next,
            Self::Complete { .. } => 0,
        }
    }
}

/// An immutable snapshot of a complete session progression.
/// Core generates this deterministically; runtime executes it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionPlan {
    pub session_id: u64,
    pub config_snapshot: SessionConfigEffective,
    pub steps: Vec<SessionStep>,
    pub total_duration_ms: u64,
    pub numbers_generated: Vec<i64>,
    pub expected_sum: i64,
}
