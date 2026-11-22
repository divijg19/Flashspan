import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { browserRuntime } from "../runtime/browser";
import { normalizeAutoRepeat } from "../runtime/localPlanner";
import { startedOrFail } from "./helpers";

/**
 * Auto-repeat must terminate after the requested number of repeats.
 *
 * `normalizeAutoRepeat` used to clamp `repeats` up to 1, and the auto-repeat
 * restart feeds the post-decrement remaining value back through it, so a chain
 * that reached zero was clamped straight back to 1 and repeated forever. Native
 * stops because it only arms validation when repeats remain.
 *
 * The browser suite had no auto-repeat coverage at all, which is how that
 * survived; these cases pin the count for 1 and for 2.
 */

const BASE_CONFIG = {
	digits_per_number: 1,
	number_duration_s: 0.5,
	total_numbers: 2,
	allow_negative_numbers: false,
} as const;

// A 2-number session is 3.1s of countdown plus 2 x 500ms exposure = 5.1s;
// 6000ms leaves slack for timer delivery.
const SESSION_MS = 6000;

describe("auto-repeat", () => {
	beforeEach(() => {
		vi.useFakeTimers();
		vi.setSystemTime(0);
	});

	afterEach(async () => {
		await browserRuntime.stopSession();
		vi.useRealTimers();
		vi.restoreAllMocks();
	});

	it("schedules exactly one repeat when repeats is 1, then stops", async () => {
		await browserRuntime.setSoundEnabled(false);

		const completed: number[] = [];
		await browserRuntime.onSessionComplete((payload) => {
			completed.push(payload.session_id);
		});

		const started = startedOrFail(
			await browserRuntime.startSession(BASE_CONFIG, {
				enabled: true,
				repeats: 1,
				delay_s: 5,
			}),
		);

		await vi.advanceTimersByTimeAsync(SESSION_MS);
		expect(completed).toEqual([started.session_id]);

		// The completed session arms the last repeat and reports the post-decrement
		// remaining, matching native (`remaining` is returned after the decrement).
		const waiting = await browserRuntime.acknowledgeComplete(
			started.session_id,
		);
		expect(waiting).not.toBeNull();
		expect(waiting?.remaining).toBe(0);

		// Let the 5s delay elapse so the single repeat runs to completion.
		await vi.advanceTimersByTimeAsync(5000 + SESSION_MS);
		expect(completed).toHaveLength(2);

		// Nothing remains, so the chain ends instead of restarting.
		const repeatId = completed[1];
		expect(await browserRuntime.acknowledgeComplete(repeatId)).toBeNull();
		await vi.advanceTimersByTimeAsync(10000);
		expect(completed).toHaveLength(2);
	});

	it("schedules exactly two repeats when repeats is 2", async () => {
		await browserRuntime.setSoundEnabled(false);

		const completed: number[] = [];
		await browserRuntime.onSessionComplete((payload) => {
			completed.push(payload.session_id);
		});

		const started = startedOrFail(
			await browserRuntime.startSession(BASE_CONFIG, {
				enabled: true,
				repeats: 2,
				delay_s: 5,
			}),
		);
		await vi.advanceTimersByTimeAsync(SESSION_MS);

		const first = await browserRuntime.acknowledgeComplete(started.session_id);
		expect(first?.remaining).toBe(1);
		await vi.advanceTimersByTimeAsync(5000 + SESSION_MS);

		expect(completed).toHaveLength(2);
		const second = await browserRuntime.acknowledgeComplete(completed[1]);
		expect(second?.remaining).toBe(0);
		await vi.advanceTimersByTimeAsync(5000 + SESSION_MS);

		// Initial session plus exactly two repeats.
		expect(completed).toHaveLength(3);
		expect(await browserRuntime.acknowledgeComplete(completed[2])).toBeNull();
		await vi.advanceTimersByTimeAsync(10000);
		expect(completed).toHaveLength(3);
	});

	it("clamps a non-positive user-supplied count up to one, like native", async () => {
		await browserRuntime.setSoundEnabled(false);

		// Native `start_session` clamps `repeats` into 1..=20, so direct callers
		// get one repeat rather than none. Only the *internal* restart bypasses
		// that clamp; that bypass is what stops the chain.
		const completed: number[] = [];
		await browserRuntime.onSessionComplete((payload) => {
			completed.push(payload.session_id);
		});

		const started = startedOrFail(
			await browserRuntime.startSession(BASE_CONFIG, {
				enabled: true,
				repeats: 0,
				delay_s: 5,
			}),
		);
		await vi.advanceTimersByTimeAsync(SESSION_MS);

		const waiting = await browserRuntime.acknowledgeComplete(
			started.session_id,
		);
		expect(waiting).not.toBeNull();
		expect(waiting?.remaining).toBe(0);

		await vi.advanceTimersByTimeAsync(5000 + SESSION_MS);
		expect(completed).toHaveLength(2);
	});

	it("returns null for an unknown session id", async () => {
		expect(await browserRuntime.acknowledgeComplete(999)).toBeNull();
	});

	/**
	 * Leaving the end screen must cancel the pending restart, so a manual Start
	 * cannot race it. Pin the cancellation rather than the UI route.
	 */
	it("cancels the pending restart when the user leaves the end screen", async () => {
		await browserRuntime.setSoundEnabled(false);

		const completed: number[] = [];
		await browserRuntime.onSessionComplete((payload) => {
			completed.push(payload.session_id);
		});

		const started = startedOrFail(
			await browserRuntime.startSession(BASE_CONFIG, {
				enabled: true,
				repeats: 3,
				delay_s: 5,
			}),
		);
		await vi.advanceTimersByTimeAsync(SESSION_MS);
		expect(completed).toEqual([started.session_id]);

		// A repeat is genuinely pending: the end screen is showing its bar.
		const waiting = await browserRuntime.acknowledgeComplete(
			started.session_id,
		);
		expect(waiting, "a repeat should be armed at this point").not.toBeNull();
		expect(waiting?.remaining).toBe(2);

		// "Home" -> stop(). The pending restart dies here.
		await browserRuntime.stopSession();

		// Well past the 5s delay plus a whole session: nothing restarted.
		await vi.advanceTimersByTimeAsync(5000 + SESSION_MS);
		expect(completed).toEqual([started.session_id]);

		// And the manual Start that follows is not fighting anything.
		await expect(
			browserRuntime.startSession(BASE_CONFIG, {
				enabled: false,
				repeats: 0,
				delay_s: 5,
			}),
		).resolves.not.toBeNull();
	});

	/**
	 * The same property for the dedicated "Cancel auto-repeat" control, which
	 * clears the pending restart without ending the session. The user stays on the
	 * end screen, so this is the one path where a restart is abandoned rather than
	 * abandoned-by-teardown.
	 */
	it("cancels the pending restart without ending the session", async () => {
		await browserRuntime.setSoundEnabled(false);

		const completed: number[] = [];
		await browserRuntime.onSessionComplete((payload) => {
			completed.push(payload.session_id);
		});

		const started = startedOrFail(
			await browserRuntime.startSession(BASE_CONFIG, {
				enabled: true,
				repeats: 3,
				delay_s: 5,
			}),
		);
		await vi.advanceTimersByTimeAsync(SESSION_MS);

		const waiting = await browserRuntime.acknowledgeComplete(
			started.session_id,
		);
		expect(waiting?.remaining).toBe(2);

		await browserRuntime.cancelAutoRepeat();
		await vi.advanceTimersByTimeAsync(5000 + SESSION_MS);

		expect(completed).toEqual([started.session_id]);
	});
});

