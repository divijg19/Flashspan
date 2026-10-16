#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(not(target_arch = "wasm32"))]
mod audio;
// `core` is the module the WASM bridge exposes, so it lives in the library and
// is shared. Declaring it a second time here compiled two copies into the
// binary and ran the core test suite twice — the bin target registered all 70
// core tests in addition to the lib target's own 70.
//
// Gated to match `native_app`, the only consumer: the desktop command layer.
#[cfg(not(target_arch = "wasm32"))]
use app_lib::core;
#[cfg(not(target_arch = "wasm32"))]
mod session;

#[cfg(not(target_arch = "wasm32"))]
// Tauri resolves command inputs (`AppHandle`, `State`) by value by
// framework contract; borrowing them is not an option.
#[allow(clippy::needless_pass_by_value)]
mod native_app {
    use crate::core::{
        types::{
            AutoRepeatPlan, ClearScreen, SessionComplete, SessionConfig, SessionConfigEffective,
            SessionConfigInput, ShowNumber,
        },
        validate::normalize_session_config,
    };
    use crate::session::{SessionEmitter, SessionManager, now_epoch_ms, recover_lock};
    use log::warn;
    use serde::Deserialize;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Instant;
    use tauri::Emitter;

    /// Argument shape for `submit_answer`. Declared at module scope so the
    /// serde aliases — the IPC contract with the frontend — are reachable from
    /// tests. Local to the command function they cannot be named.
    #[derive(Deserialize, serde::Serialize)]
    struct SubmitAnswerArgs {
        #[serde(alias = "sessionId")]
        session_id: u64,
        #[serde(alias = "providedSum")]
        provided_sum: i64,
    }

    /// Argument shape for `submit_answer_text`; same reasoning as above.
    #[derive(Deserialize, serde::Serialize)]
    struct SubmitAnswerTextArgs {
        #[serde(alias = "sessionId")]
        session_id: u64,
        #[serde(alias = "providedText")]
        provided_text: String,
    }

    struct TauriEmitter {
        app: tauri::AppHandle,
    }

    impl SessionEmitter for TauriEmitter {
        fn clear_screen(&self, payload: ClearScreen) {
            let _ = self.app.emit("clear_screen", payload);
        }

        fn countdown_tick(&self, value: String) {
            let _ = self.app.emit("countdown_tick", value);
        }

        fn show_number(&self, payload: ShowNumber) {
            let _ = self.app.emit("show_number", payload);
        }

        fn session_complete(&self, payload: SessionComplete) {
            let _ = self.app.emit("session_complete", payload);
        }
    }

    #[tauri::command]
    fn ping() -> String {
        "pong".to_string()
    }

    #[tauri::command]
    fn stop_session(manager: tauri::State<'_, Arc<SessionManager>>) {
        manager.stop();
        // Cut any lingering flash beeps so audio never outlives the session.
        crate::audio::silence();
    }

    #[tauri::command]
    fn cancel_auto_repeat(manager: tauri::State<'_, Arc<SessionManager>>) {
        manager.configure_auto_repeat(None);
    }

    #[derive(Debug, Clone, Deserialize)]
    struct AutoRepeatConfigInput {
        enabled: bool,
        repeats: i64,
        delay_s: f64,
    }

    #[derive(Debug, Clone, serde::Serialize)]
    struct AutoRepeatEffective {
        enabled: bool,
        repeats: u32,
        delay_s: f64,
    }

    /// Auto-repeat policy bounds (repeats, seconds). Defensive: the UI offers
    /// a narrower range, but IPC inputs are untrusted.
    const MIN_REPEATS: i64 = 1;
    const MAX_REPEATS: i64 = 20;
    const MIN_REPEAT_DELAY_S: f64 = 5.0;
    const MAX_REPEAT_DELAY_S: f64 = 120.0;

    /// Milliseconds per second (`From` has no f64 conversion).
    const MS_PER_S: f64 = 1000.0;

    /// How often the auto-repeat countdown re-emits while waiting.
    const AUTO_REPEAT_POLL_MS: u64 = 120;

    /// The clamped auto-repeat policy for one run: the plan handed to the
    /// manager, and the values reported back to the UI.
    struct AutoRepeatPolicy {
        plan: AutoRepeatPlan,
        effective: AutoRepeatEffective,
    }

    /// Clamp untrusted IPC input into the auto-repeat policy.
    ///
    /// Split out of `start_session` so the bounds are unit-testable: they are
    /// defensive (the UI offers a narrower range) and must stay identical to the
    /// browser's `normalizeAutoRepeat`, which enforces the same numbers.
    #[allow(clippy::cast_precision_loss)]
    fn normalize_auto_repeat(
        config: &SessionConfig,
        input: &AutoRepeatConfigInput,
    ) -> AutoRepeatPolicy {
        let repeats = u32::try_from(input.repeats.clamp(MIN_REPEATS, MAX_REPEATS))
            .expect("repeats are clamped to MIN_REPEATS..=MAX_REPEATS, which fit u32");
        let delay_secs = if input.delay_s.is_finite() {
            input.delay_s.clamp(MIN_REPEAT_DELAY_S, MAX_REPEAT_DELAY_S)
        } else {
            MIN_REPEAT_DELAY_S
        };
        // `delay_secs` is finite and clamped to 5..=120, so the millisecond value
        // is at least 5000, exactly representable, and far inside u64 range.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let delay_ms = (delay_secs * MS_PER_S).round() as u64;

        // `delay_ms <= 120_000` is exactly representable in f64. Reported back
        // from the rounded millisecond value, not the requested seconds, so the
        // UI shows the delay that will actually elapse.
        let effective_delay_s = delay_ms as f64 / MS_PER_S;

        AutoRepeatPolicy {
            plan: AutoRepeatPlan {
                remaining: repeats,
                delay_ms,
                config: config.clone(),
                awaiting_validation_session_id: None,
            },
            effective: AutoRepeatEffective {
                enabled: true,
                repeats,
                delay_s: effective_delay_s,
            },
        }
    }

