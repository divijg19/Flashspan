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
