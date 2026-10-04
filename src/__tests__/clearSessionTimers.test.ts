import { describe, expect, it, vi } from "vitest";
import { __test_clearSessionTimers } from "../runtime/browser";

describe("clearSessionTimers", () => {
	it("clears the pending handle and prevents the callback from firing", () => {
		vi.useFakeTimers();

		const spy = vi.fn();
		const id = window.setTimeout(spy, 5000);

		const session = { timerId: id };
		__test_clearSessionTimers(session);

		expect(session.timerId).toBeNull();

		vi.advanceTimersByTime(20000);
		expect(spy).not.toHaveBeenCalled();

		vi.useRealTimers();
	});

	it("is a no-op when nothing is pending", () => {
		const session = { timerId: null };
		__test_clearSessionTimers(session);
		expect(session.timerId).toBeNull();
	});

	it("tolerates a stale handle without crashing", () => {
		const session = { timerId: 999999 };
		expect(() => __test_clearSessionTimers(session)).not.toThrow();
		expect(session.timerId).toBeNull();
	});
});
