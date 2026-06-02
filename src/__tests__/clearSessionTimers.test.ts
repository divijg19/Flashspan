import { describe, expect, it, vi } from "vitest";
import { __test_clearSessionTimers } from "../runtime/browser";

describe("clearSessionTimers", () => {
	it("clears timer IDs and prevents callbacks from firing", () => {
		vi.useFakeTimers();

		const spy1 = vi.fn();
		const spy2 = vi.fn();
		const id1 = window.setTimeout(spy1, 5000);
		const id2 = window.setTimeout(spy2, 10000);

		const session = { timers: [id1, id2] };
		__test_clearSessionTimers(session);

		expect(session.timers).toHaveLength(0);

		// Advance time past both timeouts
		vi.advanceTimersByTime(20000);
		expect(spy1).not.toHaveBeenCalled();
		expect(spy2).not.toHaveBeenCalled();

		vi.useRealTimers();
	});

	it("handles empty timers array", () => {
		const session = { timers: [] as number[] };
		__test_clearSessionTimers(session);
		expect(session.timers).toHaveLength(0);
	});

	it("handles invalid timer IDs without crashing", () => {
		const session = { timers: [-1, 0, 999999] };
		expect(() => __test_clearSessionTimers(session)).not.toThrow();
		expect(session.timers).toHaveLength(0);
	});
});