    #[derive(Debug, Clone, serde::Serialize)]
    struct StartSessionResponse {
        session_id: u64,
        effective_config: SessionConfigEffective,
        effective_auto_repeat: Option<AutoRepeatEffective>,
    }

    #[derive(Debug, Clone, serde::Serialize)]
    struct AutoRepeatWaitingPayload {
        session_id: u64,
        next_start_at_ms: u64,
        remaining: u32,
    }

    #[derive(Debug, Clone, serde::Serialize)]
    struct AutoRepeatTickPayload {
        session_id: u64,
        seconds_left: u64,
        remaining: u32,
    }

    #[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "snake_case")]
    enum ColorScheme {
        Midnight,
        Ivory,
        Crimson,
        Aqua,
        Violet,
        Amber,
    }

    #[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "snake_case")]
    enum ThemeMode {
        Dark,
        Light,
    }

    #[derive(Debug, Clone, serde::Serialize)]
    struct AppSettings {
        color_scheme: ColorScheme,
        theme_mode: ThemeMode,
    }

    #[derive(Default)]
    struct SettingsState(Mutex<AppSettings>);

    impl Default for AppSettings {
        fn default() -> Self {
            Self {
                color_scheme: ColorScheme::Midnight,
                theme_mode: ThemeMode::Dark,
            }
        }
    }

    #[tauri::command]
    fn get_app_settings(settings: tauri::State<'_, SettingsState>) -> AppSettings {
        recover_lock(&settings.0, "settings").clone()
    }

    #[tauri::command]
    fn get_sound_enabled() -> bool {
        crate::audio::is_enabled()
    }

    #[tauri::command]
    fn set_sound_enabled(enabled: bool) {
        crate::audio::set_enabled(enabled);
    }

    #[tauri::command]
    fn get_audio_status() -> crate::audio::AudioStatus {
        crate::audio::status()
    }

    #[tauri::command]
    fn set_color_scheme(
        app: tauri::AppHandle,
        settings: tauri::State<'_, SettingsState>,
        color_scheme: ColorScheme,
    ) -> AppSettings {
        let updated = {
            let mut guard = recover_lock(&settings.0, "settings");
            guard.color_scheme = color_scheme;
            guard.clone()
        };
        let _ = app.emit("app_settings_changed", &updated);
        updated
    }

    #[tauri::command]
    fn set_theme_mode(
        app: tauri::AppHandle,
        settings: tauri::State<'_, SettingsState>,
        theme_mode: ThemeMode,
    ) -> AppSettings {
        let updated = {
            let mut guard = recover_lock(&settings.0, "settings");
            guard.theme_mode = theme_mode;
            guard.clone()
        };

        let _ = app.emit("app_settings_changed", &updated);
        updated
    }

    #[derive(Debug, Clone, serde::Serialize)]
    struct ValidationResult {
        expected_sum: i64,
        provided_sum: i64,
        correct: bool,
        delta: i64,
    }

    #[derive(Debug, Clone, serde::Serialize)]
    struct SubmitAnswerResponse {
        validation: ValidationResult,
        auto_repeat_waiting: Option<AutoRepeatWaitingPayload>,
        message: String,
    }

    /// Longest accepted answer text, measured after trimming and comma removal, so
    /// it bounds the digit string only. Purely defensive: the widest `i64` needs 20
    /// characters, leaving generous room for any real answer.
    const MAX_ANSWER_CHARS: usize = 64;

    const ANSWER_ERROR: &str = "Enter a single integer answer (e.g. 42 or -17).";

    /// Answer delta with verdict. Saturating (not plain) subtraction: the
    /// digit-width bound keeps attainable sums far inside i64 range, but a
    /// user can still *type* `i64::MIN`/`MAX`, and plain subtraction would
    /// then panic in debug (or wrap in release) against a large expected
    /// sum. Saturation can never flip the verdict: a clamped delta is
    /// nonzero exactly when the true delta is nonzero, and deltas that fit
    /// stay exact.
    const fn validation_delta(provided_sum: i64, expected_sum: i64) -> (i64, bool) {
        let delta = provided_sum.saturating_sub(expected_sum);
        (delta, delta == 0)
    }

    /// The user-facing grading message for a submitted answer.
    ///
    /// Split out of `submit_answer_inner` because that command needs a live
    /// `AppHandle`, which no test can build - so the one string every user reads
    /// after answering was unreachable from the test suite. It is pure, and it
    /// is where the delta sign convention lives: a positive difference (you
    /// over-answered) is shown with an explicit `+`, a negative one already has
    /// its minus.
    fn grading_message(correct: bool, expected_sum: i64, delta: i64) -> String {
        let mut lines: Vec<String> = Vec::new();
        lines.push(if correct {
            "Correct ✅".to_string()
        } else {
            "Incorrect".to_string()
        });
        lines.push(format!("Expected answer: {expected_sum}"));
        if !correct {
            lines.push(format!(
                "Difference: {}{}",
                if delta > 0 { "+" } else { "" },
                delta
            ));
        }
        lines.join("\n")
    }

    fn parse_answer_text(input: &str) -> Result<i64, String> {
        let cleaned = input.trim().replace(',', "");
        // Defensive bound: avoid absurd payload sizes. Anything that is not a
        // single integer within it gets the same message, so the failure
        // cannot be used to probe the parser.
        if cleaned.is_empty() || cleaned.len() > MAX_ANSWER_CHARS {
            return Err(ANSWER_ERROR.to_string());
        }

        cleaned.parse::<i64>().map_err(|_| ANSWER_ERROR.to_string())
    }

    #[cfg(test)]
    mod main_tests {
        use super::*;
        use proptest::prelude::*;

        #[test]
        fn parse_answer_text_accepts_commas_and_spaces() {
            assert_eq!(parse_answer_text("  42 ").unwrap(), 42);
            assert_eq!(parse_answer_text("1,234").unwrap(), 1234);
            assert!(parse_answer_text("").is_err());
            assert!(parse_answer_text("not a number").is_err());
        }

        #[test]
        fn parse_answer_text_bounds_check() {
            // Very long numeric string is rejected by our defensive bound
            let long = "1".repeat(100);
            assert!(parse_answer_text(&long).is_err());
        }

