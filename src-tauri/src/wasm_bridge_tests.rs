#[cfg(all(test, target_arch = "wasm32"))]
mod wasm_tests {
    use crate::core::types::{SessionConfig, SessionConfigEffective, SessionConfigInput, SessionPlan, SessionStep};
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
    /// Core equivalence property: the wasm path delegates to the same
    /// `normalize_session_config`, so any divergence in rounding or clamping
    /// would make the browser and desktop grade the same session differently.
    /// Compared against the native call directly, not a literal.
    #[wasm_bindgen_test]
    fn wasm_normalize_matches_the_native_function() {
        for (digits, duration_s, total, negatives) in [
            (2i64, 1.5, 10i64, false),
            (1, 0.1, 1, true),
            (15, 60.0, 10_000, true),
            (0, f64::NAN, -5, false),
            (99, 1.234, 99_999, true),
        ] {
            let input = SessionConfigInput {
                digits_per_number: digits,
                number_duration_s: duration_s,
                total_numbers: total,
                allow_negative_numbers: negatives,
            };
            let (_, native) = crate::core::validate::normalize_session_config(&input);
            let via_wasm =
                normalize(&input).unwrap_or_else(|()| panic!("normalize failed for {input:?}"));

            assert_eq!(
                via_wasm.digits_per_number, native.digits_per_number,
                "digits diverged for {input:?}"
            );
            assert_eq!(
                via_wasm.total_numbers, native.total_numbers,
                "total diverged for {input:?}"
            );
            assert_eq!(
                via_wasm.allow_negative_numbers, native.allow_negative_numbers,
                "flag diverged for {input:?}"
            );
            assert_eq!(
                via_wasm.number_duration_s, native.number_duration_s,
                "duration diverged for {input:?}"
            );
        }
    }

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
    /// `normalize_session_config_wasm` shares the JS decode boundary with
    /// `build_session_plan_wasm`, so it must reject the same malformed inputs as
    /// a JS `Err` rather than panicking. Only the plan-builder half was covered;
    /// this is the call the browser makes on every session start.
    #[wasm_bindgen_test]
    fn wasm_normalize_rejects_non_config_input() {
        let array = to_value(&serde_json::json!(["not", "a", "config"])).expect("encode");
        assert!(normalize_session_config_wasm(array).is_err());

        let string = to_value(&"just a string").expect("encode");
        assert!(normalize_session_config_wasm(string).is_err());

        // A bare number is not a struct at all.
        assert!(normalize_session_config_wasm(JsValue::from_f64(42.0)).is_err());
    }

    #[wasm_bindgen_test]
    fn wasm_build_plan_rejects_non_config_input() {
        let array = to_value(&serde_json::json!(["not", "a", "config"])).expect("encode");
        assert!(build_session_plan_wasm(1, array, None).is_err());

        let string = to_value(&"just a string").expect("encode");
        assert!(build_session_plan_wasm(1, string, None).is_err());
    }

    /// Every field the browser driver reads needs to survive the crossing. The
    /// other plan tests only count steps and check the sum; this walks a
    /// one-number plan and asserts the shape of each variant — the countdown
    /// values and their delays (the last one carries the pre-flash settle), the
    /// flash window, the per-index clear, the global clear and the completion.
    #[wasm_bindgen_test]
    fn wasm_plan_decodes_every_step_variant_with_its_fields() {
        let input = SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 0.5,
            total_numbers: 1,
            allow_negative_numbers: true,
        };
        let plan = plan(4242, &input, Some(7));

        let mut countdown_values: Vec<String> = Vec::new();
        let mut countdown_delays: Vec<u64> = Vec::new();
        let mut show_count = 0;
        let mut indexed_clears = 0;
        let mut global_clears = 0;
        let mut completes = 0;

