import { fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it } from "vitest";
import App from "../App";
import { initializeRuntime, resetRuntime } from "../runtime";
import { createMockRuntime } from "./__mocks__/mockRuntime";

/**
 * The end screen has to survive all the way to the revealed numbers.
 *
 * Nothing exercised this path through the UI before: every assertion about the
 * answer surface was made against a hand-built component, so the wiring between
 * the two answer modes, the footer, and the shared pieces had no coverage. That
 * is how the `AnswerNumbers` regression shipped — the button toggled its label
 * while the list it was meant to reveal stayed empty, and no test was looking.
 *
 * These cases drive the real app: switch mode through Advanced settings, finish
 * a session, act on the answer, then reveal. The type-mode case enables
 * auto-repeat too, because that is the flow the bug was originally reported in.
 */

function setRadio(container: HTMLElement, group: string, value: string): void {
	const groupElement = container.querySelector(`[aria-label="${group}"]`);
	const input = [...(groupElement?.querySelectorAll("input") ?? [])].find(
		(el) => (el as HTMLInputElement).value === value,
	);
	if (!input) throw new Error(`radio ${group}/${value} not found`);
	fireEvent.input(input, { target: { value } });
}

/**
 * Mount the app against a fresh mock runtime and dismiss the splash.
 *
 * `resetRuntime` rather than `vi.resetModules()`: the module graph is shared
 * with the components under test, so resetting it would hand a freshly imported
 * `App` a second copy of `solid-js` while this file renders the first.
 */
async function mountApp() {
	resetRuntime();
	const mock = createMockRuntime();
	initializeRuntime(mock);
	const rendered = render(() => <App />);
	await new Promise((resolve) => setTimeout(resolve, 0));
	window.dispatchEvent(new KeyboardEvent("keydown", { key: " " }));
	await new Promise((resolve) => setTimeout(resolve, 0));
	return { container: rendered.container, mock };
}

describe("session end screen", () => {
	afterEach(() => {
		resetRuntime();
	});

	it("type mode with auto-repeat: grades, arms the repeat, and reveals the numbers", async () => {
		const { container, mock } = await mountApp();
		mock.submitAnswerText = async () => ({
			validation: {
				expected_sum: 9,
				provided_sum: 9,
				correct: true,
				delta: 0,
			},
			auto_repeat_waiting: {
				session_id: 7,
				next_start_at_ms: Date.now() + 9000,
				remaining: 1,
			},
			message: "Correct ✅\nExpected answer: 9",
		});

		screen.getByRole("button", { name: "Additional settings" }).click();
		setRadio(container, "Answer mode", "type");
		setRadio(container, "Auto-repeat", "on");
		screen.getByRole("button", { name: "Close additional settings" }).click();

		mock.emitSessionComplete({ session_id: 7, numbers: [4, 5], sum: 9 });
		await screen.findByText("Type your answer");

		fireEvent.input(container.querySelector(".sumInput") as HTMLInputElement, {
			target: { value: "9" },
		});
		fireEvent.click(screen.getByRole("button", { name: "Validate" }));
		await new Promise((resolve) => setTimeout(resolve, 0));

		// The grading block.
		expect(container.querySelector(".validationText")?.textContent).toContain(
			"Correct ✅",
		);
		// The auto-repeat bar, armed by the response's waiting payload.
		expect(container.querySelector(".autoRepeatBar")).toBeTruthy();
		// The numbers toggle.
		expect(screen.getByRole("button", { name: "Show numbers" })).toBeTruthy();

		// ...and the list it reveals, which is what regressed.
		expect(container.querySelector(".answerNumbers")).toBeNull();
		fireEvent.click(screen.getByRole("button", { name: "Show numbers" }));
		await new Promise((resolve) => setTimeout(resolve, 0));
		expect(container.querySelectorAll(".answerRow")).toHaveLength(2);
	});

	it("reveal mode: Show answer reveals the numbers list", async () => {
		const { container, mock } = await mountApp();

		mock.emitSessionComplete({ session_id: 7, numbers: [4, 5], sum: 9 });
		await screen.findByText("Click to see answer");

		fireEvent.click(screen.getByRole("button", { name: "Show answer" }));
		await new Promise((resolve) => setTimeout(resolve, 0));

		expect(container.querySelector(".answerNumbers")).toBeNull();
		fireEvent.click(screen.getByRole("button", { name: "Show numbers" }));
		await new Promise((resolve) => setTimeout(resolve, 0));
		expect(container.querySelectorAll(".answerRow")).toHaveLength(2);
	});
});
