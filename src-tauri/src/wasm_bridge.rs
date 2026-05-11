use crate::core::engine::build_session_plan;
use crate::core::types::{SessionConfigEffective, SessionConfigInput};
use crate::core::validate::normalize_session_config;
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmNormalizedSessionConfig {
    pub config: crate::core::types::SessionConfig,
    pub effective: SessionConfigEffective,
}

fn js_error(message: impl Into<String>) -> JsValue {
    JsValue::from_str(&message.into())
}

#[wasm_bindgen]
pub fn ping() -> String {
    "pong (wasm)".to_string()
}

#[wasm_bindgen]
pub fn normalize_session_config_wasm(input: JsValue) -> Result<JsValue, JsValue> {
    let input: SessionConfigInput = serde_wasm_bindgen::from_value(input)
        .map_err(|err| js_error(format!("failed to decode SessionConfigInput: {err}")))?;
    let (config, effective) = normalize_session_config(input);
    let output = WasmNormalizedSessionConfig { config, effective };
    serde_wasm_bindgen::to_value(&output)
        .map_err(|err| js_error(format!("failed to encode normalized config: {err}")))
}

#[wasm_bindgen]
pub fn build_session_plan_wasm(
    session_id: u64,
    input: JsValue,
    seed: Option<u64>,
) -> Result<JsValue, JsValue> {
    let input: SessionConfigInput = serde_wasm_bindgen::from_value(input)
        .map_err(|err| js_error(format!("failed to decode SessionConfigInput: {err}")))?;
    let (config, effective) = normalize_session_config(input);
    let plan = build_session_plan(session_id, config, effective, seed);
    serde_wasm_bindgen::to_value(&plan)
        .map_err(|err| js_error(format!("failed to encode SessionPlan: {err}")))
}

#[cfg(all(test, target_arch = "wasm32"))]
mod tests {
    use super::*;
    use crate::core::types::{SessionConfigInput, SessionPlan};
    use wasm_bindgen_test::*;

    #[wasm_bindgen_test]
    fn ping_reports_wasm_bridge() {
        assert_eq!(ping(), "pong (wasm)");
    }

    #[wasm_bindgen_test]
    fn build_session_plan_wasm_round_trips() {
        let input = SessionConfigInput {
            digits_per_number: 2,
            number_duration_s: 0.5,
            delay_between_numbers_s: 0.2,
            total_numbers: 4,
            allow_negative_numbers: true,
        };

        let input_value = serde_wasm_bindgen::to_value(&input).expect("encode input");
        let plan_value =
            build_session_plan_wasm(42, input_value, Some(1234)).expect("plan should serialize");
        let plan: SessionPlan = serde_wasm_bindgen::from_value(plan_value).expect("decode plan");

        assert_eq!(plan.session_id, 42);
        assert_eq!(plan.numbers_generated.len(), 4);
        assert_eq!(
            plan.expected_sum,
            plan.numbers_generated.iter().sum::<i64>()
        );
    }
}
