import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { browserRuntime } from "../runtime/browser";
import type { SessionComplete } from "../runtime/types";
import type { WasmSessionPlan } from "../wasm/coreBridge";
import {
	clearWasmCoreBridge,
	fakeWasmPlan,
	registerFakeWasmBridge,
} from "./__mocks__/fakeWasmPlan";
import { startedOrFail } from "./helpers";

/**
 * The browser's session must grade and reveal the *intended plan*, not the
 * values a throttled tab happened to flash.
 *
 * This is the desktop/browser divergence that produced a genuinely wrong
 * answer key: grading used the values the browser happened to have shown rather
 * than the ones the session committed to, so a web user could be marked against
 * a total the desktop app would never accept. Native always validates the full
 * plan.
 *
 * These tests pin the fixed boundary: the answer key and the revealed series
 * come from the plan the runtime committed to at session start.
 */

/** Per-number exposure in these plans; matches the config's 0.5s duration. */
const EXPOSURE_MS = 500;

/**
 * The session config these cases start.
 *
 * Kept as a `SessionConfigInput` rather than folded into the plan fixture
 * because several cases pass it to `startSession` directly, and one of them
 * deliberately asks for a total the digit-width bound will clamp.
 */
const WASM_CONFIG = {
	digits_per_number: 2,
	number_duration_s: 0.5,
	total_numbers: 3,
	allow_negative_numbers: false,
} as const;

/**
 * A plan whose numbers do not sum to their running-sum arithmetic by accident.
 *
 * The shape comes from the shared `fakeWasmPlan` fixture; only the exposure is
 * parameterised, because these cases are specifically about how long each
 * number stays on screen.
 */
function planWith(
	numbers: number[],
	exposureMs = EXPOSURE_MS,
): WasmSessionPlan {
	return fakeWasmPlan(numbers, { exposureMs });
}

function stubBridge(numbers: number[], exposureMs?: number): void {
	registerFakeWasmBridge(async () => planWith(numbers, exposureMs));
}

function captureComplete(): {
	promise: Promise<SessionComplete>;
	unlisten: () => void;
} {
	let resolve!: (payload: SessionComplete) => void;
	const promise = new Promise<SessionComplete>((r) => {
		resolve = r;
	});
	let unlisten: () => void = () => {};
	void browserRuntime
		.onSessionComplete((payload) => {
			resolve(payload);
		})
		.then((fn) => {
			unlisten = fn;
		});
	return {
		promise,
		unlisten: () => unlisten(),
	};
}

describe("browser grades the intended plan", () => {
	beforeEach(() => {
		vi.useFakeTimers();
		vi.setSystemTime(0);
	});

	afterEach(async () => {
		await browserRuntime.stopSession();
		clearWasmCoreBridge();
		vi.useRealTimers();
		vi.restoreAllMocks();
	});

	it("reveals and grades the full plan numbers and sum", async () => {
		await browserRuntime.setSoundEnabled(false);
		const numbers = [10, 20, 30];
		registerFakeWasmBridge(async () => planWith(numbers));

		const completed = captureComplete();
		const session = await browserRuntime.startSession({ ...WASM_CONFIG }, null);

		const started = startedOrFail(session);
		// The whole schedule, plus the final settle.
		await vi.advanceTimersByTimeAsync(10_000);

		const payload = await completed.promise;
		expect(payload.numbers).toEqual(numbers);
		expect(payload.sum).toBe(60);

		// The answer key is the plan's sum, never the flashed subset.
		const right = await browserRuntime.submitAnswer(started.session_id, 60);
		expect(right.validation).toEqual({
			expected_sum: 60,
			provided_sum: 60,
			correct: true,
			delta: 0,
		});

		const wrong = await browserRuntime.submitAnswer(started.session_id, 0);
		expect(wrong.validation.correct).toBe(false);
		expect(wrong.validation.expected_sum).toBe(60);
		expect(wrong.validation.delta).toBe(-60);
	});

	it("keeps the revealed sum consistent with the revealed numbers", async () => {
		await browserRuntime.setSoundEnabled(false);
		// No bridge: the JS fallback planner must still build a full intended
		// plan, so grading has an authoritative sum even without WASM.
		clearWasmCoreBridge();

		const completed = captureComplete();
		const session = await browserRuntime.startSession(
			{
				digits_per_number: 2,
				number_duration_s: 0.3,
				total_numbers: 4,
				allow_negative_numbers: false,
			},
			null,
		);

		const started = startedOrFail(session);
		await vi.advanceTimersByTimeAsync(20_000);

		const payload = await completed.promise;
		expect(payload.numbers).toHaveLength(4);
		const sumOfRevealed = payload.numbers.reduce((a, b) => a + b, 0);
		expect(payload.sum).toBe(sumOfRevealed);

		const response = await browserRuntime.submitAnswer(
			started.session_id,
			payload.sum,
		);
		expect(response.validation.correct).toBe(true);
		expect(response.validation.expected_sum).toBe(payload.sum);
	});

	it("returns null from a duplicate acknowledgeComplete, like native", async () => {
		await browserRuntime.setSoundEnabled(false);
		registerFakeWasmBridge(async () => planWith([5, 5]));

		const completed = captureComplete();
		const session = await browserRuntime.startSession(
			{ ...WASM_CONFIG, total_numbers: 2 },
			{ enabled: true, repeats: 2, delay_s: 1 },
		);

		const started = startedOrFail(session);
		await vi.advanceTimersByTimeAsync(10_000);
		await completed.promise;

		const first = await browserRuntime.acknowledgeComplete(started.session_id);
		expect(first).not.toBeNull();
		expect(first?.session_id).toBe(started.session_id);

		const second = await browserRuntime.acknowledgeComplete(started.session_id);
		expect(second).toBeNull();
	});
});