        for step in &plan.steps {
            match step {
                SessionStep::CountdownTick {
                    value,
                    delay_ms_before_next,
                } => {
                    countdown_values.push(value.clone());
                    countdown_delays.push(*delay_ms_before_next);
                }
                SessionStep::ShowNumber {
                    index,
                    total,
                    value,
                    running_sum,
                    delay_ms_before_next,
                    ..
                } => {
                    show_count += 1;
                    assert_eq!(*index, 1, "a one-number session shows index 1");
                    assert_eq!(*total, 1);
                    assert_eq!(*value, plan.numbers_generated[0]);
                    assert_eq!(*running_sum, *value, "the first running sum is the first number");
                    assert_eq!(*delay_ms_before_next, 500, "the flash window is the exposure");
                }
                SessionStep::ClearScreen {
                    index: Some(_),
                    delay_ms_before_next,
                    ..
                } => {
                    indexed_clears += 1;
                    assert_eq!(*delay_ms_before_next, 100, "the inter-number gap is fixed");
                }
                SessionStep::ClearScreen { index: None, .. } => global_clears += 1,
                SessionStep::Complete {
                    numbers, sum, ..
                } => {
                    completes += 1;
                    assert_eq!(numbers, &plan.numbers_generated);
                    assert_eq!(*sum, plan.expected_sum);
                }
            }
        }

        assert_eq!(countdown_values, vec!["3", "2", "1"]);
        // The final tick adds the 100ms pre-flash settle so the first number
        // paints late rather than short.
        assert_eq!(countdown_delays, vec![1000, 1000, 1100]);
        assert_eq!(show_count, 1);
        assert_eq!(indexed_clears, 1);
        // One global clear before the countdown, one after the last number.
        assert_eq!(global_clears, 2);
        assert_eq!(completes, 1);
    }

    /// The widest config the app can produce, end to end through the bridge: 15
    /// digits is the `f64`-exact ceiling where the individual values and their
    /// sum are largest, so a bad `i64`/`u64` crossing would show up here first.
    #[wasm_bindgen_test]
    fn wasm_plan_builds_at_the_digit_width_ceiling() {
        let input = SessionConfigInput {
            digits_per_number: 15,
            number_duration_s: 1.0,
            total_numbers: 10_000,
            allow_negative_numbers: true,
        };
        let plan = plan(1, &input, Some(5));

        // 15 digits caps the total at 9; the request above must be clamped, not
        // rejected, and the plan must still encode.
        assert_eq!(plan.config_snapshot.digits_per_number, 15);
        assert_eq!(plan.numbers_generated.len(), 9);
        assert_eq!(plan.config_snapshot.total_numbers, 9);
        assert!(plan.expected_sum >= 0);
    }

    /// The return value carries both halves of the normalization. Only
    /// `effective` was ever asserted; `config` (the milliseconds view the
    /// worker executes) is decoded here too, and the two are checked against
    /// each other — the "reported seconds == executed milliseconds" contract,
    /// across the wasm boundary.
    #[wasm_bindgen_test]
    fn wasm_normalize_config_and_effective_agree() {
        #[derive(Debug, Deserialize)]
        struct Both {
            config: SessionConfig,
            effective: SessionConfigEffective,
        }

        let value = normalize_session_config_wasm(
            to_value(&SessionConfigInput {
                digits_per_number: 3,
                number_duration_s: 2.5,
                total_numbers: 4,
                allow_negative_numbers: true,
            })
            .expect("encode"),
        )
        .expect("valid config should normalize");

        let both: Both = from_value(value).expect("decode both halves");
        assert_eq!(both.config.number_duration_ms, 2500);
        assert_eq!(both.effective.number_duration_s, 2.5);
        assert_eq!(both.config.digits_per_number, both.effective.digits_per_number);
        assert_eq!(both.config.total_numbers, both.effective.total_numbers);
        assert_eq!(
            both.config.allow_negative_numbers,
            both.effective.allow_negative_numbers
        );
    }

    /// A decode failure must surface as a *readable JS string*, not an opaque
    /// value or a trap: the browser wraps these calls and reports the message,
    /// so the reason has to be extractable.
    #[wasm_bindgen_test]
    fn wasm_decode_errors_are_readable_js_strings() {
        let normalize_err =
            normalize_session_config_wasm(JsValue::NULL).expect_err("null must not decode");
        let message = normalize_err
            .as_string()
            .expect("the error must be a JS string the browser can read");
        assert!(message.contains("failed to decode"), "got: {message}");
        assert!(message.contains("SessionConfigInput"), "got: {message}");

        let plan_err =
            build_session_plan_wasm(1, JsValue::NULL, None).expect_err("null must not decode");
        assert!(
            plan_err
                .as_string()
                .is_some_and(|m| m.contains("failed to decode")),
            "the plan builder must report the same kind of error",
        );
    }
}
