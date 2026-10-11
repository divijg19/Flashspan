#[cfg(all(test, target_arch = "wasm32"))]
mod wasm_tests {
    use crate::core::types::{SessionConfigEffective, SessionConfigInput, SessionPlan};
    use crate::{build_session_plan_wasm, normalize_session_config_wasm, ping, wasm_version};
    use serde::{Deserialize, Serialize};
    use serde_wasm_bindgen::{from_value, to_value};
    use wasm_bindgen::JsValue;
    use wasm_bindgen_test::*;

    /// The normalized-config shape the bridge returns to JS.
    #[derive(Debug, Deserialize)]
    struct NormalizedConfig {
        effective: SessionConfigEffective,
    }

    fn normalize(input: &SessionConfigInput) -> Result<SessionConfigEffective, ()> {
        let value = normalize_session_config_wasm(to_value(input).expect("encode input"))
            .map_err(|_| ())?;
        from_value::<NormalizedConfig>(value)
            .map(|decoded| decoded.effective)
            .map_err(|_| ())
    }

    fn plan(session_id: u64, input: &SessionConfigInput, seed: Option<u64>) -> SessionPlan {
        let value =
            build_session_plan_wasm(session_id, to_value(input).expect("encode input"), seed)
                .expect("plan generation should succeed");
        from_value::<SessionPlan>(value).expect("plan should decode")
    }

    #[wasm_bindgen_test]
    fn test_ping() {
        let result = ping();
        assert_eq!(result, "pong (wasm)");
    }

    #[wasm_bindgen_test]
    fn test_wasm_version() {
        let version = wasm_version();
        assert_eq!(
            version,
            env!("CARGO_PKG_VERSION"),
            "the bridge must report the crate version the JS bundle was built from"
        );
        // Reported as at least major.minor.
        assert!(
            version.split('.').count() >= 2,
            "version {version} should have a major.minor shape"
        );
    }

    #[wasm_bindgen_test]
    fn test_normalize_session_config_wasm_valid() {
        let input = SessionConfigInput {
            digits_per_number: 2,
            number_duration_s: 1.5,
            total_numbers: 10,
            allow_negative_numbers: false,
        };

        let effective = normalize(&input).expect("valid config should normalize");
        assert_eq!(effective.digits_per_number, 2);
        assert_eq!(effective.number_duration_s, 1.5);
        assert_eq!(effective.total_numbers, 10);
        assert!(!effective.allow_negative_numbers);
    }

    #[wasm_bindgen_test]
    fn test_normalize_session_config_wasm_boundary_digits() {
        // Zero digits must clamp up to the minimum, not pass through.
        let effective = normalize(&SessionConfigInput {
            digits_per_number: 0,
            number_duration_s: 1.0,
            total_numbers: 5,
            allow_negative_numbers: false,
        })
        .expect("zero digits should normalize");
        assert_eq!(effective.digits_per_number, 1);
    }

    #[wasm_bindgen_test]
    fn test_normalize_session_config_wasm_large_digits() {
        // Absurd widths clamp to the exact-integer policy cap of 15.
        let effective = normalize(&SessionConfigInput {
            digits_per_number: 100,
            number_duration_s: 1.0,
            total_numbers: 5,
            allow_negative_numbers: false,
        })
        .expect("large digits should clamp");
        assert_eq!(effective.digits_per_number, 15);
        // 5 is inside the bound for width 15, so the total is untouched.
        assert_eq!(effective.total_numbers, 5);

        // A total past the bound for the clamped width is capped to it.
        let capped = normalize(&SessionConfigInput {
            digits_per_number: 100,
            number_duration_s: 1.0,
            total_numbers: 5_000,
            allow_negative_numbers: false,
        })
        .expect("an oversized total should clamp");
        assert_eq!(capped.digits_per_number, 15);
        assert_eq!(
            capped.total_numbers,
            crate::core::validate::max_total_for_digits(15),
            "total must be capped by the clamped width's exact-integer bound"
        );
    }

