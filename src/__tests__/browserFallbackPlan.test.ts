import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { browserRuntime } from "../runtime/browser";
import type { SessionConfigInput } from "../runtime/types";

/**
 * With no WASM bridge the browser generates the session's numbers itself. That
 * local plan has to be built exactly once: it feeds both the events that are
 * flashed and the `plannedNumbers`/`plannedSum` used for grading and the answer
 * reveal. Building it twice produced two independent random sequences, so the
 * user was shown one set of numbers and then graded against another.
 *
 * Without a bridge this is the only number source, so the invariant is simply
 * that the sequence shown equals the sequence revealed. `Math.random` is
 * replaced with an advancing seeded PRNG: the run is reproducible, and the
 * second draw (were it taken) would necessarily differ from the first, turning
 * the old double-generation into a deterministic failure rather than a coin
 * flip.
 */

// mulberry32 — a tiny deterministic PRNG, one `Math.random()` call per draw.
function seededRandom(seed: number): () => number {
	let state = seed >>> 0;
	return () => {
		state = (state + 0x6d2b79f5) >>> 0;
		let t = state;
		t = Math.imul(t ^ (t >>> 15), t | 1);
		t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
		return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
	};
}

const CONFIG: SessionConfigInput = {
	digits_per_number: 1,
	number_duration_s: 0.5,
	total_numbers: 4,
	allow_negative_numbers: false,
};

// 3 countdown ticks x 1s + 100ms settle + 4 x (500ms show + 100ms gap) = 5.5s.
const SESSION_MS = 6000;

describe("browser JS planner fallback", () => {
	beforeEach(async () => {
		vi.useFakeTimers();
		vi.setSystemTime(0);
		vi.spyOn(Math, "random").mockImplementation(seededRandom(0x5eed));
		await browserRuntime.setSoundEnabled(false);
	});

	afterEach(async () => {
		await browserRuntime.stopSession();
		vi.useRealTimers();
		vi.restoreAllMocks();
	});

	it("flashes the same numbers it later reveals and grades", async () => {
		// The bridge is unregistered in this file, so `resolvePlannedSessionData`
		// returns null and the local JS planner supplies the whole plan.
		const flashed: number[] = [];
		await browserRuntime.onShowNumber((payload) => {
			flashed.push(payload.value);
		});

		const completed: { numbers: number[]; sum: number }[] = [];
		await browserRuntime.onSessionComplete((payload) => {
			completed.push({ numbers: payload.numbers, sum: payload.sum });
		});

		await browserRuntime.startSession(CONFIG);
		await vi.advanceTimersByTimeAsync(SESSION_MS);

		expect(completed).toHaveLength(1);
		const result = completed[0];

		// What the user saw is exactly what the answer key holds...
		expect(result.numbers).toEqual(flashed);
		expect(result.numbers).toHaveLength(CONFIG.total_numbers);
		// ...and the reported total is the total of that same sequence.
		expect(result.sum).toBe(flashed.reduce((acc, value) => acc + value, 0));
	});
});
