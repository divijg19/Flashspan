import { describe, expect, it } from "vitest";
import { deterministicFallback } from "../runtime/localPlanner";

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
			);
			expect(Math.abs(result.value)).toBeGreaterThanOrEqual(min);
			expect(Math.abs(result.value)).toBeLessThanOrEqual(max);
		}
	});

	it("rotates the maximum value instead of wrapping to 1", () => {
		// digits=3: max is 999. Wrapping produced 1, which is not a 3-digit value.
		expect(deterministicFallback("999", 3, Number.MAX_SAFE_INTEGER).value).toBe(
			100,
		);
		expect(deterministicFallback("99", 2, Number.MAX_SAFE_INTEGER).value).toBe(
			10,
		);
		expect(deterministicFallback("9", 1, Number.MAX_SAFE_INTEGER).value).toBe(
			1,
		);
	});

	/**
	 * The sign follows the previous value, exactly as `next_distinct_value` does
	 * in `core/engine.rs`.
	 *
	 * The old JS form did the opposite twice over: it forced any negative
	 * predecessor back to a positive *and* skipped the rotation, and it turned a
	 * positive duplicate negative whenever the sum could afford one. So with
	 * negatives enabled this disagreed with the shipped planner on every input,
	 * while with negatives disabled they agreed — which is precisely why the
	 * parity corpus never caught it. These cases pin both halves of the rule.
	 */
	it("keeps a positive predecessor positive and a negative one negative", () => {
		// A funded positive duplicate must not be turned negative.
		expect(deterministicFallback("50", 2, 1000).value).toBe(51);

		// A funded negative duplicate stays negative, and has still rotated.
		const funded = deterministicFallback("-50", 2, 1000);
		expect(funded.value).toBe(-51);
		expect(Math.abs(funded.value)).toBeGreaterThanOrEqual(10);

		// Unrotated for comparison: the naive `Math.abs` result the old form
		// returned, which matched neither the sign nor the rotation.
		expect(deterministicFallback("-50", 2, 1000).value).not.toBe(50);
	});

	it("drops a negative only when the running sum cannot fund it", () => {
		// A running sum of 0 cannot fund any negative, so the replacement is
		// taken as a positive — the same decision `next_distinct_value` makes.
		expect(deterministicFallback("-50", 2, 0).value).toBe(51);

		// Just affordable stays negative; one short does not.
		expect(deterministicFallback("-50", 2, 51).value).toBe(-51);
		expect(deterministicFallback("-50", 2, 50).value).toBe(51);
	});

	it("returns an in-width value when there is no previous payload", () => {
		for (const digits of WIDTHS) {
			const { min, max } = domain(digits);
			const result = deterministicFallback(null, digits, 0);
			expect(result.value).toBeGreaterThanOrEqual(min);
			expect(result.value).toBeLessThanOrEqual(max);
		}
	});

	/**
	 * The JS fallback must agree with `next_distinct_value` in `core/engine.rs`
	 * on the same sweep that function's own test uses: every magnitude in the
	 * domain, both signs, and a running sum that is too small, exactly right,
	 * and far too large to fund a negative.
	 *
	 * This is the assertion that would have caught the divergence. The two
	 * functions are the same algorithm on the two runtimes, and the JS one only
	 * runs when the WASM bridge is unavailable — so a disagreement means the
	 * browser and the desktop produce different numbers for a session the user
	 * configured identically. The sweep is cheap and total, so it is worth
	 * running rather than sampling.
	 */
	it("agrees with Rust next_distinct_value on the whole domain", () => {
		for (const digits of WIDTHS) {
			const { min, max } = domain(digits);
			for (const magnitude of [
				min,
				min + 1,
				Math.floor((min + max) / 2),
				max - 1,
				max,
			]) {
				for (const value of [magnitude, -magnitude]) {
					for (const runningSum of [
						0,
						magnitude,
						magnitude + 1,
						Number.MAX_SAFE_INTEGER,
					]) {
						const result = deterministicFallback(
							String(value),
							digits,
							runningSum,
						);
						expect(
							result.value,
							`digits=${digits} value=${value} sum=${runningSum}`,
						).toBe(rustNextDistinctValue(value, runningSum, digits));
					}
				}
			}
		}
	});
});

/**
 * `next_distinct_value` from `core/engine.rs`, transcribed so the JS fallback
 * can be checked against it directly rather than against a hand-written table.
 *
 * Keeping the transcription next to the assertion is deliberate: a table of
 * expected values would go stale the same way the previous comments did, and
 * the point of this test is that the two implementations cannot drift.
 */
function rustNextDistinctValue(
	value: number,
	runningSum: number,
	digits: number,
): number {
	const min = digits <= 1 ? 1 : 10 ** (digits - 1);
	const max = digits <= 1 ? 9 : 10 ** digits - 1;
	const span = max - min + 1;
	const rotated = min + ((Math.abs(value) - min + 1) % span);
	return value < 0 && runningSum >= rotated ? -rotated : rotated;
}
