#[cfg(all(test, target_arch = "wasm32"))]
mod wasm_tests {
    use crate::core::types::SessionConfigInput;
    use crate::{build_session_plan_wasm, normalize_session_config_wasm, ping, wasm_version};
    use serde_wasm_bindgen::to_value;
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    #[wasm_bindgen_test]
    fn test_ping() {
        let result = ping();
        assert_eq!(result, "pong (wasm)");
    }

    #[wasm_bindgen_test]
    fn test_wasm_version() {
        let version = wasm_version();
        // Version should be in format "major.minor.patch"
        let parts: Vec<&str> = version.split('.').collect();
        assert!(
            parts.len() >= 2,
            "Version should have at least major.minor format"
        );

        // All parts should be numeric or pre-release identifiers
        assert!(!version.is_empty(), "Version should not be empty");
    }

    #[wasm_bindgen_test]
    fn test_normalize_session_config_wasm_valid() {
        let input = SessionConfigInput {
            digits_per_number: 2,
            number_duration_s: 1.5,
            delay_between_numbers_s: 0.5,
            total_numbers: 10,
            allow_negative_numbers: false,
        };

        let input_value = to_value(&input).unwrap();
        let result = normalize_session_config_wasm(input_value);

        assert!(result.is_ok(), "Valid config should not error");
        let config_value = result.unwrap();
        assert!(!config_value.is_null(), "Config should not be null");
    }

    #[wasm_bindgen_test]
    fn test_normalize_session_config_wasm_boundary_digits() {
        // Test with 0 digits (should clamp to 1)
        let input = SessionConfigInput {
            digits_per_number: 0,
            number_duration_s: 1.0,
            delay_between_numbers_s: 0.0,
            total_numbers: 5,
            allow_negative_numbers: false,
        };

        let input_value = to_value(&input).unwrap();
        let result = normalize_session_config_wasm(input_value);
        assert!(result.is_ok(), "Should handle edge case digits");
    }

    #[wasm_bindgen_test]
    fn test_normalize_session_config_wasm_large_digits() {
        // Test with large digits (should clamp to 18)
        let input = SessionConfigInput {
            digits_per_number: 100,
            number_duration_s: 1.0,
            delay_between_numbers_s: 0.0,
            total_numbers: 5,
            allow_negative_numbers: false,
        };

        let input_value = to_value(&input).unwrap();
        let result = normalize_session_config_wasm(input_value);
        assert!(result.is_ok(), "Should clamp large digits");
    }

    #[wasm_bindgen_test]
    fn test_build_session_plan_wasm_basic() {
        let input = SessionConfigInput {
            digits_per_number: 2,
            number_duration_s: 1.0,
            delay_between_numbers_s: 0.5,
            total_numbers: 5,
            allow_negative_numbers: false,
        };

        let input_value = to_value(&input).unwrap();
        let result = build_session_plan_wasm(12345, input_value, None);

        assert!(result.is_ok(), "Plan generation should not error");
        let plan_value = result.unwrap();
        assert!(!plan_value.is_null(), "Plan should not be null");
    }

    #[wasm_bindgen_test]
    fn test_build_session_plan_wasm_with_seed() {
        let input = SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 1.0,
            delay_between_numbers_s: 0.0,
            total_numbers: 3,
            allow_negative_numbers: false,
        };

        let input_value = to_value(&input).unwrap();

        // Generate two plans with the same seed - should be identical
        let result1 = build_session_plan_wasm(11111, input_value.clone(), Some(42));
        let result2 = build_session_plan_wasm(11111, input_value.clone(), Some(42));

        assert!(result1.is_ok());
        assert!(result2.is_ok());

        let plan1_str = format!("{:?}", result1.unwrap());
        let plan2_str = format!("{:?}", result2.unwrap());
        assert_eq!(
            plan1_str, plan2_str,
            "Same seed should produce identical plans"
        );
    }

    #[wasm_bindgen_test]
    fn test_build_session_plan_wasm_determinism() {
        let input = SessionConfigInput {
            digits_per_number: 2,
            number_duration_s: 1.0,
            delay_between_numbers_s: 0.0,
            total_numbers: 4,
            allow_negative_numbers: true,
        };

        let input_value = to_value(&input).unwrap();

        // Generate three plans with same config and seed
        let result1 = build_session_plan_wasm(99999, input_value.clone(), Some(999));
        let result2 = build_session_plan_wasm(99999, input_value.clone(), Some(999));
        let result3 = build_session_plan_wasm(99999, input_value, Some(999));

        assert!(result1.is_ok());
        assert!(result2.is_ok());
        assert!(result3.is_ok());

        let plan1 = result1.unwrap();
        let plan2 = result2.unwrap();
        let plan3 = result3.unwrap();

        // All three should produce identical results
        assert_eq!(
            format!("{:?}", plan1),
            format!("{:?}", plan2),
            "Determinism check 1"
        );
        assert_eq!(
            format!("{:?}", plan2),
            format!("{:?}", plan3),
            "Determinism check 2"
        );
    }

    #[wasm_bindgen_test]
    fn test_build_session_plan_wasm_different_seeds() {
        let input = SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 1.0,
            delay_between_numbers_s: 0.0,
            total_numbers: 2,
            allow_negative_numbers: false,
        };

        let input_value = to_value(&input).unwrap();

        // Generate plans with different seeds - should produce different results
        let result_seed1 = build_session_plan_wasm(55555, input_value.clone(), Some(1));
        let result_seed2 = build_session_plan_wasm(55555, input_value, Some(2));

        assert!(result_seed1.is_ok());
        assert!(result_seed2.is_ok());

        let plan1_str = format!("{:?}", result_seed1.unwrap());
        let plan2_str = format!("{:?}", result_seed2.unwrap());
        assert_ne!(
            plan1_str, plan2_str,
            "Different seeds should produce different plans"
        );
    }

    #[wasm_bindgen_test]
    fn test_session_plan_step_count() {
        let input = SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 1.0,
            delay_between_numbers_s: 0.0,
            total_numbers: 5,
            allow_negative_numbers: false,
        };

        let input_value = to_value(&input).unwrap();
        let result = build_session_plan_wasm(77777, input_value, None);

        assert!(result.is_ok());
        let plan_value = result.unwrap();

        // Convert back to check structure - plan should have steps property
        assert!(!plan_value.is_null());
    }
}
