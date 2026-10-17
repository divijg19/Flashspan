import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { browserRuntime, planStepsToEvents } from "../runtime/browser";
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

function showStep(
	index: number,
	value: number,
	runningSum: number,
): WasmSessionPlan["steps"][number] {
	return {
		ShowNumber: {
			session_id: 1,
			index,
			total: 2,
			value,
			running_sum: runningSum,
			delay_ms_before_next: 500,
		},
	};
}

function clearStep(index: number | null): WasmSessionPlan["steps"][number] {
	return {
		ClearScreen: {
			session_id: 1,
			index,
			delay_ms_before_next: index == null ? 0 : 100,
		},
	};
}

function countdownStep(value: string): WasmSessionPlan["steps"][number] {
	return {
		CountdownTick: {
			value,
			delay_ms_before_next: value === "1" ? 1100 : 1000,
		},
	};
}

/**
 * The two-number plan most of these cases drive.
 *
 * Built by the shared `fakeWasmPlan` fixture, which reproduces what
 * `build_session_plan` in `core/engine.rs` emits for a two-number session: the
 * countdown, then a show/clear pair per number, then the completion. Its
 * `total_duration_ms` of 4300 is the same value this used to hard-code.
 */
function fixedPlan(): WasmSessionPlan {
	return fakeWasmPlan([42, 17]);
}

