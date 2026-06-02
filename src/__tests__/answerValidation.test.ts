import { describe, expect, it } from "vitest";
import {
	__test_setCompletedSession,
	deterministicFallback,
	parseProvidedAnswerText,
	validateAnswer,
} from "../runtime/browser";

describe("parseProvidedAnswerText", () => {
	it("parses integers and trims whitespace", () => {
		expect(parseProvidedAnswerText("42")).toBe(42);
		expect(parseProvidedAnswerText("  -17 ")).toBe(-17);
		expect(parseProvidedAnswerText("1,234")).toBe(1234);
	});

	it("truncates floats", () => {
		expect(parseProvidedAnswerText("42.9")).toBe(42);
		expect(parseProvidedAnswerText("-3.14")).toBe(-3);
	});

	it("throws on empty or non-numeric input", () => {
		expect(() => parseProvidedAnswerText("")).toThrow();
		expect(() => parseProvidedAnswerText("   ")).toThrow();
		expect(() => parseProvidedAnswerText("abc")).toThrow();
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
});

describe("deterministicFallback", () => {
	it("strips minus for negative lastPayload", () => {
		const result = deterministicFallback("-5", 1, 10);
		expect(result.payload).toBe("5");
		expect(result.value).toBe(5);
	});

	it("applies modulo rotation for positive lastPayload", () => {
		const result = deterministicFallback("5", 1, 10);
		// With digits=1: maxExclusive=10, (5 % 9) + 1 = 6
		// runningSum 10 - 6 >= 0 → uses negative: "-6"
		expect(result.payload).toBe("-6");
		expect(result.value).toBe(-6);
	});

	it("produces different payload from lastPayload for positive input", () => {
		const result = deterministicFallback("5", 1, 10);
		expect(result.payload).not.toBe("5");
	});

	it("respects runningSum constraint when next would exceed it", () => {
		// runningSum=3, next=6 → 3-6 < 0 → uses positive
		const result = deterministicFallback("5", 1, 3);
		expect(result.value).toBeGreaterThanOrEqual(0);
		expect(result.payload).toBe("6");
	});

	it("handles null lastPayload", () => {
		const result = deterministicFallback(null, 1, 0);
		expect(result).toEqual({ payload: "1", value: 1 });
	});

	it("never returns same as lastPayload for digits=2", () => {
		const result = deterministicFallback("50", 2, 100);
		expect(result.payload).not.toBe("50");
		// With digits=2: maxExclusive=100, (50 % 99) + 1 = 51
		expect(result.payload).toBe("-51");
		expect(result.value).toBe(-51);
	});

	it("handles zero as lastPayload", () => {
		const result = deterministicFallback("0", 1, 10);
		// (0 % 9) + 1 = 1, runningSum 10 - 1 >= 0 → "-1"
		expect(result.payload).toBe("-1");
	});
});
