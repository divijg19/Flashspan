import { render } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { describe, expect, it, vi } from "vitest";
import { AutoRepeatBar } from "../App";

/**
 * The auto-repeat fill is one continuous ramp, not a per-tick step.
 *
 * It works because the animation duration is captured once, when the countdown
 * arms, and never derived from the ticking `seconds_left`. A changing
 * `animation-duration` restarts a CSS animation, so binding it to the tick would
 * silently turn the smooth fill back into 20 discrete jumps over a 20s delay,
 * with nothing in a snapshot test to show for it.
 */

function fillOf(container: HTMLElement): HTMLElement {
	const fill = container.querySelector(".autoRepeatProgressFill");
	expect(fill, "expected the progress fill").toBeTruthy();
	return fill as HTMLElement;
}

describe("auto-repeat progress ramp", () => {
	it("animates over the countdown's remaining time, not a nominal delay", () => {
		const { container } = render(() => (
			<AutoRepeatBar
				secondsLeft={5}
				remaining={2}
				fillMs={4820}
				onCancel={() => {}}
			/>
		));

		expect(container.textContent).toContain("Next question in 5s");
		expect(container.textContent).toContain("3 remaining");
		expect(fillOf(container).getAttribute("style")).toContain("4820ms");
	});

	it("keeps the duration stable as the countdown ticks", () => {
		const [secondsLeft, setSecondsLeft] = createSignal(5);
		const { container } = render(() => (
			<AutoRepeatBar
				secondsLeft={secondsLeft()}
				remaining={2}
				fillMs={4820}
				onCancel={() => {}}
			/>
		));

		const before = fillOf(container).getAttribute("style");
		expect(before).toContain("4820ms");

		// Each tick changes only the label; the ramp must keep running.
		for (const value of [4, 3, 2, 1, 0]) {
			setSecondsLeft(value);
			expect(container.textContent).toContain(`Next question in ${value}s`);
			expect(fillOf(container).getAttribute("style")).toBe(before);
		}
	});

	it("invokes the cancel handler", () => {
		const onCancel = vi.fn();
		const { getByRole } = render(() => (
			<AutoRepeatBar
				secondsLeft={5}
				remaining={0}
				fillMs={5000}
				onCancel={onCancel}
			/>
		));

		getByRole("button", { name: /cancel auto-repeat/i }).click();
		expect(onCancel).toHaveBeenCalledTimes(1);
	});
});