/**
 * The clamp vector table, mirroring the desktop's
 * `normalize_auto_repeat_clamps_and_reports_the_applied_values` in `main.rs`
 * field for field. The two implementations are separate — the browser cannot
 * call the native-only command — so the only thing keeping them honest is that
 * both are pinned to the same numbers.
 *
 * A pure-function table rather than a session run: `normalizeAutoRepeat` is
 * exported from `localPlanner` precisely so these can be asserted directly,
 * with no fake timers or live session.
 */
describe("normalizeAutoRepeat clamp vectors", () => {
	it("clamps a non-positive repeat count up to one and a short delay up to 5s", () => {
		expect(
			normalizeAutoRepeat({ enabled: true, repeats: 0, delay_s: 1 }),
		).toEqual({ enabled: true, repeats: 1, delay_s: 5 });
	});

	it("clamps an excessive repeat count to 20 and a long delay to 120s", () => {
		expect(
			normalizeAutoRepeat({ enabled: true, repeats: 999, delay_s: 1e9 }),
		).toEqual({ enabled: true, repeats: 20, delay_s: 120 });
	});

	it("falls back to the minimum delay for a non-finite one", () => {
		// Native guards with `is_finite`, so `+Infinity` must map to the minimum
		// rather than the maximum a bare clamp would produce.
		for (const delay_s of [
			Number.NaN,
			Number.POSITIVE_INFINITY,
			Number.NEGATIVE_INFINITY,
		]) {
			expect(
				normalizeAutoRepeat({ enabled: true, repeats: 3, delay_s }),
				`delay_s=${delay_s}`,
			).toEqual({ enabled: true, repeats: 3, delay_s: 5 });
		}
	});

	it("reports a fractional delay as the millisecond value that will elapse", () => {
		// Native reports `round(secs * 1000) / 1000`; a sub-millisecond input
		// must round the same way rather than pass through raw.
		const delay = normalizeAutoRepeat({
			enabled: true,
			repeats: 2,
			delay_s: 12.3456,
		});
		expect(delay?.delay_s).toBeDefined();
		expect(Math.round((delay?.delay_s ?? 0) * 1000)).toBe(12346);

		expect(
			normalizeAutoRepeat({ enabled: true, repeats: 2, delay_s: 12.34 }),
		).toEqual({ enabled: true, repeats: 2, delay_s: 12.34 });
	});

	it("returns null when the repeat is disabled or absent", () => {
		expect(normalizeAutoRepeat(null)).toBeNull();
		expect(normalizeAutoRepeat(undefined)).toBeNull();
		expect(
			normalizeAutoRepeat({ enabled: false, repeats: 5, delay_s: 30 }),
		).toBeNull();
	});
});
