import { describe, expect, it } from "vitest";
import {
	__test_setCompletedSession,
	deterministicFallback,
	generateNumber,
	parseProvidedAnswerText,
	parseWithNumberFallback,
	validateAnswer,
} from "../runtime/browser";

describe("parseWithNumberFallback (pre-BigInt browsers)", () => {
	it("grades exactly up to 15 digits", () => {
		expect(parseWithNumberFallback("42")).toBe(42);
		expect(parseWithNumberFallback("+123456789012345")).toBe(123456789012345);
		expect(parseWithNumberFallback("-17")).toBe(-17);
	});

	it("rejects longer inputs that native would grade incorrect", () => {
		expect(() => parseWithNumberFallback("1234567890123456")).toThrow(
			"Enter a single integer answer",
		);
		expect(() => parseWithNumberFallback("+1234567890123456")).toThrow(
			"Enter a single integer answer",
		);
	});
});

describe("generateNumber invariants (mirror of native generator)", () => {
	it("never emits zero, negatives when disabled, or consecutive duplicates", () => {
		for (const allowNegative of [false, true]) {
			let last: string | null = null;
			let runningSum = 0;
			for (let i = 0; i < 1000; i += 1) {
				const { payload, value } = generateNumber(
					3,
					allowNegative,
					i,
					runningSum,
					last,
				);
				expect(payload).not.toBe(last);
				if (i === 0) {
					expect(payload.startsWith("-")).toBe(false);
				}
				if (!allowNegative) {
					expect(value).toBeGreaterThanOrEqual(0);
				}
				runningSum = Math.max(0, runningSum + value);
				expect(runningSum).toBeGreaterThanOrEqual(0);
				last = payload;
			}
		}
	});

	it("uses 1..=9 magnitudes at digits=1 (no leading zero, no zero)", () => {
		let last: string | null = null;
		let runningSum = 0;
		for (let i = 0; i < 200; i += 1) {
			const { payload, value } = generateNumber(1, false, i, runningSum, last);
			expect(value).toBeGreaterThanOrEqual(1);
			expect(value).toBeLessThanOrEqual(9);
			expect(payload).not.toBe("0");
			runningSum += value;
			last = payload;
		}
	});
});

describe("parseProvidedAnswerText", () => {
	it("parses integers and trims whitespace", () => {
		expect(parseProvidedAnswerText("42")).toBe(42);
		expect(parseProvidedAnswerText("  -17 ")).toBe(-17);
		expect(parseProvidedAnswerText("1,234")).toBe(1234);
		expect(parseProvidedAnswerText("+42")).toBe(42);
		expect(parseProvidedAnswerText("-0")).toBe(0);
	});

	it("falls back gracefully without BigInt", () => {
		const realBigInt = globalThis.BigInt;
		Object.defineProperty(globalThis, "BigInt", {
			configurable: true,
			writable: true,
			value: undefined,
		});
		try {
			expect(parseProvidedAnswerText("1,234")).toBe(1234);
			expect(parseProvidedAnswerText("  -17 ")).toBe(-17);
			expect(() => parseProvidedAnswerText("42.9")).toThrow(
				"Enter a single integer answer",
			);
		} finally {
			Object.defineProperty(globalThis, "BigInt", {
				configurable: true,
				writable: true,
				value: realBigInt,
			});
		}
	});

	it("mirrors the native strict rule: same accepts and rejects", () => {
		// Accepted with identical values (mirror of main_tests vectors).
		const accepted: Array<[string, number]> = [
			["  42 ", 42],
			["1,234", 1234],
			["-42", -42],
			["  -17  ", -17],
			// Beyond f64 precision: accept/reject matches native; the
			// value rounds identically on both sides of the comparison.
			["9223372036854775807", Number(9223372036854775807n)],
			["-9223372036854775808", Number(-9223372036854775808n)],
			["  1,234,567  ", 1234567],
			["-9,876", -9876],
			["1,,2", 12],
		];
		for (const [input, expected] of accepted) {
			expect(parseProvidedAnswerText(input)).toBe(expected);
		}

		// Rejected on both runtimes: floats, scientific/hex notation,
		// internal whitespace, non-digits, empty, overlong, out of range.
		const rejected = [
			"",
			"   ",
			"abc",
			"12abc34",
			"not a number",
			"42.9",
			"-3.14",
			"42.0",
			"1e3",
			"0x10",
			"1_2",
			"1 2",
			"4\t2",
			"--42",
			"+-42",
			"1".repeat(65),
			"9223372036854775808",
			"-9223372036854775809",
		];
		for (const input of rejected) {
			expect(() => parseProvidedAnswerText(input)).toThrow(
				"Enter a single integer answer",
			);
		}
	});
});