        #[test]
        fn validation_delta_extremes_and_saturation() {
            // Ordinary cases stay exact.
            assert_eq!(validation_delta(6, 6), (0, true));
            assert_eq!(validation_delta(9, 10), (-1, false));
            assert_eq!(validation_delta(12, 10), (2, false));

            // Absurd typed extremes saturate without flipping the verdict.
            assert_eq!(
                validation_delta(i64::MIN, 9_000_000_000_000_000),
                (i64::MIN, false)
            );
            assert_eq!(
                validation_delta(i64::MAX, -9_000_000_000_000_000),
                (i64::MAX, false)
            );
            // Exact boundary: MIN minus zero fits and stays exact.
            assert_eq!(validation_delta(i64::MIN, 0), (i64::MIN, false));
        }

        #[test]
        fn parse_answer_text_mirror_vectors() {
            // Must accept/reject exactly like the browser parser
            // (parseProvidedAnswerText): strict integers only.
            assert_eq!(parse_answer_text("+42").unwrap(), 42);
            assert_eq!(parse_answer_text("1,,2").unwrap(), 12);

            for bad in [
                "42.9",
                "-3.14",
                "42.0",
                "1e3",
                "0x10",
                "1_2",
                "1 2",
                "4\t2",
                "--42",
                "+-42",
                "not a number",
                "12abc34",
            ] {
                assert!(
                    parse_answer_text(bad).is_err(),
                    "{bad:?} should be rejected"
                );
            }

            assert!(parse_answer_text(&"1".repeat(65)).is_err());
            assert!(parse_answer_text("9223372036854775808").is_err());
            assert!(parse_answer_text("-9223372036854775809").is_err());
        }

        #[test]
        /// The serde aliases on `SubmitAnswerArgs` are the IPC contract with
        /// the frontend: `runtime/native.ts` sends `{"sessionId", "providedSum"}`
        /// and the Rust struct is spelled `session_id`/`provided_sum`. Both
        /// spellings must decode, and a missing field must be an `Err` rather than
        /// a panic across the IPC boundary.
        fn submit_answer_args_accept_both_field_spellings() {
            let canonical: SubmitAnswerArgs =
                serde_json::from_value(serde_json::json!({"session_id": 7, "provided_sum": 42}))
                    .expect("canonical field names");
            assert_eq!(canonical.session_id, 7);
            assert_eq!(canonical.provided_sum, 42);

            let aliased: SubmitAnswerArgs =
                serde_json::from_value(serde_json::json!({"sessionId": 8, "providedSum": 43}))
                    .expect("aliased field names");
            assert_eq!(aliased.session_id, 8);
            assert_eq!(aliased.provided_sum, 43);

            // Supplying both spellings is a duplicate field, which serde rejects
            // rather than silently picking one — an ambiguous payload must not
            // decode.
            let duplicate = serde_json::from_value::<SubmitAnswerArgs>(serde_json::json!({
                "session_id": 1,
                "sessionId": 99,
                "provided_sum": 1,
            }));
            assert!(duplicate.is_err(), "duplicate field names must be rejected");

            // A missing or mistyped field must fail the decode, never panic.
            assert!(serde_json::from_value::<SubmitAnswerArgs>(serde_json::json!({})).is_err());
            assert!(
                serde_json::from_value::<SubmitAnswerArgs>(serde_json::json!({
                    "session_id": "seven",
                    "provided_sum": 42,
                }))
                .is_err()
            );
        }

        /// Same contract for the text-answer variant.
        #[test]
        fn submit_answer_text_args_accept_both_field_spellings() {
            let canonical: SubmitAnswerTextArgs = serde_json::from_value(serde_json::json!({
                "session_id": 1,
                "provided_text": "42",
            }))
            .expect("canonical field names");
            assert_eq!(canonical.session_id, 1);
            assert_eq!(canonical.provided_text, "42");

            let aliased: SubmitAnswerTextArgs = serde_json::from_value(serde_json::json!({
                "sessionId": 2,
                "providedText": "42",
            }))
            .expect("aliased field names");
            assert_eq!(aliased.session_id, 2);
            assert_eq!(aliased.provided_text, "42");

            assert!(serde_json::from_value::<SubmitAnswerTextArgs>(serde_json::json!({})).is_err());
            assert!(
                serde_json::from_value::<SubmitAnswerTextArgs>(serde_json::json!({
                    "sessionId": 2,
                    "providedText": 1234,
                }))
                .is_err()
            );
        }

        /// An empty typed answer and an unparseable one must surface as the
        /// shared `ANSWER_ERROR`, not as a different or empty string.
        #[test]
        fn submit_answer_text_args_reject_bad_text_without_panicking() {
            for bad in ["", "   ", "not a number", "42.5"] {
                let args = SubmitAnswerTextArgs {
                    session_id: 1,
                    provided_text: bad.to_string(),
                };
                let decoded = serde_json::to_value(&args).expect("serialize args");
                let parsed: SubmitAnswerTextArgs =
                    serde_json::from_value(decoded).expect("round trip");
                assert_eq!(
                    parse_answer_text(&parsed.provided_text).unwrap_err(),
                    ANSWER_ERROR
                );
            }
        }

        #[test]
        fn parse_answer_text_negatives_and_extremes() {
            // Negative numbers
            assert_eq!(parse_answer_text("-42").unwrap(), -42);
            assert_eq!(parse_answer_text("  -17  ").unwrap(), -17);
            assert_eq!(parse_answer_text("-0").unwrap(), 0);

            // Boundary values
            assert_eq!(parse_answer_text("9223372036854775807").unwrap(), i64::MAX);
            assert_eq!(parse_answer_text("-9223372036854775808").unwrap(), i64::MIN);

            // Whitespace and commas
            assert_eq!(parse_answer_text("  1,234,567  ").unwrap(), 1_234_567);
            assert_eq!(parse_answer_text("-9,876").unwrap(), -9876);

            // Rejects non-numeric and empty
            assert!(parse_answer_text("abc").is_err());
            assert!(parse_answer_text("12abc34").is_err());
            assert!(parse_answer_text("   ").is_err());
        }