describe("planStepsToEvents", () => {
	it("converts engine-order steps to the golden timeline", () => {
		const events = planStepsToEvents(fixedPlan().steps);
		// Driver event indices are 0-based; the emitter adds one back so the
		// payload matches Rust's 1-based `index`. The WASM and JS-planner paths
		// share this convention, so the first flash is `index: 1` on both.
		expect(events).toEqual([
			{ at: 0, kind: "countdown", value: "3" },
			{ at: 1000, kind: "countdown", value: "2" },
			{ at: 2000, kind: "countdown", value: "1" },
			{ at: 3100, kind: "show", index: 0, value: 42, runningSum: 42 },
			{ at: 3600, kind: "clear", index: 0 },
			{ at: 3700, kind: "show", index: 1, value: 17, runningSum: 59 },
			{ at: 4200, kind: "clear", index: 1 },
			{ at: 4300, kind: "finish" },
		]);
	});

	it("rejects a plan whose steps would fire two stimuli at once", () => {
		// A zero-delay ShowNumber would put the number up and down in the same
		// turn of the event loop, so it would never be seen. Rust's schedule
		// cannot produce one, but nothing in TypeScript enforced that, and the
		// driver's fixed-interval chaining has no remaining backstop.
		const collide = (): WasmSessionPlan["steps"] => [
			{ CountdownTick: { value: "1", delay_ms_before_next: 1100 } },
			{
				ShowNumber: {
					session_id: 1,
					index: 1,
					total: 1,
					value: 5,
					running_sum: 5,
					delay_ms_before_next: 0,
				},
			},
			{ ClearScreen: { session_id: 1, index: 1, delay_ms_before_next: 100 } },
			{ Complete: { session_id: 1, numbers: [5], sum: 5 } },
		];
		expect(planStepsToEvents(collide())).toBeNull();

		// The same shape with a real exposure is accepted.
		const fine = collide();
		const show = fine[1];
		if ("ShowNumber" in show) {
			show.ShowNumber.delay_ms_before_next = 500;
		}
		expect(planStepsToEvents(fine)).not.toBeNull();
	});

	it("rejects malformed step shapes", () => {
		expect(planStepsToEvents(null)).toBeNull();
		expect(planStepsToEvents([])).toBeNull();
		expect(planStepsToEvents([{}] as never)).toBeNull();
		expect(planStepsToEvents([{ Bogus: {} }] as never)).toBeNull();
		expect(
			planStepsToEvents([
				{ ShowNumber: { index: 1, value: 5, running_sum: 5 } },
			] as never),
		).toBeNull();
		expect(
			planStepsToEvents([
				clearStep(null),
				{ CountdownTick: { value: 3 } },
			] as never),
		).toBeNull();
	});

	/**
	 * Every remaining way a step can be rejected, as one table.
	 *
	 * Each of these is reachable from a foreign or corrupt wasm module rather
	 * than from our own planner, and each one must return null so the caller
	 * falls back to the local planner. A module that half-decodes would otherwise
	 * push a truncated schedule into the driver, and the driver's fixed-interval
	 * chaining turns a missing or malformed event into a stimulus that never
	 * fires — a session that starts and then goes silent, with nothing to show
	 * for it.
	 */
	it.each([
		// Two serde tags in one step is not a valid externally-tagged enum.
		"two tags in one step",
		"body is not a record",
		"ShowNumber with index below 1",
		"ShowNumber with a non-integer value",
		"ClearScreen with index below 1",
		"a plan with no numbers at all",
	])("rejects %s", (label) => {
		switch (label) {
			case "two tags in one step": {
				// The step must be valid under *either* tag, so that the only thing
				// that can reject it is the ambiguity itself. The trailing
				// ShowNumber also means `shows > 0`, so without the guard the
				// parser would take the last-matching tag and accept the plan
				// instead of falling back to the local planner.
				const ambiguous = {
					ShowNumber: {
						index: 1,
						value: 5,
						running_sum: 5,
						delay_ms_before_next: 500,
					},
					ClearScreen: {
						session_id: 1,
						index: 1,
						delay_ms_before_next: 100,
					},
				} as never;
				expect(
					planStepsToEvents([
						clearStep(null),
						ambiguous,
						showStep(2, 7, 12),
						clearStep(2),
					]),
				).toBeNull();
				break;
			}
			case "body is not a record":
				expect(planStepsToEvents([{ ShowNumber: 42 }] as never)).toBeNull();
				break;
			case "ShowNumber with index below 1":
				expect(
					planStepsToEvents([
						clearStep(null),
						{
							ShowNumber: {
								index: 0,
								value: 5,
								running_sum: 5,
								delay_ms_before_next: 500,
							},
						},
					] as never),
				).toBeNull();
				break;
			case "ShowNumber with a non-integer value":
				expect(
					planStepsToEvents([
						clearStep(null),
						{
							ShowNumber: {
								index: 1,
								value: "five",
								running_sum: 5,
								delay_ms_before_next: 500,
							},
						},
					] as never),
				).toBeNull();
				break;
			case "ClearScreen with index below 1":
				expect(
					planStepsToEvents([
						clearStep(null),
						{
							ClearScreen: {
								session_id: 1,
								index: 0,
								delay_ms_before_next: 100,
							},
						},
					] as never),
				).toBeNull();
				break;
			case "a plan with no numbers at all":
				// `shows === 0` is rejected: a plan of only countdown ticks would
				// otherwise drive a countdown and then a "finish" with nothing
				// flashed, which reads as a session that started and went blank.
				expect(
					planStepsToEvents([
						clearStep(null),
						countdownStep("3"),
						countdownStep("2"),
						countdownStep("1"),
					] as never),
				).toBeNull();
				break;
		}
	});
});

