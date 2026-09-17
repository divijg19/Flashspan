import { render, screen } from "@solidjs/testing-library";
import { describe, expect, it } from "vitest";
import App from "../App";
import { initializeRuntime } from "../runtime";
import { browserRuntime, maxTotalForDigits } from "../runtime/browser";
import { createMockRuntime } from "./__mocks__/mockRuntime";

describe("maxTotalForDigits bound table", () => {
	it("matches the exact-integer budget min(10000, floor((2^53-1)/(10^d-1)))", () => {
		const expected: Array<[number, number]> = [
			[1, 10000],
			[2, 10000],
			[3, 10000],
			[4, 10000],
			[5, 10000],
			[6, 10000],
			[7, 10000],
			[8, 10000],
			[9, 10000],
			[10, 10000],
			[11, 10000],
			[12, 9007],
			[13, 900],
			[14, 90],
			[15, 9],
		];
		for (const [digits, bound] of expected) {
			expect(maxTotalForDigits(digits)).toBe(bound);
		}
	});

	it("keeps every worst-case sum below 2^53", () => {
		const maxExact = 2 ** 53 - 1;
		for (let digits = 1; digits <= 15; digits += 1) {
			const maxMagnitude = digits <= 1 ? 9 : 10 ** digits - 1;
			expect(maxTotalForDigits(digits) * maxMagnitude).toBeLessThanOrEqual(
				maxExact,
			);
		}
	});
});

describe("browser session config bounds", () => {
	it("clamps digits to 15 and total to the digit-width bound", async () => {
		await browserRuntime.setSoundEnabled(false);
		const resp = await browserRuntime.startSession({
			digits_per_number: 20,
			number_duration_s: 0.5,
			delay_between_numbers_s: 0,
			total_numbers: 100,
			allow_negative_numbers: false,
		});
		try {
			expect(resp.effective_config.digits_per_number).toBe(15);
			expect(resp.effective_config.total_numbers).toBe(9);
			expect(resp.effective_config.delay_between_numbers_s).toBe(0.1);
		} finally {
			await browserRuntime.stopSession();
		}
	});

	it("caps the digits inputs at 15 in the UI", async () => {
		const mock = createMockRuntime();
		initializeRuntime(mock);
		const { container } = render(() => <App />);
		await new Promise((r) => setTimeout(r, 0));

		const digitsLabel = screen.getByText("Digits per number");
		expect(digitsLabel).toBeTruthy();
		const numberInputs = Array.from(
			container.querySelectorAll('input[type="number"]'),
		) as HTMLInputElement[];
		const rangeInputs = Array.from(
			container.querySelectorAll('input[type="range"]'),
		) as HTMLInputElement[];
		expect(numberInputs.map((i) => i.max)).toContain("15");
		expect(rangeInputs.map((i) => i.max)).toContain("15");
		expect(numberInputs.map((i) => i.max)).not.toContain("18");
		expect(rangeInputs.map((i) => i.max)).not.toContain("18");
	});

	it("preserves totals within the bound", async () => {
		await browserRuntime.setSoundEnabled(false);
		const resp = await browserRuntime.startSession({
			digits_per_number: 3,
			number_duration_s: 0.5,
			delay_between_numbers_s: 0,
			total_numbers: 500,
			allow_negative_numbers: false,
		});
		try {
			expect(resp.effective_config.total_numbers).toBe(500);
		} finally {
			await browserRuntime.stopSession();
		}
	});
});
