/**
 * The browser's local planner: the TypeScript mirror of the Rust core.
 *
 * Every value this module produces must agree with `src-tauri/src/core` value
 * for value, because it is the *only* planner when the WASM bridge is
 * unavailable. The wasm-parity suites assert that agreement against the real
 * Rust core (see `wasmParity.test.ts`), so the functions here are exported where
 * they are defined rather than wrapped in a seam.
 *
 * It is deliberately pure: no timers, DOM, storage or Web Audio. The mixed
 * browser runtime that drives a session lives in `browser.ts` and imports what
 * it needs from here; keeping the two apart makes the "mirror of Rust" boundary
 * explicit and lets this half be tested without the driver machinery.
 */

import type {
	AutoRepeatConfig,
	AutoRepeatEffective,
	SessionConfigEffective,
	SessionConfigInput,
} from "./types";

function clamp(value: number, min: number, max: number): number {
	if (Number.isNaN(value)) {
		return min;
	}

	return Math.min(max, Math.max(min, value));
}

function round1(value: number): number {
	return Math.round(value * 10) / 10;
}

function safeInt(value: number): number {
	if (!Number.isFinite(value)) {
		return 0;
	}

	return Math.trunc(value);
}

/**
 * Largest integer exactly representable in an f64 (2^53 - 1). The browser
 * accumulates sums in f64, so the worst-case session sum must stay below
 * this for grading to be exact. Must mirror Rust `MAX_EXACT_INTEGER`.
 */
const MAX_EXACT_INTEGER = 2 ** 53 - 1;
const MAX_TOTAL_NUMBERS = 10000;

/**
 * Maximum numbers for a digit width such that even the worst case (every
 * number at maximum magnitude) sums below MAX_EXACT_INTEGER. Must mirror
 * Rust `max_total_for_digits`.
 */
export function maxTotalForDigits(digits: number): number {
	const maxMagnitude = digits <= 1 ? 9 : 10 ** digits - 1;
	return Math.max(
		1,
		Math.min(MAX_TOTAL_NUMBERS, Math.floor(MAX_EXACT_INTEGER / maxMagnitude)),
	);
}

/**
 * Normalize a session config for the browser.
 *
 * Exported for the JS/WASM parity test, which asserts this agrees with the Rust
 * normalizer on an adversarial input table. The comment-based protocol between
 * the two is not sufficient on its own: the auto-repeat decrement and the
 * duplicate-value fallback both drifted without any test failing.
 */
export function normalizeSessionConfig(
	input: SessionConfigInput,
): SessionConfigEffective {
	// Policy cap: 15 digits keeps every value exactly representable in f64.
	const digits = clamp(safeInt(input.digits_per_number), 1, 15);
	return {
		digits_per_number: digits,
		number_duration_s: round1(clamp(input.number_duration_s, 0.1, 60)),
		// No inter-number gap field: the gap is fixed (INTER_NUMBER_GAP_MS).
		total_numbers: Math.min(
			clamp(safeInt(input.total_numbers), 1, MAX_TOTAL_NUMBERS),
			maxTotalForDigits(digits),
		),
		allow_negative_numbers: Boolean(input.allow_negative_numbers),
	};
}

/** Auto-repeat bounds, mirroring `MIN_REPEATS`/`MAX_REPEATS` etc. in `main.rs`. */
const MIN_REPEATS = 1;
const MAX_REPEATS = 20;
const MIN_REPEAT_DELAY_S = 5;
const MAX_REPEAT_DELAY_S = 120;

/**
 * Clamp a requested auto-repeat config the way the native `start_session` does.
 *
 * Must stay identical to Rust `normalize_auto_repeat`, which is pinned by the
 * same vector table on both sides (`autoRepeat.test.ts` for the web,
 * `main.rs::normalize_auto_repeat_clamps_and_reports_the_applied_values` for the
 * desktop). Exported so that table can reach it directly.
 */
