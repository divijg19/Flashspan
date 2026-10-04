import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { browserRuntime } from "../runtime/browser";

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

// A 2-number session is 3.1s of countdown plus 2 x (500ms + 100ms) = 4.3s.
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

		const started = await browserRuntime.startSession(BASE_CONFIG, {
			enabled: true,
			repeats: 1,
			delay_s: 5,
		});

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

		const started = await browserRuntime.startSession(BASE_CONFIG, {
			enabled: true,
			repeats: 2,
			delay_s: 5,
		});
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

		const started = await browserRuntime.startSession(BASE_CONFIG, {
			enabled: true,
			repeats: 0,
			delay_s: 5,
		});
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
});
