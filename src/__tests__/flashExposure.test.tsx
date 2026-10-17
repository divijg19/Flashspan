import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { browserRuntime } from "../runtime/browser";
import type {
	ClearScreen,
	ShowNumber,
	StartSessionResponse,
} from "../runtime/types";
import {
	registerWasmCoreBridge,
	type WasmSessionPlan,
} from "../wasm/coreBridge";
import {
	clearWasmCoreBridge,
	fakeWasmPlan,
	registerFakeWasmBridge,
} from "./__mocks__/fakeWasmPlan";
import { startedOrFail } from "./helpers";

/**
 * Every flashed number must stay on screen for at least its full exposure.
 *
 * This is the "first flash number is short lived" bug. The driver used to
 * schedule each event against an absolute grid anchored at session start, so
 * whatever lateness the *previous* event was delivered with was subtracted from
 * this one's window. The first show is the only one delivered alongside the
 * countdown's paint tail and the countdown->flashing view transition, so its
 * lateness truncated the first number's exposure while later numbers flashed
 * for the full duration.
 *
 * The driver now chains fixed intervals, which makes exposure exact by
 * construction: a late delivery can only delay when an event lands, never
 * shorten how long the previous one stayed up.
 *
 * Lateness is injected where a browser creates it — the timer fires later than
 * asked — by padding the delay the runtime requests. Exposure is then measured
 * from the payloads' own `emitted_at_ms` rather than from the test's tick
 * positions, so the assertion is about what a user would actually see.
 */

const CONFIG = {
	digits_per_number: 1,
	number_duration_s: 0.5,
	total_numbers: 3,
	allow_negative_numbers: false,
};

const EXPOSURE_MS = 500;

/** Blank gap between a clear and the next flash. `INTER_NUMBER_GAP_MS`. */
const GAP_MS = 100;

/**
 * The plan these cases drive, from the shared fixture.
 *
 * `digits` is passed because `CONFIG` asks for one-digit numbers and a
 * `config_snapshot` should describe the plan it accompanies; `exposureMs`
 * matches `EXPOSURE_MS`, which is what the config's 0.5s duration implies.
 */
function planFor(numbers: number[]): WasmSessionPlan {
	return fakeWasmPlan(numbers, { exposureMs: EXPOSURE_MS, digits: 1 });
}

/**
 * Records the stimulus stream and measures each number's on-screen duration
 * from the payload timestamps.
 */
function recorder() {
	const shows: ShowNumber[] = [];
	const clears: ClearScreen[] = [];
	const unsubscribers: Array<() => void> = [];
	return {
		shows,
		clears,
		async start(): Promise<void> {
			unsubscribers.push(
				await browserRuntime.onShowNumber((p) => shows.push(p)),
				await browserRuntime.onClearScreen((p) => clears.push(p)),
			);
		},
		stop(): void {
			for (const fn of unsubscribers) fn();
		},
		/** On-screen duration of each number, by its 1-based payload index. */
		exposures(): number[] {
			return shows.map((show) => {
				const clear = clears.find((c) => c.index === show.index);
				if (!clear) {
					throw new Error(`Number ${show.index} was never cleared`);
				}
				return clear.emitted_at_ms - show.emitted_at_ms;
			});
		},
	};
}

function stubBridge(numbers: number[]): void {
	registerFakeWasmBridge(async () => planFor(numbers));
}