        /**
         * The remaining parity corpus. Both parsers were verified to agree on
         * every one of these, but the agreement is incidental — nothing pinned
         * it, so either side drifting would silently grade differently.
         */
        #[test]
        fn parse_answer_text_agrees_with_the_browser_on_the_rest() {
            // Accepted by both, with the values both produce.
            for (input, expected) in [
                ("+0", 0),
                ("-0", 0),
                ("+1,000", 1000),
                (",42", 42),
                ("42,", 42),
                ("12,3,4", 1234),
                ("1,,2", 12),
                // Tabs are whitespace on both sides, not separators.
                ("\t42\t", 42),
            ] {
                assert_eq!(
                    parse_answer_text(input).unwrap_or(i64::MIN),
                    expected,
                    "{input:?} should parse to {expected}"
                );
            }

            // Rejected by both, for the same reason: an internal separator is
            // not a valid integer on either side.
            for bad in [
                "+ +1",
                "+",
                "-",
                "   ",
                "٤٢",
                "１２３",
                "１２３４５６７８９０",
            ] {
                assert!(
                    parse_answer_text(bad).is_err(),
                    "{bad:?} should be rejected"
                );
            }
        }

        /// The grading message is the one piece of feedback every user reads
        /// after answering, and it was built inside a Tauri command that needs a
        /// live `AppHandle` — unreachable from any test. Now that it is a pure
        /// function these pin the shape, and in particular the delta sign
        /// convention: a positive difference is shown as `+3` so it reads as
        /// "you over-answered by 3" rather than the bare `3` a reader could
        /// mistake for the expected total.
        #[test]
        fn grading_message_reports_the_expected_sum() {
            let message = grading_message(true, 42, 0);
            assert_eq!(message, "Correct \u{2705}\nExpected answer: 42");
        }

        #[test]
        fn grading_message_signs_a_positive_difference() {
            let (delta, correct) = validation_delta(45, 42);
            assert!(!correct);
            assert_eq!(
                grading_message(correct, 42, delta),
                "Incorrect\nExpected answer: 42\nDifference: +3"
            );
        }

        #[test]
        fn grading_message_keeps_a_negative_sign_without_a_prefix() {
            let (delta, correct) = validation_delta(39, 42);
            assert!(!correct);
            // The minus already carries the direction; a `+-3` would be wrong.
            assert_eq!(
                grading_message(correct, 42, delta),
                "Incorrect\nExpected answer: 42\nDifference: -3"
            );
        }

        /// A saturating delta still formats, and the verdict still holds: a
        /// clamped delta is nonzero exactly when the true delta is.
        #[test]
        fn grading_message_survives_a_saturated_delta() {
            let (delta, correct) = validation_delta(i64::MAX, -9_000_000_000_000_000);
            assert!(!correct);
            assert_eq!(delta, i64::MAX);
            let message = grading_message(correct, -9_000_000_000_000_000, delta);
            assert!(message.starts_with("Incorrect\n"));
            assert!(message.contains("Difference: +9223372036854775807"));
        }

        /// The 64-char cap counts BYTES, while the browser counts UTF-16 code
        /// units. No input the browser accepts can expose the difference (its
        /// regex admits ASCII only), so pin the Rust side so any change to this
        /// is a reviewed decision rather than an accident.
        #[test]
        fn parse_answer_text_cap_counts_bytes() {
            // 21 full-width characters = 63 bytes: passes the cap, fails to parse.
            assert!(parse_answer_text(&"１".repeat(21)).is_err());
            // 22 = 66 bytes: rejected by the cap itself.
            assert!(parse_answer_text(&"１".repeat(22)).is_err());
        }

        /// `String::trim` and JS `trim()` disagree on exactly one character:
        /// `U+FEFF` is in JS's `WhiteSpace` set but is `Cf` (format) in Unicode, so
        /// Rust keeps it and the parse fails. Every other whitespace character
        /// tested agrees. A pasted answer carrying a BOM therefore grades on
        /// the web and errors on the desktop.
        #[test]
        fn parse_answer_text_bom_diverges_from_the_browser() {
            assert_eq!(parse_answer_text("42").unwrap(), 42);
            // Lock the current desktop behaviour; fixing Rust to strip it would
            // make both agree and this assertion would be updated alongside.
            assert!(parse_answer_text("\u{feff}42").is_err());
        }

        /// The auto-repeat bounds are defensive: the UI offers 1..=20 repeats of
        /// 5..=120s, but IPC input is untrusted. These pin the clamp, the
        /// millisecond rounding, and the effective value reported back — the
        /// same policy the browser's `normalizeAutoRepeat` enforces.
        #[test]
        #[allow(clippy::float_cmp)]
        fn normalize_auto_repeat_clamps_and_reports_the_applied_values() {
            let (config, _effective) = normalize_session_config(&SessionConfigInput {
                digits_per_number: 1,
                number_duration_s: 1.0,
                total_numbers: 2,
                allow_negative_numbers: false,
            });

            let clamped_low = normalize_auto_repeat(
                &config,
                &AutoRepeatConfigInput {
                    enabled: true,
                    repeats: 0,
                    delay_s: 1.0,
                },
            );
            assert_eq!(clamped_low.effective.repeats, 1);
            assert_eq!(clamped_low.plan.remaining, 1);
            assert_eq!(clamped_low.plan.delay_ms, 5_000);
            assert_eq!(clamped_low.effective.delay_s, 5.0);

            let clamped_high = normalize_auto_repeat(
                &config,
                &AutoRepeatConfigInput {
                    enabled: true,
                    repeats: 999,
                    delay_s: 1.0e9,
                },
            );
            assert_eq!(clamped_high.effective.repeats, 20);
            assert_eq!(clamped_high.plan.remaining, 20);
            assert_eq!(clamped_high.plan.delay_ms, 120_000);
            assert_eq!(clamped_high.effective.delay_s, 120.0);

            // A non-finite delay falls back to the minimum, never to NaN/Inf.
            for delay_s in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
                let policy = normalize_auto_repeat(
                    &config,
                    &AutoRepeatConfigInput {
                        enabled: true,
                        repeats: 3,
                        delay_s,
                    },
                );
                assert_eq!(policy.plan.delay_ms, 5_000);
                assert_eq!(policy.effective.delay_s, 5.0);
            }

