import { describe, expect, it } from "vitest";
import {
	__test_setCompletedSession,
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