describe("flashed number exposure", () => {
	let originalSetTimeout: typeof window.setTimeout;
	/** Lateness applied to the next N timer firings, in ms, then exhausted. */
	let pendingLag: number[];
	const scheduledDelays: number[] = [];

	beforeEach(() => {
		pendingLag = [];
		scheduledDelays.length = 0;
		vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "Date"] });
		originalSetTimeout = window.setTimeout.bind(window);
		// A browser fires a timer no earlier than asked; the pad models the
		// main thread being busy when the timer came due.
		vi.spyOn(window, "setTimeout").mockImplementation(((
			handler: TimerHandler,
			delay?: number,
		) => {
			scheduledDelays.push(Number(delay ?? 0));
			const lag = pendingLag.shift() ?? 0;
			return originalSetTimeout(
				handler,
				(delay ?? 0) + lag,
			) as unknown as number;
		}) as typeof window.setTimeout);
	});

	afterEach(async () => {
		await browserRuntime.stopSession();
		clearWasmCoreBridge();
		vi.restoreAllMocks();
		vi.useRealTimers();
	});

	async function run(): Promise<StartSessionResponse> {
		return startedOrFail(
			await browserRuntime.startSession({ ...CONFIG }, null),
		);
	}

	it("keeps the first number's exposure full despite late delivery", async () => {
		await browserRuntime.setSoundEnabled(false);
		stubBridge([4, 5, 6]);

		const record = recorder();
		await record.start();
		// The countdown's paint tail: the first show is delivered 60ms late,
		// which is what previously came straight out of its exposure.
		pendingLag = [1000, 1000, 60];

		const session = await run();
		await vi.advanceTimersByTimeAsync(20_000);

		expect(record.shows.map((s) => s.index)).toEqual([1, 2, 3]);
		expect(record.exposures()).toEqual([EXPOSURE_MS, EXPOSURE_MS, EXPOSURE_MS]);

		const graded = await browserRuntime.submitAnswer(session.session_id, 15);
		expect(graded.validation.correct).toBe(true);
	});

	it("never shortens any number's exposure, however large the lateness", async () => {
		await browserRuntime.setSoundEnabled(false);
		stubBridge([4, 5, 6]);

		const record = recorder();
		await record.start();
		// A lateness larger than the whole exposure, on every number.
		pendingLag = [1000, 1000, 700, 700, 700];

		const session = await run();
		await vi.advanceTimersByTimeAsync(30_000);

		expect(record.shows.map((s) => s.index)).toEqual([1, 2, 3]);
		for (const exposure of record.exposures()) {
			expect(exposure).toBeGreaterThanOrEqual(EXPOSURE_MS);
		}

		const graded = await browserRuntime.submitAnswer(session.session_id, 15);
		expect(graded.validation.correct).toBe(true);
	});

	it("drops no stimulus when a delivery is later than the whole exposure", async () => {
		await browserRuntime.setSoundEnabled(false);
		stubBridge([7, 8, 9]);

		const record = recorder();
		await record.start();
		// Far worse than the exposure: the session must stretch, not truncate.
		pendingLag = [1000, 1000, 6000];

		const session = await run();
		await vi.advanceTimersByTimeAsync(40_000);

		expect(record.shows.map((s) => s.value)).toEqual([7, 8, 9]);
		expect(record.exposures()).toEqual([EXPOSURE_MS, EXPOSURE_MS, EXPOSURE_MS]);
		const graded = await browserRuntime.submitAnswer(session.session_id, 24);
		expect(graded.validation.correct).toBe(true);
	});

	it("reports the first flash as index 1 on both planner paths", async () => {
		await browserRuntime.setSoundEnabled(false);

		// JS-fallback path: no bridge registered.
		clearWasmCoreBridge();
		const fallback = recorder();
		await fallback.start();
		await run();
		await vi.advanceTimersByTimeAsync(20_000);
		expect(fallback.shows.map((s) => s.index)).toEqual([1, 2, 3]);
		await browserRuntime.stopSession();
		fallback.stop();

		// WASM-plan path, carrying Rust's own 1-based indices in the steps.
		stubBridge([1, 2, 3]);
		const planned = recorder();
		await planned.start();
		await run();
		await vi.advanceTimersByTimeAsync(20_000);
		expect(planned.shows.map((s) => s.index)).toEqual([1, 2, 3]);
	});

	it("never schedules two stimuli in the same instant", async () => {
		await browserRuntime.setSoundEnabled(false);
		stubBridge([1, 2, 3]);

		const record = recorder();
		await record.start();
		await run();
		await vi.advanceTimersByTimeAsync(20_000);
		expect(record.shows).toHaveLength(3);

		// Every scheduled delay must be a real interval. Zero would fire two
		// stimuli in one turn of the event loop, which is what previously read as
		// a strobe; only the first turn of the chain is legitimately 0.
		const positive = scheduledDelays.filter((delay) => delay > 0);
		expect(positive.length).toBeGreaterThan(0);
		expect(Math.min(...positive)).toBeGreaterThanOrEqual(GAP_MS);
	});
});

/**
 * A stop that lands while the plan is still building must not start a session.
 *
 * `phase` is already `"starting"` while `resolvePlannedSessionData` is awaited,
 * and Escape is gated to include it, so this is reachable by pressing Escape
 * during start. Before the epoch guard the teardown cleared only the *outgoing*
 * session and the incoming one armed anyway: the UI returned to idle and then a
 * full countdown/flash/complete cycle ran unprompted.
 */
describe("stop during start", () => {
	beforeEach(() => {
		vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "Date"] });
	});

	afterEach(async () => {
		clearWasmCoreBridge();
		vi.restoreAllMocks();
		vi.useRealTimers();
	});

	it("starts nothing when the stop lands before the plan resolves", async () => {
		await browserRuntime.setSoundEnabled(false);
		// A bridge whose plan resolution is itself deferred, so the test can stop
		// the session in the window before it completes.
		let releasePlan: (plan: WasmSessionPlan) => void = () => {};
		registerWasmCoreBridge({
			buildSessionPlan() {
				return new Promise((resolve) => {
					releasePlan = resolve;
				});
			},
		});

		const record = recorder();
		await record.start();
		const startPromise = browserRuntime.startSession({ ...CONFIG }, null);
		// Let the teardown-before-await run, then stop before the plan resolves.
		await Promise.resolve();
		await Promise.resolve();
		await browserRuntime.stopSession();
		releasePlan(planFor([1, 2, 3]));

		const response = await startPromise;
		expect(response).toBeNull();

		await vi.advanceTimersByTimeAsync(60_000);
		expect(record.shows).toEqual([]);
		expect(record.clears).toEqual([]);
		expect(record.exposures()).toEqual([]);
	});

	it("still starts a session when nothing stops it", async () => {
		await browserRuntime.setSoundEnabled(false);
		stubBridge([1, 2, 3]);
		const record = recorder();
		await record.start();

		const response = await browserRuntime.startSession({ ...CONFIG }, null);
		expect(response).not.toBeNull();
		await vi.advanceTimersByTimeAsync(20_000);
		expect(record.shows.map((s) => s.value)).toEqual([1, 2, 3]);
	});
});
