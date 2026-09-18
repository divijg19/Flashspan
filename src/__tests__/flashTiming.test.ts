import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { browserRuntime } from "../runtime/browser";

describe("flash timing: uniform exposure with fixed 100ms gap", () => {
	beforeEach(() => {
		vi.useFakeTimers();
		vi.setSystemTime(0);
	});

	afterEach(async () => {
		await browserRuntime.stopSession();
		vi.useRealTimers();
		vi.restoreAllMocks();
	});

	it("schedules countdown, uniform flashes, and 100ms blanks", async () => {
		await browserRuntime.setSoundEnabled(false);

		const countdowns: Array<{ value: string; at: number }> = [];
		const shows: Array<{ index: number; at: number }> = [];
		const indexedClears: Array<{ index: number; at: number }> = [];
		let completedAt: number | null = null;

		await browserRuntime.onCountdownTick((value) => {
			countdowns.push({ value, at: Date.now() });
		});
		await browserRuntime.onShowNumber((payload) => {
			shows.push({ index: payload.index, at: Date.now() });
		});
		await browserRuntime.onClearScreen((payload) => {
			if (payload.index != null) {
				indexedClears.push({ index: payload.index, at: Date.now() });
			}
		});
		await browserRuntime.onSessionComplete(() => {
			completedAt = Date.now();
		});

		await browserRuntime.startSession({
			digits_per_number: 1,
			number_duration_s: 0.5,
			total_numbers: 2,
			allow_negative_numbers: false,
		});

		// Countdown ticks at 0 / 1000 / 2000.
		await vi.advanceTimersByTimeAsync(0);
		expect(countdowns).toEqual([{ value: "3", at: 0 }]);
		await vi.advanceTimersByTimeAsync(1000);
		expect(countdowns.map((c) => c.value)).toEqual(["3", "2"]);
		await vi.advanceTimersByTimeAsync(1000);
		expect(countdowns.map((c) => c.value)).toEqual(["3", "2", "1"]);

		// 1000ms tick + 100ms pre-flash settle -> first show at 3100.
		await vi.advanceTimersByTimeAsync(1100);
		expect(shows).toEqual([{ index: 1, at: 3100 }]);

		// Uniform 500ms exposure for the first flash (no first-flash bonus).
		await vi.advanceTimersByTimeAsync(500);
		expect(indexedClears).toEqual([{ index: 1, at: 3600 }]);

		// Fixed 100ms blank before the second flash.
		await vi.advanceTimersByTimeAsync(100);
		expect(shows).toEqual([
			{ index: 1, at: 3100 },
			{ index: 2, at: 3700 },
		]);

		// Second exposure is identical to the first.
		await vi.advanceTimersByTimeAsync(500);
		expect(indexedClears).toEqual([
			{ index: 1, at: 3600 },
			{ index: 2, at: 4200 },
		]);

		// 100ms trailing gap, then completion.
		await vi.advanceTimersByTimeAsync(100);
		expect(completedAt).toBe(4300);
	});
});
