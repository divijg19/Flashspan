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

// ---- Schema lock for the wire format --------------------------------------
//
// `SessionStep` crosses the wasm boundary into the browser (and into JSON in
// some debug paths), so its exact externally-tagged shape is a contract, not
// an implementation detail. The browser's driver validates against these
// assumptions and silently falls back to the JS planner when they change:
// renaming a variant, renaming a field, or adding `delay_ms_before_next` to
// `Complete` would break every web session without a test failing. These
// assertions are the Rust-side anchor for that contract.

#[cfg(test)]
mod schema_tests {
    // These lock the wire format the browser consumes, so they have to run on
    // the wasm target too. A plain `#[test]` compiles there but is silently
    // skipped by the wasm-bindgen test runner — which is how the schema went
    // unverified on the one platform that depends on it.
    #[cfg(target_arch = "wasm32")]
    use wasm_bindgen_test::wasm_bindgen_test;

    use super::{SessionPlan, SessionStep};

    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn countdown_tick_serializes_with_stable_keys() {
        let json = serde_json::to_value(SessionStep::CountdownTick {
            value: "3".to_string(),
            delay_ms_before_next: 1000,
        })
        .expect("serialize");
        assert_eq!(
            json,
            serde_json::json!({
                "CountdownTick": { "value": "3", "delay_ms_before_next": 1000 },
            })
        );
    }

    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn show_number_serializes_with_stable_keys() {
        let json = serde_json::to_value(SessionStep::ShowNumber {
            session_id: 7,
            index: 3,
            total: 10,
            value: 42,
            running_sum: 100,
            delay_ms_before_next: 500,
        })
        .expect("serialize");
        assert_eq!(
            json,
            serde_json::json!({
                "ShowNumber": {
                    "session_id": 7,
                    "index": 3,
                    "total": 10,
                    "value": 42,
                    "running_sum": 100,
                    "delay_ms_before_next": 500,
                },
            })
        );
    }

    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn clear_screen_index_is_null_when_absent() {
        // `index` is `Option<u32>`; as a JSON null here and as `undefined`
        // through serde-wasm-bindgen, the browser must treat the initial and
        // trailing clears as global (no index), never as clearing a number.
        let json = serde_json::to_value(SessionStep::ClearScreen {
            session_id: 1,
            index: None,
            delay_ms_before_next: 0,
        })
        .expect("serialize");
        assert_eq!(
            json,
            serde_json::json!({
                "ClearScreen": { "session_id": 1, "index": null, "delay_ms_before_next": 0 },
            })
        );
    }

    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn complete_step_has_no_delay_key() {
        // Unlike the other variants, `Complete` carries no `delay_ms_before_next`;
        // its delay is defined as 0 by `delay_ms()`. A consumer reading the key
        // finds `null`/`undefined` and must treat it as zero.
        let json = serde_json::to_value(SessionStep::ClearScreen {
            session_id: 1,
            index: Some(4),
            delay_ms_before_next: 100,
        })
        .expect("serialize");
        let complete = serde_json::to_value(SessionStep::Complete {
            session_id: 1,
            numbers: vec![1, 2, 3],
            sum: 6,
        })
        .expect("serialize");
        let clear_inner = json["ClearScreen"].as_object().expect("object");
        assert!(clear_inner.contains_key("delay_ms_before_next"));
        let complete_inner = complete["Complete"].as_object().expect("object");
        assert!(
            !complete_inner.contains_key("delay_ms_before_next"),
            "Complete must not carry delay_ms_before_next; delay_ms() maps it to 0"
        );
        assert_eq!(complete_inner["sum"], serde_json::json!(6));
    }

    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn plan_serializes_with_typed_numeric_fields() {
        // Every crosser of these is a JS `number`, not a BigInt: serde-wasm-
        // bindgen defaults to non-bigint for large integers, and the bounded
        // `i64` sums stay within f64 precision, so no JSON number becomes a
        // BigInt or a float.
        let plan = SessionPlan {
            session_id: 9,
            config_snapshot: super::SessionConfigEffective {
                digits_per_number: 2,
                number_duration_s: 0.5,
                total_numbers: 3,
                allow_negative_numbers: true,
            },
            steps: vec![],
            total_duration_ms: 0,
            numbers_generated: vec![12, -3, 5],
            expected_sum: 14,
        };
        let json = serde_json::to_value(&plan).expect("serialize");
        assert_eq!(json["session_id"], serde_json::json!(9));
        assert_eq!(json["expected_sum"], serde_json::json!(14));
        assert_eq!(json["numbers_generated"], serde_json::json!([12, -3, 5]));
    }
}