export function normalizeAutoRepeat(
	autoRepeat?: AutoRepeatConfig | null,
): AutoRepeatEffective | null {
	if (!autoRepeat?.enabled) {
		return null;
	}

	// Mirror native `normalize_auto_repeat` exactly:
	//
	// - `repeats` clamps to 1..=20.
	// - A non-finite delay falls back to the *minimum*, matching native's
	//   `is_finite` guard. A plain `clamp` would map `+Infinity` to the maximum
	//   (120) while native reports the minimum (5), so an API caller that sent
	//   `Infinity` got two different delays on the two runtimes.
	// - The reported seconds are derived from the millisecond value that will
	//   actually elapse, so a sub-millisecond input cannot make the web and the
	//   desktop disagree about the delay shown in the UI. `delay_s` is
	//   deliberately NOT rounded to one decimal first: native rounds to the
	//   nearest millisecond (`(secs * 1000).round()`), so rounding to 0.1s would
	//   turn a typed `12.34` into 12,300ms on the web and 12,340ms on the desktop.
	const delaySecs = Number.isFinite(autoRepeat.delay_s)
		? clamp(autoRepeat.delay_s, MIN_REPEAT_DELAY_S, MAX_REPEAT_DELAY_S)
		: MIN_REPEAT_DELAY_S;

	return {
		enabled: true,
		repeats: clamp(safeInt(autoRepeat.repeats), MIN_REPEATS, MAX_REPEATS),
		delay_s: Math.round(delaySecs * 1000) / 1000,
	};
}

function randomInt(maxExclusive: number): number {
	if (maxExclusive <= 1) {
		return 0;
	}

	return Math.floor(Math.random() * maxExclusive);
}

function randomMagnitude(digits: number): number {
	if (digits <= 1) {
		// No leading zero: 1..=9, mirroring the native generator.
		return 1 + randomInt(9);
	}

	const min = 10 ** (digits - 1);
	const span = 10 ** digits - min;
	return min + randomInt(span);
}

export function generateNumber(
	digits: number,
	allowNegative: boolean,
	index: number,
	runningSum: number,
	lastPayload: string | null,
): { payload: string; value: number } {
	let attempt = 0;

	while (attempt < 256) {
		const magnitude = randomMagnitude(digits);
		const negativeAllowed = allowNegative && index > 0;
		const signedValue =
			negativeAllowed && randomInt(2) === 1 ? -magnitude : magnitude;

		if (index === 0 && signedValue < 0) {
			attempt += 1;
			continue;
		}

		if (runningSum + signedValue < 0) {
			attempt += 1;
			continue;
		}

		const payload = String(signedValue);
		if (payload === lastPayload) {
			attempt += 1;
			continue;
		}

		return { payload, value: signedValue };
	}

	return deterministicFallback(lastPayload, digits, runningSum);
}

/**
 * Deterministic replacement for a generated value that equals the previous one:
 * rotate the magnitude one step inside its own digit-width domain.
 *
 * This mirrors `next_distinct_value` in `core/engine.rs` and is not free to
 * differ from it. The JS planner runs whenever the WASM bridge is unavailable,
 * so a disagreement means the browser and the desktop produce different number
 * sequences for the same config — and a different sign on a flash — under a
 * session the user configured identically.
 *
 * Two rules from the Rust original, both of which this previously got wrong:
 *
 * 1. The sign follows the **previous** value. A negative predecessor stays
 *    negative and a positive one stays positive. The old form returned
 *    `Math.abs(lastVal)` for any negative predecessor, which both dropped the
 *    sign *and* skipped the rotation, so it disagreed with Rust on magnitude as
 *    well as sign.
 * 2. A negative result survives only while the running sum can still fund it.
 *    The old form returned `-next` for *any* positive predecessor whenever
 *    negatives were enabled, so a positive duplicate became a negative.
 *
 * Together those meant that whenever a session allowed negatives — the only
 * configuration where the sign can differ — this function disagreed with the
 * planner that actually ships in the WASM bundle on every reachable input.
 * With negatives disabled the result is always positive on both sides, which
 * is why the parity corpus never caught it.
 */