            // The reported seconds come from the rounded milliseconds, so the UI
            // shows the delay that will actually elapse.
            let fractional = normalize_auto_repeat(
                &config,
                &AutoRepeatConfigInput {
                    enabled: true,
                    repeats: 2,
                    delay_s: 12.34,
                },
            );
            assert_eq!(fractional.plan.delay_ms, 12_340);
            assert_eq!(fractional.effective.delay_s, 12.34);

            // The plan carries the normalized config, so the restarted session
            // reuses it rather than re-clamping.
            assert_eq!(fractional.plan.config.digits_per_number, 1);
            assert_eq!(fractional.plan.awaiting_validation_session_id, None);
        }

        /// The colour schemes cross to the UI as their `snake_case` names, which
        /// `runtime/types.ts` narrows into the `ColorScheme` union and the
        /// stylesheet keys off. A renamed variant would serialise to a string no
        /// scheme matches, silently falling back to default colours on desktop
        /// while the web picker kept working — and no test would notice.
        #[test]
        fn color_scheme_round_trips_as_snake_case() {
            for (scheme, wire) in [
                (ColorScheme::Midnight, "midnight"),
                (ColorScheme::Ivory, "ivory"),
                (ColorScheme::Crimson, "crimson"),
                (ColorScheme::Aqua, "aqua"),
                (ColorScheme::Violet, "violet"),
                (ColorScheme::Amber, "amber"),
            ] {
                assert_eq!(
                    serde_json::to_value(scheme).expect("serialize scheme"),
                    serde_json::json!(wire),
                    "{scheme:?} must cross as {wire:?}",
                );
                // And back again, so a rename on either side of the derive is
                // caught rather than only the serializer.
                let decoded: ColorScheme =
                    serde_json::from_value(serde_json::json!(wire)).expect("decode scheme");
                assert_eq!(
                    serde_json::to_value(decoded).expect("re-serialize scheme"),
                    serde_json::json!(wire),
                );
            }

            // A stale saved value for a scheme that no longer exists must fail
            // to decode, not silently become the default.
            assert!(
                serde_json::from_value::<ColorScheme>(serde_json::json!("obsidian")).is_err(),
                "an unknown scheme must be rejected",
            );
        }

        #[test]
        fn theme_mode_round_trips_as_snake_case() {
            for (mode, wire) in [(ThemeMode::Dark, "dark"), (ThemeMode::Light, "light")] {
                assert_eq!(
                    serde_json::to_value(mode).expect("serialize mode"),
                    serde_json::json!(wire),
                );
                let decoded: ThemeMode =
                    serde_json::from_value(serde_json::json!(wire)).expect("decode mode");
                assert_eq!(
                    serde_json::to_value(decoded).expect("re-serialize mode"),
                    serde_json::json!(wire),
                );
            }
            assert!(serde_json::from_value::<ThemeMode>(serde_json::json!("night")).is_err());
        }

        #[test]
        fn app_settings_default_is_midnight_dark_with_the_frontend_keys() {
            let settings = AppSettings::default();
            assert!(matches!(settings.color_scheme, ColorScheme::Midnight));
            assert!(matches!(settings.theme_mode, ThemeMode::Dark));

            // The UI reads these two exact keys off `get_app_settings`.
            assert_eq!(
                serde_json::to_value(&settings).expect("serialize settings"),
                serde_json::json!({ "color_scheme": "midnight", "theme_mode": "dark" }),
            );
        }

        /// The nested auto-repeat object `runtime/native.ts` sends. Serde has no
        /// rename here, so the field names are the contract; `delay_s` in
        /// particular is shared verbatim with `types.ts`.
        #[test]
        #[allow(clippy::float_cmp)] // Exactness lock on the decoded delay.
        fn auto_repeat_config_input_decodes_the_frontend_payload() {
            let decoded: AutoRepeatConfigInput = serde_json::from_value(serde_json::json!({
                "enabled": true,
                "repeats": 3,
                "delay_s": 10.0,
            }))
            .expect("the shape runtime/native.ts sends");
            assert!(decoded.enabled);
            assert_eq!(decoded.repeats, 3);
            assert_eq!(decoded.delay_s, 10.0);

            // A missing field is an error, not a silently-defaulted zero.
            assert!(
                serde_json::from_value::<AutoRepeatConfigInput>(serde_json::json!({
                    "enabled": true,
                    "repeats": 3,
                }))
                .is_err(),
                "a partial payload must not decode",
            );

            // A non-integer repeat count is rejected rather than truncated.
            assert!(
                serde_json::from_value::<AutoRepeatConfigInput>(serde_json::json!({
                    "enabled": true,
                    "repeats": 3.5,
                    "delay_s": 10.0,
                }))
                .is_err(),
                "repeats must be an integer",
            );

            // Unknown fields are ignored, so a newer frontend can add one
            // without breaking an older backend.
            let extra: AutoRepeatConfigInput = serde_json::from_value(serde_json::json!({
                "enabled": false,
                "repeats": 1,
                "delay_s": 5.0,
                "future_field": "ignored",
            }))
            .expect("unknown fields are ignored");
            assert!(!extra.enabled);
        }

        #[test]
        fn auto_repeat_effective_serializes_the_reported_numbers() {
            let effective = AutoRepeatEffective {
                enabled: true,
                repeats: 5,
                delay_s: 12.5,
            };
            assert_eq!(
                serde_json::to_value(&effective).expect("serialize effective"),
                serde_json::json!({ "enabled": true, "repeats": 5, "delay_s": 12.5 }),
            );
        }