/**
 * A session abandoned while the tab is hidden must be interrupted, not left
 * crawling. The driver chains fixed intervals, so a late delivery stretches the
 * session rather than dropping stimulus — which is correct per number, but
 * means a hidden tab (whose timers Chrome throttles to one wake per minute)
 * could otherwise run for hours with nothing to show.
 */
describe("hidden-tab interruption", () => {
	beforeEach(() => {
		vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "Date"] });
	});

	afterEach(async () => {
		clearWasmCoreBridge();
		vi.restoreAllMocks();
		vi.useRealTimers();
	});

	/** Puts the document into the given visibility state. */
	function setVisibility(state: "visible" | "hidden"): void {
		Object.defineProperty(document, "visibilityState", {
			configurable: true,
			get: () => state,
		});
		document.dispatchEvent(new Event("visibilitychange"));
	}

	/**
	 * Subscribes to interruptions and returns a counter plus the unsubscribe.
	 * The hook is optional on the Runtime interface, so bind defensively.
	 */
	async function watchInterruptions(): Promise<{
		count: () => number;
		unlisten: () => void;
	}> {
		let count = 0;
		const onInterrupted =
			browserRuntime.onSessionInterrupted?.bind(browserRuntime);
		const unlisten = onInterrupted
			? await onInterrupted(() => {
					count += 1;
				})
			: () => {};
		return { count: () => count, unlisten };
	}

	// A session long enough to still be running when the 30s threshold fires; a
	// 5s one would finish first and leave nothing to interrupt.
	const LONG = {
		digits_per_number: 2,
		number_duration_s: 60,
		total_numbers: 3,
		allow_negative_numbers: false,
	};

	it("interrupts a session hidden for longer than the threshold", async () => {
		await browserRuntime.setSoundEnabled(false);
		stubBridge([11, 22, 33], 60_000);

		const watcher = await watchInterruptions();
		const session = await browserRuntime.startSession({ ...LONG }, null);
		if (!session) throw new Error("startSession unexpectedly returned null");

		setVisibility("hidden");
		// Short of the session's own ~180s, past the 30s interruption threshold.
		await vi.advanceTimersByTimeAsync(30_000);
		expect(watcher.count()).toBe(0);

		setVisibility("visible");
		expect(watcher.count()).toBe(1);

		// The session is gone: grading is no longer possible, and a fresh start
		// is not blocked by the wreckage.
		await expect(
			browserRuntime.submitAnswer(session.session_id, 66),
		).rejects.toThrow();
		watcher.unlisten();
	});

	it("leaves a briefly hidden session running", async () => {
		await browserRuntime.setSoundEnabled(false);
		stubBridge([11, 22, 33], 60_000);

		const watcher = await watchInterruptions();
		const completed = captureComplete();
		const session = await browserRuntime.startSession({ ...LONG }, null);
		if (!session) throw new Error("startSession unexpectedly returned null");

		setVisibility("hidden");
		await vi.advanceTimersByTimeAsync(5_000);
		setVisibility("visible");
		expect(watcher.count()).toBe(0);

		// Still running: the driver stretched rather than abandoned it.
		await vi.advanceTimersByTimeAsync(200_000);
		const payload = await completed.promise;
		expect(payload.numbers).toEqual([11, 22, 33]);
		const graded = await browserRuntime.submitAnswer(session.session_id, 66);
		expect(graded.validation.correct).toBe(true);
		watcher.unlisten();
	});
});