    #[wasm_bindgen_test]
    fn test_normalize_session_config_wasm_duration_is_grid_aligned() {
        // The reported duration is exactly the exposure, on the 0.1s grid the
        // UI exposes (see the fixed-point property in core::prop_tests).
        let effective = normalize(&SessionConfigInput {
            digits_per_number: 2,
            number_duration_s: 0.25,
            total_numbers: 4,
            allow_negative_numbers: false,
        })
        .expect("duration should normalize");
        assert_eq!(effective.number_duration_s, 0.3);
    }

    #[wasm_bindgen_test]
    fn test_build_session_plan_wasm_basic() {
        let input = SessionConfigInput {
            digits_per_number: 2,
            number_duration_s: 1.0,
            total_numbers: 5,
            allow_negative_numbers: false,
        };

        let plan = plan(12345, &input, None);
        assert_eq!(plan.session_id, 12345);
        assert_eq!(plan.numbers_generated.len(), 5);
        assert_eq!(
            plan.expected_sum,
            plan.numbers_generated.iter().sum::<i64>(),
            "the plan must report the sum of its own numbers"
        );
        assert_eq!(
            plan.config_snapshot.digits_per_number, 2,
            "the plan carries the normalized config"
        );
    }

    #[wasm_bindgen_test]
    fn test_build_session_plan_wasm_with_seed_is_reproducible() {
        let input = SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 1.0,
            total_numbers: 3,
            allow_negative_numbers: false,
        };

        let first = plan(11111, &input, Some(42));
        let second = plan(11111, &input, Some(42));