        /// `App.tsx` reads `session_id`, `effective_config` and
        /// `effective_auto_repeat` off the start response. `None` must be an
        /// explicit `null` (the TS type is `| null`), not a missing key.
        #[test]
        fn start_session_response_serializes_with_the_frontend_keys() {
            let response = StartSessionResponse {
                session_id: 7,
                effective_config: SessionConfigEffective {
                    digits_per_number: 2,
                    number_duration_s: 0.5,
                    total_numbers: 3,
                    allow_negative_numbers: true,
                },
                effective_auto_repeat: Some(AutoRepeatEffective {
                    enabled: true,
                    repeats: 2,
                    delay_s: 10.0,
                }),
            };

            let json = serde_json::to_value(&response).expect("serialize response");
            assert_eq!(json["session_id"], serde_json::json!(7));
            assert_eq!(json["effective_config"]["digits_per_number"], serde_json::json!(2));
            assert_eq!(
                json["effective_config"],
                serde_json::json!({
                    "digits_per_number": 2,
                    "number_duration_s": 0.5,
                    "total_numbers": 3,
                    "allow_negative_numbers": true,
                }),
            );
            assert_eq!(json["effective_auto_repeat"]["repeats"], serde_json::json!(2));

            let without = StartSessionResponse {
                effective_auto_repeat: None,
                ..response
            };
            let json = serde_json::to_value(&without).expect("serialize response");
            assert_eq!(
                json["effective_auto_repeat"],
                serde_json::Value::Null,
                "an absent repeat policy must be an explicit null",
            );
        }

        #[test]
        fn auto_repeat_payloads_serialize_with_the_frontend_keys() {
            let waiting = AutoRepeatWaitingPayload {
                session_id: 1,
                next_start_at_ms: 1_700_000_000_000,
                remaining: 3,
            };
            assert_eq!(
                serde_json::to_value(&waiting).expect("serialize waiting"),
                serde_json::json!({
                    "session_id": 1,
                    "next_start_at_ms": 1_700_000_000_000u64,
                    "remaining": 3,
                }),
            );

            let tick = AutoRepeatTickPayload {
                session_id: 1,
                seconds_left: 2,
                remaining: 3,
            };
            assert_eq!(
                serde_json::to_value(&tick).expect("serialize tick"),
                serde_json::json!({ "session_id": 1, "seconds_left": 2, "remaining": 3 }),
            );
        }

        /// The grading payload is what the UI renders after a submission: the
        /// per-field validation (used for the correct/incorrect styling) and the
        /// runtime-owned `message` (rendered verbatim). Pin every key.
        #[test]
        fn submit_answer_response_serializes_the_grading_payload() {
            let response = SubmitAnswerResponse {
                validation: ValidationResult {
                    expected_sum: 42,
                    provided_sum: 39,
                    correct: false,
                    delta: -3,
                },
                auto_repeat_waiting: None,
                message: "Incorrect\nExpected answer: 42\nDifference: -3".to_string(),
            };

            let json = serde_json::to_value(&response).expect("serialize response");
            assert_eq!(
                json["validation"],
                serde_json::json!({
                    "expected_sum": 42,
                    "provided_sum": 39,
                    "correct": false,
                    "delta": -3,
                }),
            );
            assert_eq!(json["auto_repeat_waiting"], serde_json::Value::Null);
            assert_eq!(
                json["message"],
                serde_json::json!("Incorrect\nExpected answer: 42\nDifference: -3"),
            );
        }

        /// The verdict is exactly "the typed answer equals the expected sum", and
        /// the delta is zero exactly when that holds — including at both integer
        /// extremes, where the subtraction saturates.
        #[test]
        fn validation_delta_verdict_is_provided_equals_expected() {
            for (provided, expected) in [
                (0i64, 0i64),
                (5, 5),
                (-7, -7),
                (i64::MAX, i64::MAX),
                (i64::MIN, i64::MIN),
                (9, 10),
                (10, 9),
                (i64::MIN, i64::MAX),
                (i64::MAX, i64::MIN),
            ] {
                let (delta, correct) = validation_delta(provided, expected);
                assert_eq!(
                    correct,
                    provided == expected,
                    "verdict for {provided} vs {expected}",
                );
                assert_eq!(
                    correct,
                    delta == 0,
                    "a zero delta and a correct verdict must coincide ({provided} vs {expected})",
                );
            }
        }

        /// The 64-character cap is applied *after* trimming and comma removal, so
        /// a comma-formatted answer well over 64 raw characters still parses.
        /// This is what lets a user paste `1,111,111,111` without a length error.
        #[test]
        fn parse_answer_text_length_cap_counts_the_cleaned_digits() {
            let padded = format!("{}1", "1,".repeat(10));
            assert!(padded.len() > 20, "the raw text carries separators");
            assert_eq!(parse_answer_text(&padded).expect("parses after cleaning"), 11_111_111_111);

            // A string that cleans to nothing is rejected by the empty check.
            assert!(parse_answer_text(&",".repeat(100)).is_err());

            // An over-long *digit* string is still rejected by the cap.
            assert!(parse_answer_text(&"1".repeat(65)).is_err());
        }

        /// Untrusted IPC text: no input may panic the parser, and every failure
        /// must carry the one shared message (a distinct message would let a
        /// caller probe the parser). Sweeps arbitrary unicode, whitespace,
        /// separators and length.
        #[test]
        fn prop_parse_answer_text_never_panics_and_never_leaks() {
            proptest!(|(text in any::<String>())| {
                let cleaned = text.trim().replace(',', "");
                match parse_answer_text(&text) {
                    Ok(value) => {
                        // Acceptance implies a plain integer within the cap.
                        prop_assert!(cleaned.len() <= 64);
                        prop_assert_eq!(cleaned.parse::<i64>().ok(), Some(value));
                    }
                    Err(message) => prop_assert_eq!(message, ANSWER_ERROR),
                }
            });
        }