describe("WASM single-source execution", () => {
	beforeEach(() => {
		vi.useFakeTimers();
		vi.setSystemTime(0);
	});

	afterEach(async () => {
		await browserRuntime.stopSession();
		vi.useRealTimers();
		vi.restoreAllMocks();
	});

	it("plays planned values and sums on the golden timeline", async () => {
		await browserRuntime.setSoundEnabled(false);
		registerFakeWasmBridge(async () => fixedPlan());

		const shows: Array<{ value: number; runningSum: number; at: number }> = [];
		let completed: { numbers: number[]; sum: number } | null = null;
		await browserRuntime.onShowNumber((payload) => {
			shows.push({
				value: payload.value,
				runningSum: payload.running_sum,
				at: Date.now(),
			});
		});
		await browserRuntime.onSessionComplete((payload) => {
			completed = { numbers: payload.numbers, sum: payload.sum };
		});

		await browserRuntime.startSession({
			digits_per_number: 2,
			number_duration_s: 0.5,
			total_numbers: 2,
			allow_negative_numbers: false,
		});

		await vi.advanceTimersByTimeAsync(3100);
		expect(shows).toEqual([{ value: 42, runningSum: 42, at: 3100 }]);
		await vi.advanceTimersByTimeAsync(1100);
		expect(shows).toEqual([
			{ value: 42, runningSum: 42, at: 3100 },
			{ value: 17, runningSum: 59, at: 3700 },
		]);
		await vi.advanceTimersByTimeAsync(600);
		expect(completed).toEqual({ numbers: [42, 17], sum: 59 });
	});

	it("falls back to the JS planner when the bridge fails", async () => {
		await browserRuntime.setSoundEnabled(false);
		registerFakeWasmBridge(async () => {
			throw new Error("wasm unavailable");
		});

		let completedCount = 0;
		let completedNumbers: number[] = [];
		await browserRuntime.onSessionComplete((payload) => {
			completedCount += 1;
			completedNumbers = payload.numbers;
		});

		const resp = await browserRuntime.startSession({
			digits_per_number: 1,
			number_duration_s: 0.1,
			total_numbers: 1,
			allow_negative_numbers: false,
		});
		const started = startedOrFail(resp);
		expect(started.effective_config.total_numbers).toBe(1);

		await vi.advanceTimersByTimeAsync(5000);
		expect(completedCount).toBe(1);
		expect(completedNumbers).toHaveLength(1);
	});

	it("falls back to the JS planner on malformed steps", async () => {
		await browserRuntime.setSoundEnabled(false);
		const bad = fixedPlan();
		registerFakeWasmBridge(async () => ({
			...bad,
			steps: [{ Bogus: {} }] as never,
		}));

		let completedCount = 0;
		await browserRuntime.onSessionComplete(() => {
			completedCount += 1;
		});

		await browserRuntime.startSession({
			digits_per_number: 1,
			number_duration_s: 0.1,
			total_numbers: 1,
			allow_negative_numbers: false,
		});

		await vi.advanceTimersByTimeAsync(5000);
		expect(completedCount).toBe(1);
	});
});

/**
 * A plan whose length disagrees with the config it was built from must fail the
 * session, not flash a different count than the payload advertises.
 *
 * Rust always produces exactly `total_numbers` values, so this is only reachable
 * through a lying or corrupt bridge — but that is the same class of foreign-module
 * gap the `pushEvent` collision guard exists for, and the fallback planner is the
 * path a bridge failure lands on.
 */
describe("plan/config agreement", () => {
	beforeEach(() => {
		vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "Date"] });
	});

	afterEach(async () => {
		await browserRuntime.stopSession();
		clearWasmCoreBridge();
		vi.useRealTimers();
	});

	it("fails a start whose plan length contradicts its config", async () => {
		await browserRuntime.setSoundEnabled(false);
		// The config asks for 3 numbers; the plan provides 2.
		registerWasmCoreBridge({
			async buildSessionPlan() {
				const plan = fixedPlan();
				// The plan carries 2 numbers; the config asks for 3.
				plan.config_snapshot = {
					...plan.config_snapshot,
					total_numbers: 3,
				};
				return plan;
			},
		});

		await expect(
			browserRuntime.startSession(
				{
					digits_per_number: 1,
					number_duration_s: 0.5,
					total_numbers: 3,
					allow_negative_numbers: false,
				},
				null,
			),
		).rejects.toThrow();

		// Nothing is left running, so a subsequent start is clean.
		await vi.advanceTimersByTimeAsync(10_000);
		await expect(browserRuntime.submitAnswer(1, 3)).rejects.toThrow();
	});
});
