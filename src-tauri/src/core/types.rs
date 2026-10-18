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

    use super::{SessionConfigEffective, SessionConfigInput, SessionPlan, SessionStep};

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

    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn delay_ms_reports_each_variant_delay_and_zero_for_complete() {
        // `delay_ms` is the single source of a plan's `total_duration_ms`, so
        // the value each variant contributes has to be pinned directly. A
        // regression that swapped two arms would keep the total's type (u64)
        // while silently rescheduling the session.
        assert_eq!(
            SessionStep::CountdownTick {
                value: "3".to_string(),
                delay_ms_before_next: 1000,
            }
            .delay_ms(),
            1000
        );
        assert_eq!(
            SessionStep::ShowNumber {
                session_id: 1,
                index: 0,
                total: 1,
                value: 7,
                running_sum: 7,
                delay_ms_before_next: 350,
            }
            .delay_ms(),
            350
        );
        assert_eq!(
            SessionStep::ClearScreen {
                session_id: 1,
                index: Some(0),
                delay_ms_before_next: 100,
            }
            .delay_ms(),
            100
        );
        // `Complete` carries no delay field; its delay is defined as zero.
        assert_eq!(
            SessionStep::Complete {
                session_id: 1,
                numbers: vec![],
                sum: 0,
            }
            .delay_ms(),
            0
        );
    }

    /// The *decode* direction of the wire format. The browser consumes the plan
    /// (and the wasm bridge decodes it back), so the externally-tagged shape has
    /// to round-trip as well as serialize — a rename that only fixed the
    /// serializer would fail here.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn each_step_variant_decodes_from_its_externally_tagged_json() {
        let countdown: SessionStep = serde_json::from_value(serde_json::json!({
            "CountdownTick": { "value": "2", "delay_ms_before_next": 1000 },
        }))
        .expect("countdown tick");
        match countdown {
            SessionStep::CountdownTick {
                value,
                delay_ms_before_next,
            } => {
                assert_eq!(value, "2");
                assert_eq!(delay_ms_before_next, 1000);
            }
            other => panic!("expected CountdownTick, got {other:?}"),
        }

        let show: SessionStep = serde_json::from_value(serde_json::json!({
            "ShowNumber": {
                "session_id": 7,
                "index": 3,
                "total": 10,
                "value": -42,
                "running_sum": 12,
                "delay_ms_before_next": 500,
            },
        }))
        .expect("show number");
        match show {
            SessionStep::ShowNumber {
                session_id,
                index,
                total,
                value,
                running_sum,
                delay_ms_before_next,
            } => {
                assert_eq!((session_id, index, total), (7, 3, 10));
                assert_eq!((value, running_sum, delay_ms_before_next), (-42, 12, 500));
            }
            other => panic!("expected ShowNumber, got {other:?}"),
        }

        // The global clear arrives as an explicit `null`, not a missing key.
        let clear: SessionStep = serde_json::from_value(serde_json::json!({
            "ClearScreen": { "session_id": 1, "index": null, "delay_ms_before_next": 0 },
        }))
        .expect("global clear");
        match clear {
            SessionStep::ClearScreen {
                session_id,
                index,
                delay_ms_before_next,
            } => {
                assert_eq!(session_id, 1);
                assert_eq!(index, None);
                assert_eq!(delay_ms_before_next, 0);
            }
            other => panic!("expected ClearScreen, got {other:?}"),
        }

        let complete: SessionStep = serde_json::from_value(serde_json::json!({
            "Complete": { "session_id": 9, "numbers": [1, -2, 3], "sum": 2 },
        }))
        .expect("complete");
        match complete {
            SessionStep::Complete {
                session_id,
                numbers,
                sum,
            } => {
                assert_eq!(session_id, 9);
                assert_eq!(numbers, vec![1, -2, 3]);
                assert_eq!(sum, 2);
            }
            other => panic!("expected Complete, got {other:?}"),
        }
    }

    /// An unknown tag must be an `Err`, not a silent decode into some other
    /// variant: a driver that mistyped a tag would otherwise run a wrong step.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn unknown_step_tag_is_rejected() {
        assert!(
            serde_json::from_value::<SessionStep>(serde_json::json!({
                "CountdownTicks": { "value": "3", "delay_ms_before_next": 1000 },
            }))
            .is_err(),
            "a near-miss tag must not decode",
        );
        // A bare object with no variant tag is not a step either.
        assert!(
            serde_json::from_value::<SessionStep>(serde_json::json!({ "value": "3" })).is_err(),
            "an untagged object must not decode",
        );
    }

    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn plan_round_trips_through_json() {
        // A plan is both emitted (native/browser) and read back (the wasm test
        // bridge), so the whole document has to survive a round-trip.
        let plan = SessionPlan {
            session_id: 9,
            config_snapshot: SessionConfigEffective {
                digits_per_number: 2,
                number_duration_s: 0.5,
                total_numbers: 2,
                allow_negative_numbers: true,
            },
            steps: vec![
                SessionStep::CountdownTick {
                    value: "3".to_string(),
                    delay_ms_before_next: 1000,
                },
                SessionStep::Complete {
                    session_id: 9,
                    numbers: vec![12, -3],
                    sum: 9,
                },
            ],
            total_duration_ms: 1000,
            numbers_generated: vec![12, -3],
            expected_sum: 9,
        };

        let json = serde_json::to_value(&plan).expect("serialize");
        let decoded: SessionPlan = serde_json::from_value(json).expect("deserialize");

        assert_eq!(decoded.session_id, 9);
        assert_eq!(decoded.total_duration_ms, 1000);
        assert_eq!(decoded.numbers_generated, vec![12, -3]);
        assert_eq!(decoded.expected_sum, 9);
        assert_eq!(decoded.config_snapshot, plan.config_snapshot);
        assert_eq!(decoded.steps.len(), 2);
        // The derived equality is checked field-by-field above; the steps
        // themselves are decoded again here to prove the sequence survives.
        match &decoded.steps[1] {
            SessionStep::Complete { numbers, sum, .. } => {
                assert_eq!(numbers, &vec![12, -3]);
                assert_eq!(*sum, 9);
            }
            other => panic!("expected the Complete step last, got {other:?}"),
        }
    }

    /// The UI may omit `allow_negative_numbers` (it is `#[serde(default)]`), and
    /// an absent flag must mean "off", never a decode failure. This is the IPC
    /// shape `runtime/native.ts` builds.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn session_config_input_defaults_the_negative_flag_when_absent() {
        let without: SessionConfigInput = serde_json::from_value(serde_json::json!({
            "digits_per_number": 2,
            "number_duration_s": 1.5,
            "total_numbers": 5,
        }))
        .expect("the flag is optional");
        assert!(!without.allow_negative_numbers, "absent must mean off");
        assert_eq!(without.digits_per_number, 2);
        assert_eq!(without.total_numbers, 5);

        let with: SessionConfigInput = serde_json::from_value(serde_json::json!({
            "digits_per_number": 2,
            "number_duration_s": 1.5,
            "total_numbers": 5,
            "allow_negative_numbers": true,
        }))
        .expect("explicit flag");
        assert!(with.allow_negative_numbers, "present must be honoured");
    }
}