        /// The auto-repeat clamp is the last line of defence for untrusted IPC
        /// numbers. For *any* `i64` repeats and *any* `f64` delay — `NaN` and
        /// both infinities included — it must yield a policy inside the
        /// documented bounds, with the reported seconds exactly the executed
        /// milliseconds and never a non-finite value.
        #[test]
        #[allow(clippy::float_cmp)] // Exactness lock: the reported seconds are derived from the ms.
        fn prop_normalize_auto_repeat_stays_inside_the_policy_bounds() {
            let (config, _effective) = normalize_session_config(&SessionConfigInput {
                digits_per_number: 1,
                number_duration_s: 0.5,
                total_numbers: 1,
                allow_negative_numbers: false,
            });

            proptest!(|(repeats in any::<i64>(), delay_s in any::<f64>())| {
                let policy = normalize_auto_repeat(
                    &config,
                    &AutoRepeatConfigInput {
                        enabled: true,
                        repeats,
                        delay_s,
                    },
                );

                prop_assert!((1..=20).contains(&policy.effective.repeats));
                prop_assert_eq!(policy.plan.remaining, policy.effective.repeats);
                prop_assert!((5_000..=120_000).contains(&policy.plan.delay_ms));
                prop_assert!((5.0..=120.0).contains(&policy.effective.delay_s));

                // The reported seconds are the rounded milliseconds, exactly,
                // so the UI shows the delay that will actually elapse.
                prop_assert_eq!(
                    policy.effective.delay_s,
                    f64::from(u32::try_from(policy.plan.delay_ms).expect("inside the bound"))
                        / 1000.0,
                );

                // A non-finite delay pins to the minimum rather than passing
                // NaN/Inf through to the countdown.
                if !delay_s.is_finite() {
                    prop_assert_eq!(policy.plan.delay_ms, 5_000);
                }
            });
        }