export function deterministicFallback(
	lastPayload: string | null,
	digits: number,
	runningSum: number,
): { payload: string; value: number } {
	if (lastPayload == null) {
		// No previous value to rotate from. Unreachable from the generator (the
		// fallback only runs on a repeat), but keep it inside the digit width
		// rather than hard-coding a one-digit value.
		const min = digits <= 1 ? 1 : 10 ** (digits - 1);
		return { payload: String(min), value: min };
	}

	const lastVal = Number(lastPayload);

	// Rotate the magnitude one step inside its own digit-width domain. The
	// previous form wrapped to `(max - 1) + 1 = 1`, which put a one-digit value
	// in a three-digit session and silently dropped the configured digit width.
	const min = digits <= 1 ? 1 : 10 ** (digits - 1);
	const max = digits <= 1 ? 9 : 10 ** digits - 1;
	// `Math.max(0, …)` mirrors Rust's `saturating_sub`: without it a magnitude
	// below the digit-width floor (only 0, which the generator never emits but
	// this function is still tested with) makes the subtraction negative, and JS
	// `%` keeps that sign, so the result falls outside the domain where
	// `next_distinct_value` clamps back into it.
	const next =
		min + ((Math.max(0, Math.abs(lastVal) - min) + 1) % (max - min + 1));

	// Keep the negative form only while the sum can absorb it, exactly as
	// `next_distinct_value` does. `allowNegative` is not a parameter here for the
	// same reason it is not one there: a negative payload can only have been
	// generated when negatives were already enabled, so an unaffordable negative
	// is the only case that needs a decision, and it resolves to a positive.
	if (lastVal < 0 && runningSum - next >= 0) {
		return { payload: String(-next), value: -next };
	}
	return { payload: String(next), value: next };
}

/**
 * The fallback session plan, generated once up front when no Rust plan is
 * available (the WASM bridge failed to load, or its steps did not validate).
 *
 * The values are generated eagerly rather than lazily at fire time: the driver's
 * fixed-interval chaining means nothing is ever dropped, so the full sequence is
 * always shown, and grading plus the answer reveal must both be able to see it.
 * Generating lazily could only ever know a partial sum.
 *
 * Callers build this plan exactly once and reuse it for both the flashed events
 * and the stored `plannedNumbers`/`plannedSum`. Building it twice produced two
 * independent random sequences, so the browser flashed one set of numbers and
 * then graded against another.
 */
export function generateLocalPlan(config: SessionConfigEffective): {
	numbers: number[];
	sum: number;
} {
	const numbers: number[] = [];
	let running = 0;
	let lastPayload: string | null = null;
	for (let index = 0; index < config.total_numbers; index += 1) {
		const generated = generateNumber(
			config.digits_per_number,
			config.allow_negative_numbers,
			index,
			running,
			lastPayload,
		);
		lastPayload = generated.payload;
		running += generated.value;
		numbers.push(generated.value);
	}
	return { numbers, sum: running };
}

/**
 * Strict answer rule, mirroring Rust `parse_answer_text` in
 * `src-tauri/src/main.rs` exactly: trim ends, strip commas everywhere,
 * optional single sign, ASCII digits only, 64-char cap, i64 range.
 * `BigInt` keeps accept/reject identical to Rust on every input (including
 * beyond f64 precision); the return is exact for every attainable session
 * sum, which the digit-width bound keeps below 2^53.
 */
const INVALID_ANSWER_MESSAGE =
	"Enter a single integer answer (e.g. 42 or -17).";
const I64_MIN = -(2n ** 63n);
const I64_MAX = 2n ** 63n - 1n;

function hasBigInt(): boolean {
	try {
		return typeof BigInt === "function";
	} catch {
		return false;
	}
}

/**
 * Pre-BigInt fallback: exact for inputs up to 15 digits (every attainable
 * session sum), since those fit f64 exactly. Longer inputs are rejected
 * here while native would grade them incorrect — a documented deviation
 * confined to obsolete browsers and unanswerable inputs.
 */
export function parseWithNumberFallback(cleaned: string): number {
	if (cleaned.replace(/^[+-]/, "").length > 15) {
		throw new Error(INVALID_ANSWER_MESSAGE);
	}
	const parsed = Number(cleaned);
	if (!Number.isSafeInteger(parsed)) {
		throw new Error(INVALID_ANSWER_MESSAGE);
	}
	return parsed;
}

export function parseProvidedAnswerText(value: string): number {
	const cleaned = value.trim().replace(/,/g, "");
	if (!cleaned || cleaned.length > 64) {
		throw new Error(INVALID_ANSWER_MESSAGE);
	}
	if (!/^[+-]?[0-9]+$/.test(cleaned)) {
		throw new Error(INVALID_ANSWER_MESSAGE);
	}
	if (!hasBigInt()) {
		return parseWithNumberFallback(cleaned);
	}
	const big = BigInt(cleaned);
	if (big < I64_MIN || big > I64_MAX) {
		throw new Error(INVALID_ANSWER_MESSAGE);
	}
	return Number(big);
}