/// The standalone event payloads: the structs the desktop emits over Tauri
/// (`show_number`, `clear_screen`, `session_complete`), which the browser mirrors
/// in its own runtime. `SessionStep`'s schema is locked above, but these are a
/// *separate* wire contract and were entirely unpinned — a renamed field would
/// reach the UI as `undefined` on desktop only, while the web path kept working.
///
/// They are `Serialize`-only (nothing decodes them), so only the emitted shape
/// is a contract.
#[cfg(test)]
mod payload_schema_tests {
    #[cfg(target_arch = "wasm32")]
    use wasm_bindgen_test::wasm_bindgen_test;

    use super::{ClearScreen, SessionComplete, ShowNumber};

    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn show_number_emits_with_the_frontend_keys() {
        let json = serde_json::to_value(ShowNumber {
            session_id: 7,
            index: 3,
            total: 10,
            value: -42,
            running_sum: 12,
            emitted_at_ms: 1_700_000_000_000,
        })
        .expect("serialize show_number");
        assert_eq!(
            json,
            serde_json::json!({
                "session_id": 7,
                "index": 3,
                "total": 10,
                "value": -42,
                "running_sum": 12,
                "emitted_at_ms": 1_700_000_000_000u64,
            }),
        );
    }

    /// A global clear (session start/stop/complete) carries `index: null`; the
    /// UI distinguishes it from a per-number clear by exactly that. A missing key
    /// would read as `undefined` too, so pin the explicit `null`.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn clear_screen_emits_null_index_for_a_global_clear() {
        let global = serde_json::to_value(ClearScreen {
            session_id: 1,
            index: None,
            emitted_at_ms: 5,
        })
        .expect("serialize global clear");
        assert_eq!(
            global,
            serde_json::json!({ "session_id": 1, "index": null, "emitted_at_ms": 5 }),
        );

        let indexed = serde_json::to_value(ClearScreen {
            session_id: 1,
            index: Some(4),
            emitted_at_ms: 6,
        })
        .expect("serialize indexed clear");
        assert_eq!(
            indexed,
            serde_json::json!({ "session_id": 1, "index": 4, "emitted_at_ms": 6 }),
        );
    }

    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn session_complete_emits_the_numbers_in_order_and_their_sum() {
        let json = serde_json::to_value(SessionComplete {
            session_id: 9,
            numbers: vec![12, -3, 5],
            sum: 14,
        })
        .expect("serialize session_complete");
        assert_eq!(
            json,
            serde_json::json!({ "session_id": 9, "numbers": [12, -3, 5], "sum": 14 }),
        );

        // A 'zero-number' session (only reachable by building a plan directly)
        // still emits both keys, with an empty array rather than an absent one.
        let empty = serde_json::to_value(SessionComplete {
            session_id: 1,
            numbers: vec![],
            sum: 0,
        })
        .expect("serialize empty completion");
        assert_eq!(
            empty,
            serde_json::json!({ "session_id": 1, "numbers": [], "sum": 0 }),
        );
    }

    /// Normalization bounds every flashed value and the sum below `2^53`, which
    /// is what lets the browser read them as exact `Number`s. Pin that the
    /// serializer itself is exact right up to that budget, so the limit is the
    /// app's policy and not the wire format.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn event_payloads_carry_the_exact_integer_budget_without_truncation() {
        let budget =
            i64::try_from(crate::core::validate::MAX_EXACT_INTEGER).expect("2^53 - 1 fits i64");

        let json = serde_json::to_value(ShowNumber {
            session_id: 1,
            index: 1,
            total: 1,
            value: budget,
            running_sum: budget,
            emitted_at_ms: 0,
        })
        .expect("serialize");
        assert_eq!(json["value"], serde_json::json!(budget));
        assert_eq!(json["running_sum"], serde_json::json!(budget));

        let negative = serde_json::to_value(SessionComplete {
            session_id: 1,
            numbers: vec![-budget, budget],
            sum: 0,
        })
        .expect("serialize");
        assert_eq!(
            negative["numbers"],
            serde_json::json!([-budget, budget]),
            "the signed extremes must survive the serializer exactly",
        );
    }
}
