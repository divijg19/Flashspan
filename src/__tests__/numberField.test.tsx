import { fireEvent, render } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { describe, expect, it } from "vitest";
import { NumberField } from "../components/NumberField";

/**
 * The numeric settings fields must never put `NaN` into the session config.
 *
 * Clearing a `type="number"` input reports `valueAsNumber === NaN`, not `0` and
 * not an empty string. An unguarded read therefore writes `NaN` into the config,
 * which reaches `normalizeSessionConfig`, where `safeInt` turns it into `0` and
 * the clamped session silently becomes a one-number session with zero duration
 * — a session that finishes instantly and grades against nothing.
 *
 * The guard used to be written out inline six times across the three fields. It
 * now lives in one place inside `NumberField`, which is exactly why it needs its
 * own test: a pinch point with no coverage is a pinch point that can be removed
 * by accident.
 */

function numberInput(container: HTMLElement): HTMLInputElement {
	const input = container.querySelector<HTMLInputElement>(
		'input[type="number"]',
	);
	expect(input, "expected a number input").toBeTruthy();
	return input as HTMLInputElement;
}

function rangeInput(container: HTMLElement): HTMLInputElement {
	const input = container.querySelector<HTMLInputElement>(
		'input[type="range"]',
	);
	expect(input, "expected a range input").toBeTruthy();
	return input as HTMLInputElement;
}

/** Raw values recorded by the setter, so a stray `NaN` is visible directly. */
function harness(fallback: number) {
	const [value, setValue] = createSignal(fallback === 1 ? 3 : fallback);
	const seen: number[] = [];
	const set = (next: number): void => {
		seen.push(next);
		setValue(next);
	};
	const view = render(() => (
		<NumberField
			label="Digits per number"
			value={value}
			setValue={set}
			min={1}
			max={15}
			step={1}
			fallback={fallback}
			disabled={() => false}
		/>
	));
	return { ...view, seen, value };
}

describe("NumberField", () => {
	it("renders the label and bounds on both inputs", () => {
		const { getByText, container } = render(() => (
			<NumberField
				label="Digits per number"
				value={() => 3}
				setValue={() => {}}
				min={1}
				max={15}
				step={1}
				fallback={1}
				disabled={() => false}
			/>
		));

		expect(getByText("Digits per number")).toBeTruthy();
		for (const input of [numberInput(container), rangeInput(container)]) {
			expect(input.min).toBe("1");
			expect(input.max).toBe("15");
			expect(input.step).toBe("1");
		}
	});

	it("passes a typed value straight through", () => {
		const h = harness(1);
		fireEvent.input(numberInput(h.container), {
			target: { valueAsNumber: 7 },
		});

		expect(h.value()).toBe(7);
		expect(h.seen).not.toContain(Number.NaN);
	});

	/**
	 * The case the guard exists for. `valueAsNumber` is `NaN` when the field is
	 * empty, so an unchecked read would ship `NaN` into the config.
	 */
	it("falls back when the field is emptied, instead of writing NaN", () => {
		const h = harness(1);
		fireEvent.input(numberInput(h.container), {
			target: { valueAsNumber: Number.NaN },
		});

		// An emptied field must fall back to the minimum.
		expect(h.value()).toBe(1);
		expect(Number.isNaN(h.value())).toBe(false);
		// The setter must never even see a NaN: `safeInt` would later turn it into
		// 0, which clamps to a different value than the deliberate fallback.
		expect(h.seen).not.toContain(Number.NaN);
	});

	it("falls back on the range input too, not just the number box", () => {
		const h = harness(1);
		fireEvent.input(rangeInput(h.container), {
			target: { valueAsNumber: Number.NaN },
		});

		expect(h.value()).toBe(1);
		expect(h.seen).not.toContain(Number.NaN);
	});

	it("keeps the two inputs bound to the same value", () => {
		const h = harness(1);
		fireEvent.input(numberInput(h.container), {
			target: { valueAsNumber: 9 },
		});

		expect(rangeInput(h.container).value).toBe("9");
		expect(numberInput(h.container).value).toBe("9");
	});

	it("stands down while a session is running", () => {
		const { container } = render(() => (
			<NumberField
				label="Total numbers"
				value={() => 5}
				setValue={() => {}}
				min={1}
				max={1500}
				step={1}
				fallback={1}
				disabled={() => true}
			/>
		));

		for (const input of [numberInput(container), rangeInput(container)]) {
			expect(input.disabled).toBe(true);
		}
	});
});