        assert_eq!(
            first.numbers_generated, second.numbers_generated,
            "same seed must produce the same numbers"
        );
        assert_eq!(first.expected_sum, second.expected_sum);
        assert_eq!(
            first.total_duration_ms, second.total_duration_ms,
            "same seed must produce the same schedule"
        );
    }

    #[wasm_bindgen_test]
    fn test_build_session_plan_wasm_determinism() {
        let input = SessionConfigInput {
            digits_per_number: 2,
            number_duration_s: 1.0,
            total_numbers: 4,
            allow_negative_numbers: true,
        };

        // Three independent calls with the same config and seed.
        let first = plan(99999, &input, Some(999));
        let second = plan(99999, &input, Some(999));
        let third = plan(99999, &input, Some(999));

        assert_eq!(first.numbers_generated, second.numbers_generated);
        assert_eq!(second.numbers_generated, third.numbers_generated);
        assert_eq!(first.expected_sum, third.expected_sum);
    }

    #[wasm_bindgen_test]
    fn test_build_session_plan_wasm_different_seeds() {
        let input = SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 1.0,
            total_numbers: 2,
            allow_negative_numbers: false,
        };

        let first = plan(55555, &input, Some(1));
        let second = plan(55555, &input, Some(2));

        assert_ne!(
            first.numbers_generated, second.numbers_generated,
            "different seeds should produce different plans"
        );
    }

    #[wasm_bindgen_test]
    fn test_session_plan_step_count_and_timing() {
        let input = SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 1.0,
            total_numbers: 5,
            allow_negative_numbers: false,
        };

        let plan = plan(77777, &input, None);

        // 1 initial clear + 3 countdown ticks + 2 per number + 1 final clear
        // + 1 complete.
        assert_eq!(plan.steps.len(), 1 + 3 + (2 * 5) + 1 + 1);
        assert_eq!(plan.total_duration_ms, 3_000 + 100 + 5 * (1_000 + 100));
    }

    /// Malformed input must surface as a thrown value, never a wasm trap. A
    /// trap silently abandons the bridge (and, worse, is uncaught across the
    /// async boundary), so every decode failure must stay an `Err`.
    #[wasm_bindgen_test]
    fn wasm_normalize_rejects_null_and_missing_fields() {
        assert!(
            normalize_session_config_wasm(JsValue::NULL).is_err(),
            "null must not decode as a config"
        );

        let missing = to_value(&serde_json::json!({"digits_per_number": 2})).expect("encode");
        assert!(
            normalize_session_config_wasm(missing).is_err(),
            "missing required fields must be an Err"
        );
    }

    #[wasm_bindgen_test]
    fn wasm_normalize_rejects_wrong_types() {
        let wrong_duration = to_value(&serde_json::json!({
            "digits_per_number": 2,
            "number_duration_s": "1.5",
            "total_numbers": 5,
            "allow_negative_numbers": false,
        }))
        .expect("encode");
        assert!(normalize_session_config_wasm(wrong_duration).is_err());

        let wrong_total = to_value(&serde_json::json!({
            "digits_per_number": 2,
            "number_duration_s": 1.5,
            "total_numbers": "10",
            "allow_negative_numbers": false,
        }))
        .expect("encode");
        assert!(normalize_session_config_wasm(wrong_total).is_err());
    }

    /**
     * Wrong *types* are covered above. The remaining decode risk is a number
     * that cannot fit an `i64`: it must surface as an `Err` the browser can
     * catch, never a wasm trap, which would abandon the bridge uncaught across
     * the async boundary.
     *
     * NaN and ±Infinity are not tested here: `serde_json::Value` cannot carry
     * them (they serialize as null), so they cannot cross this boundary. Their
     * clamping is covered on the host in `core/prop_tests.rs`
     * (`nan_duration_clamps_to_min`, `infinity_duration_clamps_to_max`), and the
     * wasm path delegates to that same function.
     */
    #[wasm_bindgen_test]
    fn wasm_normalize_rejects_numbers_that_overflow_i64() {
        // 1e300 is a valid finite f64 but far beyond i64, so the *field* decode
        // must fail rather than truncate.
        for oversized in [
            serde_json::json!({
                "digits_per_number": 1e300f64,
                "number_duration_s": 1.0,
                "total_numbers": 5,
                "allow_negative_numbers": false,
            }),
            serde_json::json!({
                "digits_per_number": 2,
                "number_duration_s": 1.0,
                "total_numbers": 1e300f64,
                "allow_negative_numbers": false,
            }),
        ] {
            let encoded = to_value(&oversized).expect("encode");
            assert!(
                normalize_session_config_wasm(encoded).is_err(),
                "an out-of-range number must be an Err, never a trap"
            );
        }
    }

    /// The `config` half of the normalize return value is never asserted on any
    /// target. No runtime code consumes it, so a rename would ship silently.
    #[wasm_bindgen_test]
    fn wasm_normalize_returns_the_raw_config_as_well_as_effective() {
        let value = normalize_session_config_wasm(
            to_value(&SessionConfigInput {
                digits_per_number: 2,
                number_duration_s: 1.5,
                total_numbers: 10,
                allow_negative_numbers: true,
            })
            .expect("encode"),
        )
        .expect("valid config should normalize");
        from_value::<NormalizedConfig>(value).expect("decode");
    }

    /// `ClearScreen { index: None }` is the one field whose shape differs between
    /// serializers: `serde-wasm-bindgen` emits `null` where `serde_json` emits
    /// `undefined`. The browser guards with `== null`, but nothing proved what the
    /// wasm side actually produces. Round-tripping through the real serializer is
    /// stronger than inspecting it with reflection.
    #[wasm_bindgen_test]
    fn wasm_clear_screen_index_none_round_trips() {
        #[derive(Debug, Deserialize, Serialize)]
        struct ClearScreenWire {
            session_id: u64,
            index: Option<u32>,
            delay_ms_before_next: u64,
        }
        #[derive(Debug, Deserialize, Serialize)]
        struct StepWire {
            #[serde(rename = "ClearScreen")]
            clear: ClearScreenWire,
        }

        let step = StepWire {
            clear: ClearScreenWire {
                session_id: 1,
                index: None,
                delay_ms_before_next: 0,
            },
        };
        let encoded = to_value(&step).expect("encode");
        let decoded: StepWire = from_value(encoded).expect("decode");
        assert_eq!(
            decoded.clear.index, None,
            "index: None must survive as None"
        );
        assert_eq!(decoded.clear.session_id, 1);
        assert_eq!(decoded.clear.delay_ms_before_next, 0);
    }
    #[wasm_bindgen_test]
    fn wasm_build_plan_rejects_non_config_input() {
        let array = to_value(&serde_json::json!(["not", "a", "config"])).expect("encode");
        assert!(build_session_plan_wasm(1, array, None).is_err());

        let string = to_value(&"just a string").expect("encode");
        assert!(build_session_plan_wasm(1, string, None).is_err());
    }
}
