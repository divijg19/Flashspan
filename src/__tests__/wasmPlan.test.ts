import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { browserRuntime, planStepsToEvents } from "../runtime/browser";
import type { SessionConfigInput } from "../runtime/types";
import {
	registerWasmCoreBridge,
	type WasmCoreBridge,
	type WasmSessionPlan,
} from "../wasm/coreBridge";

const CONFIG_SNAPSHOT = {
	digits_per_number: 2,
	number_duration_s: 0.5,
	total_numbers: 2,
	allow_negative_numbers: false,
};

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

function fixedPlan(): WasmSessionPlan {
	return {
		session_id: 1,
		config_snapshot: { ...CONFIG_SNAPSHOT },
		steps: [
			clearStep(null),
			countdownStep("3"),
			countdownStep("2"),
			countdownStep("1"),
			showStep(1, 42, 42),
			clearStep(1),
			showStep(2, 17, 59),
			clearStep(2),
			clearStep(null),
			{
				Complete: { session_id: 1, numbers: [42, 17], sum: 59 },
			},
		],
		total_duration_ms: 4300,
		numbers_generated: [42, 17],
		expected_sum: 59,
	};
}

function stubBridge(
	buildSessionPlan: WasmCoreBridge["buildSessionPlan"],
): WasmCoreBridge {
	return {
		async normalizeSessionConfig(_input: SessionConfigInput) {
			throw new Error("not used in these tests");
		},
		buildSessionPlan,
	};
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
		registerWasmCoreBridge(stubBridge(async () => fixedPlan()));

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
		registerWasmCoreBridge(
			stubBridge(async () => {
				throw new Error("wasm unavailable");
			}),
		);

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
		// No case here supersedes the start, so null would be a contract break.
		if (!resp) throw new Error("startSession unexpectedly returned null");
		expect(resp.effective_config.total_numbers).toBe(1);

		await vi.advanceTimersByTimeAsync(5000);
		expect(completedCount).toBe(1);
		expect(completedNumbers).toHaveLength(1);
	});

	it("falls back to the JS planner on malformed steps", async () => {
		await browserRuntime.setSoundEnabled(false);
		const bad = fixedPlan();
		registerWasmCoreBridge(
			stubBridge(async () => ({
				...bad,
				steps: [{ Bogus: {} }] as never,
			})),
		);

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
		registerWasmCoreBridge(null as never);
		vi.useRealTimers();
	});

	it("fails a start whose plan length contradicts its config", async () => {
		await browserRuntime.setSoundEnabled(false);
		// The config asks for 3 numbers; the plan provides 2.
		registerWasmCoreBridge({
			async normalizeSessionConfig() {
				throw new Error("not used");
			},
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