        /// Fractional-second delays round to whole milliseconds, and the value
        /// reported back to the UI is derived from those milliseconds — not the
        /// requested seconds — so the countdown the user sees is the one that
        /// will elapse. Only the clamp bounds were tested before.
        #[test]
        #[allow(clippy::float_cmp)] // Exactness lock: the reported seconds derive from the ms.
        fn normalize_auto_repeat_rounds_the_delay_to_whole_milliseconds() {
            let (config, _effective) = normalize_session_config(&SessionConfigInput {
                digits_per_number: 1,
                number_duration_s: 0.5,
                total_numbers: 1,
                allow_negative_numbers: false,
            });

            for (delay_s, expected_ms) in [
                (5.0, 5_000u64),
                (5.0004, 5_000),   // rounds down
                (5.0006, 5_001),   // rounds up
                (7.777, 7_777),
                (12.3454, 12_345),
                (12.3456, 12_346),
                (59.999, 59_999),
                (60.001, 60_001),
                (119.999, 119_999),
                (120.0, 120_000),
            ] {
                let policy = normalize_auto_repeat(
                    &config,
                    &AutoRepeatConfigInput {
                        enabled: true,
                        repeats: 3,
                        delay_s,
                    },
                );
                assert_eq!(policy.plan.delay_ms, expected_ms, "delay {delay_s}s");
                assert_eq!(
                    policy.effective.delay_s,
                    f64::from(u32::try_from(expected_ms).expect("inside the bound")) / 1000.0,
                    "the reported seconds must be the executed milliseconds",
                );
            }
        }
    }

    fn schedule_auto_repeat_if_needed(
        app: &tauri::AppHandle,
        manager: Arc<SessionManager>,
        session_id: u64,
    ) -> Option<AutoRepeatWaitingPayload> {
        warn!(target: "auto-repeat", "schedule_auto_repeat_if_needed: session_id={session_id}");
        let result = manager.mark_validated_and_schedule_info(session_id);
        warn!(target: "auto-repeat", "mark_validated_and_schedule_info returned: {result:?}");
        let (delay_ms, remaining, config, generation) = result?;

        let next_start_at_ms = now_epoch_ms().saturating_add(delay_ms);
        let payload = AutoRepeatWaitingPayload {
            session_id,
            next_start_at_ms,
            remaining,
        };

        let _ = app.emit("auto_repeat_waiting", &payload);

        // The countdown runs on its own thread and this returns at once. Waiting
        // here would hold open the IPC call that asked for it for the whole
        // delay — up to two minutes — so the frontend would never learn the
        // answer was accepted (`hasValidated` stays false, so the type-mode
        // "Show numbers" button never works) and, as a synchronous command, the
        // app itself would stall until the restart.
        //
        // Ticks are emitted from *inside* the wait, one per distinct second,
        // rather than collected and replayed afterwards, so the bar counts down
        // while it waits instead of jumping to zero at the deadline. A generation
        // bump at any check — including the last one, after the delay — abandons
        // the session rather than starting one the user cancelled or superseded.
        let manager_arc = Arc::clone(&manager);
        let app_for_thread = app.clone();
        let remaining_repeats = remaining;
        let generation_at_start = generation;
        let spawned = std::thread::Builder::new()
            .name("auto-repeat".into())
            .spawn(move || {
                let end_at = Instant::now() + std::time::Duration::from_millis(delay_ms);
                let poll = || {
                    // One poll interval, bounded by what is left so the final
                    // iteration lands on the deadline rather than overshooting.
                    thread::sleep(
                        std::time::Duration::from_millis(AUTO_REPEAT_POLL_MS)
                            .min(end_at.saturating_duration_since(Instant::now())),
                    );
                };

                let may_start = crate::session::drive_auto_repeat_wait(
                    generation_at_start,
                    &mut || {
                        crate::session::millis_u64(end_at.saturating_duration_since(Instant::now()))
                    },
                    &mut || manager_arc.auto_repeat_generation(),
                    |seconds_left| {
                        let _ = app_for_thread.emit(
                            "auto_repeat_tick",
                            AutoRepeatTickPayload {
                                session_id,
                                seconds_left,
                                remaining: remaining_repeats,
                            },
                        );
                    },
                    poll,
                );
                if !may_start {
                    warn!(target: "auto-repeat", "abandoned: generation bumped while waiting");
                    return;
                }

                if let Err(e) = manager_arc.start_with_emitter(
                    TauriEmitter {
                        app: app_for_thread,
                    },
                    config,
                ) {
                    warn!(target: "auto-repeat", "start failed: {e}");
                }
            });

        if let Err(e) = spawned {
            warn!(target: "auto-repeat", "failed to spawn auto-repeat thread: {e}");
        }

        Some(payload)
    }

    // Millisecond values below are exactly representable in f64
    // (`delay_ms <= 120_000`, durations <= 60_000); `From` has no u64 impl.
    #[allow(clippy::cast_precision_loss)]
    #[tauri::command]
    fn start_session(
        app: tauri::AppHandle,
        manager: tauri::State<'_, Arc<SessionManager>>,
        config: SessionConfigInput,
        auto_repeat: Option<AutoRepeatConfigInput>,
    ) -> Result<StartSessionResponse, String> {
        warn!(target: "auto-repeat", "start_session: auto_repeat={auto_repeat:?}");
        let (config, effective_config) = normalize_session_config(&config);

        // Configure the auto-repeat plan for this run, or clear it.
        //
        // Written as a `match` with the two clearing arms adjacent so the rule
        // "absent or disabled means no repeat, and must also clear whatever the
        // previous session left armed" reads in one place. The `map_or_else`
        // this replaced spread that rule across the closure's default arm and a
        // nested `else`, so the two `configure_auto_repeat(None)` calls were
        // easy to change independently.
        let effective_auto_repeat = match auto_repeat {
            None => {
                manager.configure_auto_repeat(None);
                None
            }
            Some(ar) if !ar.enabled => {
                manager.configure_auto_repeat(None);
                None
            }
            Some(ar) => {
                let policy = normalize_auto_repeat(&config, &ar);
                manager.configure_auto_repeat(Some(policy.plan));
                Some(policy.effective)
            }
        };

        // Pre-warm audio while the 3s countdown runs: cold device open
        // would otherwise delay the first beep of the first session.
        crate::audio::warmup();

        let session_id = manager.start_with_emitter(TauriEmitter { app }, config)?;
        Ok(StartSessionResponse {
            session_id,
            effective_config,
            effective_auto_repeat,
        })
    }

    // The outer `Result` is the Tauri command contract (IPC errors); the
    // inner `Option` is the only meaningful absence signal.
    #[allow(clippy::unnecessary_wraps)]
    #[tauri::command]
    fn acknowledge_complete(
        app: tauri::AppHandle,
        manager: tauri::State<'_, Arc<SessionManager>>,
        session_id: u64,
    ) -> Result<Option<AutoRepeatWaitingPayload>, String> {
        Ok(schedule_auto_repeat_if_needed(
            &app,
            Arc::clone(&*manager),
            session_id,
        ))
    }

    /// Shared body of `submit_answer` / `submit_answer_text`: the two commands
    /// differ only in how the answer arrives (typed sum vs. text), so grading
    /// and auto-repeat scheduling live here exactly once.
    fn submit_answer_inner(
        app: &tauri::AppHandle,
        manager: &Arc<SessionManager>,
        session_id: u64,
        provided_sum: i64,
    ) -> Result<SubmitAnswerResponse, String> {
        let result = manager.result_for(session_id)?;
        let expected_sum = result.sum;

        let (delta, correct) = validation_delta(provided_sum, expected_sum);

        let validation = ValidationResult {
            expected_sum,
            provided_sum,
            correct,
            delta,
        };

        // Play feedback sound based on validation result (Rust owns playback).
        let _ = crate::audio::play_kind(if correct { "applause" } else { "buzzer" });

        let waiting = schedule_auto_repeat_if_needed(app, Arc::clone(manager), session_id);
        let message = grading_message(correct, expected_sum, validation.delta);

        Ok(SubmitAnswerResponse {
            validation,
            auto_repeat_waiting: waiting,
            message,
        })
    }

    #[tauri::command]
    fn submit_answer_text(
        app: tauri::AppHandle,
        manager: tauri::State<'_, Arc<SessionManager>>,
        args: serde_json::Value,
    ) -> Result<SubmitAnswerResponse, String> {
        let parsed: SubmitAnswerTextArgs =
            serde_json::from_value(args).map_err(|e| format!("invalid args: {e}"))?;
        let provided_sum = parse_answer_text(&parsed.provided_text)?;
        submit_answer_inner(&app, &manager, parsed.session_id, provided_sum)
    }

    #[tauri::command]
    fn submit_answer(
        app: tauri::AppHandle,
        manager: tauri::State<'_, Arc<SessionManager>>,
        args: serde_json::Value,
    ) -> Result<SubmitAnswerResponse, String> {
        let parsed: SubmitAnswerArgs =
            serde_json::from_value(args).map_err(|e| format!("invalid args: {e}"))?;
        submit_answer_inner(&app, &manager, parsed.session_id, parsed.provided_sum)
    }

    /// Launch the Tauri application event loop.
    ///
    /// # Panics
    ///
    /// Panics if the Tauri runtime fails to start (missing `WebView` backend or
    /// context initialization failure).
    ///
    /// The `mobile_entry_point` annotation used to sit on an uncalled `run()` in
    /// `lib.rs`, so a mobile build would have resolved to a dead function while
    /// this one — the entry point `main()` actually calls — carried no
    /// annotation. It is inert whenever the `mobile` cfg is unset, which is
    /// every target CI builds.
    #[cfg_attr(mobile, tauri::mobile_entry_point)]
    pub fn run() {
        tauri::Builder::default()
            .setup(|app| {
                if cfg!(debug_assertions) {
                    app.handle().plugin(
                        tauri_plugin_log::Builder::default()
                            .level(log::LevelFilter::Info)
                            .build(),
                    )?;
                }
                Ok(())
            })
            .manage(Arc::new(SessionManager::default()))
            .manage(SettingsState::default())
            .invoke_handler(tauri::generate_handler![
                ping,
                get_app_settings,
                set_color_scheme,
                set_theme_mode,
                start_session,
                stop_session,
                cancel_auto_repeat,
                acknowledge_complete,
                submit_answer,
                submit_answer_text,
                crate::audio::play_sound_kind,
                get_sound_enabled,
                set_sound_enabled,
                get_audio_status
            ])
            .run(tauri::generate_context!())
            .expect("error while running tauri application");
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    native_app::run();
}

#[cfg(target_arch = "wasm32")]
fn main() {}