describe("validateAnswer", () => {
	it("returns correct validation for matching sum", () => {
		__test_setCompletedSession(101, [1, 2, 3]);
		const resp = validateAnswer(101, 6);
		expect(resp.validation.correct).toBe(true);
		expect(resp.validation.expected_sum).toBe(6);
		expect(resp.validation.provided_sum).toBe(6);
		expect(resp.validation.delta).toBe(0);
	});

	it("reports delta for incorrect answers", () => {
		__test_setCompletedSession(202, [5, 5]);
		const resp = validateAnswer(202, 9);
		expect(resp.validation.expected_sum).toBe(10);
		expect(resp.validation.provided_sum).toBe(9);
		expect(resp.validation.delta).toBe(-1);
		expect(resp.validation.correct).toBe(false);
	});

	it("grades exactly at the largest allowed sums", () => {
		// 9 x (10^15 - 1): the digit-width bound maximum, still below 2^53.
		const wide = 999999999999999;
		const numbers = Array.from({ length: 9 }, () => wide);
		const sum = wide * 9;
		expect(sum).toBeLessThan(2 ** 53);
		__test_setCompletedSession(303, numbers);
		const resp = validateAnswer(303, sum);
		expect(resp.validation.expected_sum).toBe(sum);
		expect(resp.validation.provided_sum).toBe(sum);
		expect(resp.validation.delta).toBe(0);
		expect(resp.validation.correct).toBe(true);

		const off = validateAnswer(303, sum - 1);
		expect(off.validation.correct).toBe(false);
		expect(off.validation.delta).toBe(-1);
	});
});

describe("deterministicFallback", () => {
	it("strips minus for negative lastPayload", () => {
		const result = deterministicFallback("-5", 1, 10, false);
		expect(result.payload).toBe("5");
		expect(result.value).toBe(5);
	});

	it("applies modulo rotation for positive lastPayload", () => {
		const result = deterministicFallback("5", 1, 10, true);
		// With digits=1: maxExclusive=10, (5 % 9) + 1 = 6
		// runningSum 10 - 6 >= 0 → uses negative: "-6"
		expect(result.payload).toBe("-6");
		expect(result.value).toBe(-6);
	});

	it("never invents a negative when negatives are disabled", () => {
		const result = deterministicFallback("5", 1, 10, false);
		expect(result.payload).toBe("6");
		expect(result.value).toBe(6);
	});

	it("produces different payload from lastPayload for positive input", () => {
		const result = deterministicFallback("5", 1, 10, true);
		expect(result.payload).not.toBe("5");
	});

	it("respects runningSum constraint when next would exceed it", () => {
		// runningSum=3, next=6 → 3-6 < 0 → uses positive
		const result = deterministicFallback("5", 1, 3, true);
		expect(result.value).toBeGreaterThanOrEqual(0);
		expect(result.payload).toBe("6");
	});

	it("handles null lastPayload", () => {
		const result = deterministicFallback(null, 1, 0, false);
		expect(result).toEqual({ payload: "1", value: 1 });
	});

	it("never returns same as lastPayload for digits=2", () => {
		const result = deterministicFallback("50", 2, 100, true);
		expect(result.payload).not.toBe("50");
		// With digits=2: maxExclusive=100, (50 % 99) + 1 = 51
		expect(result.payload).toBe("-51");
		expect(result.value).toBe(-51);
	});

	it("handles zero as lastPayload", () => {
		const result = deterministicFallback("0", 1, 10, true);
		// (0 % 9) + 1 = 1, runningSum 10 - 1 >= 0 → "-1"
		expect(result.payload).toBe("-1");
	});
});
