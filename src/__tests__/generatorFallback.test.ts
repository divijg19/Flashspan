import { describe, expect, it } from "vitest";
import { deterministicFallback } from "../runtime/browser";

/**
 * The JS fallback planner's duplicate-value escape hatch must stay inside the
 * configured digit width.
 *
 * The previous form was `(lastVal % (maxExclusive - 1)) + 1`, which wrapped
 * `max` to `1` and put a one-digit value into a three-digit session. That is the
 * same defect the Rust core had (`engine.rs::next_distinct_value` now rotates
 * inside `magnitude_bounds`), and it mattered more than it looks: when the WASM
 * bridge is unavailable the JS planner is the only planner.
 */

const WIDTHS = [1, 2, 3, 5, 15] as const;

function domain(digits: number): { min: number; max: number } {
	return digits <= 1
		? { min: 1, max: 9 }
		: { min: 10 ** (digits - 1), max: 10 ** digits - 1 };
}

describe("deterministicFallback stays in the digit width", () => {
	it.each(WIDTHS)("keeps 1-digit values within 1..9 (width %i)", (digits) => {
		const { min, max } = domain(digits);
		for (const value of [
			min,
			min + 1,
			Math.floor((min + max) / 2),
			max - 1,
			max,
		]) {
			const result = deterministicFallback(
				String(value),
				digits,
				Number.MAX_SAFE_INTEGER,
				false,
			);
			expect(Math.abs(result.value)).toBeGreaterThanOrEqual(min);
			expect(Math.abs(result.value)).toBeLessThanOrEqual(max);
		}
	});

	it("rotates the maximum value instead of wrapping to 1", () => {
		// digits=3: max is 999. Wrapping produced 1, which is not a 3-digit value.
		expect(
			deterministicFallback("999", 3, Number.MAX_SAFE_INTEGER, false).value,
		).toBe(100);
		expect(
			deterministicFallback("99", 2, Number.MAX_SAFE_INTEGER, false).value,
		).toBe(10);
		expect(
			deterministicFallback("9", 1, Number.MAX_SAFE_INTEGER, false).value,
		).toBe(1);
	});

	it("always produces a value different from the one it replaced", () => {
		for (const digits of WIDTHS) {
			const { min, max } = domain(digits);
			for (const value of [min, max]) {
				const result = deterministicFallback(
					String(value),
					digits,
					Number.MAX_SAFE_INTEGER,
					false,
				);
				expect(Math.abs(result.value)).not.toBe(value);
			}
		}
	});

	it("never invents a negative the running sum cannot absorb", () => {
		// running_sum 0 cannot fund any negative.
		const result = deterministicFallback("50", 2, 0, true);
		expect(result.value).toBeGreaterThan(0);

		// A funded negative stays negative.
		const funded = deterministicFallback("50", 2, 1000, true);
		expect(funded.value).toBeLessThan(0);
		expect(Math.abs(funded.value)).toBeGreaterThanOrEqual(10);
	});

	it("returns an in-width value when there is no previous payload", () => {
		for (const digits of WIDTHS) {
			const { min, max } = domain(digits);
			const result = deterministicFallback(null, digits, 0, false);
			expect(result.value).toBeGreaterThanOrEqual(min);
			expect(result.value).toBeLessThanOrEqual(max);
		}
	});
});
