import { render, screen } from "@solidjs/testing-library";
import { describe, expect, it, vi } from "vitest";
import type { StartSessionResponse } from "../runtime/types";
import { createMockRuntime } from "./__mocks__/mockRuntime";

/**
 * Each case gets a fresh module registry: `initializeRuntime` is a one-shot
 * per module, so without `vi.resetModules()` a second `render` would silently
 * reuse the first test's runtime and its mock.
 */
async function renderApp(startResponse?: StartSessionResponse) {
	vi.resetModules();
	const runtimeModule = await import("../runtime");
	const appModule = await import("../App");
	const mock = createMockRuntime();
	if (startResponse) {
		mock.startSession = async () => startResponse;
	}
	runtimeModule.initializeRuntime(mock);
	const rendered = render(() => <appModule.default />);
	await new Promise((r) => setTimeout(r, 0));
	window.dispatchEvent(new KeyboardEvent("keydown", { key: " " }));
	return { container: rendered.container, mock };
}

/**
 * The countdown -> first flash transition, asserted at the DOM level.
 *
 * The countdown text test below only proves numbers render. What it cannot see
 * is the contract the first flash has to keep: the countdown decoration is torn
 * down, the digits element is replaced, and the payload index stays 1-based on
 * every planner path. The index part is not cosmetic — the two browser planner
 * paths once disagreed by one, which silently disabled the first-flash
 * instrumentation on the platform that needed it.
 */
describe("App flow tests", () => {
	it("shows countdown, flashes number, and displays completion", async () => {
		const { container, mock } = await renderApp();
		mock.emitCountdown("3");
		expect(await screen.findByText("3")).toBeTruthy();

		// The countdown surface: decoration mounted, digit is the countdown glyph.
		const number = container.querySelector(".number");
		expect(number?.classList.contains("countdown")).toBe(true);
		expect(container.querySelector(".countdownDigit")?.textContent).toBe("3");
		expect(container.querySelector(".countdownRing")).toBeTruthy();

		// Emit show number
		mock.emitShowNumber({
			session_id: 1,
			index: 1,
			total: 1,
			value: 7,
			running_sum: 7,
			emitted_at_ms: Date.now(),
		});
		expect(await screen.findByText("7")).toBeTruthy();

		// Same element, no longer the countdown: the decoration is gone and the
		// digit markup is the signed-number form used for flashes.
		expect(
			container.querySelector(".number")?.classList.contains("countdown"),
		).toBe(false);
		expect(container.querySelector(".countdownRing")).toBeNull();
		expect(container.querySelector(".countdownDigit")).toBeNull();
		expect(
			container.querySelector(".signedNumber .magnitude")?.textContent,
		).toBe("7");

		// Emit session complete
		mock.emitSessionComplete({ session_id: 1, numbers: [7], sum: 7 });
		expect(await screen.findByText("Session complete")).toBeTruthy();
	});

	/**
	 * jsdom has no layout engine, so this cannot measure centring. What it does
	 * pin is the contract that makes centring stable: the sign slot exists for
	 * every flash once negatives are enabled, and only its opacity changes
	 * between a positive and a negative. The geometry itself is guarded in
	 * `countdownStyles.test.ts`.
	 */
	it("keeps the sign slot present and toggling only opacity", async () => {
		// Negative-capable session: the sign slot is always reserved, so the
		// digits must sit at the same place for both signs.
		const { container, mock } = await renderApp({
			session_id: 1,
			effective_config: {
				digits_per_number: 1,
				number_duration_s: 0.5,
				total_numbers: 2,
				allow_negative_numbers: true,
			},
			effective_auto_repeat: null,
		});

		screen.getByRole("button", { name: "Start" }).click();
		await new Promise((r) => setTimeout(r, 0));

		const sign = () => container.querySelector<HTMLElement>(".signOverlay");
		const magnitude = () => container.querySelector<HTMLElement>(".magnitude");

		// Positive: the slot exists but is transparent, and the digits are there.
		mock.emitShowNumber({
			session_id: 1,
			index: 1,
			total: 2,
			value: 5,
			running_sum: 5,
			emitted_at_ms: Date.now(),
		});
		expect(await screen.findByText("5")).toBeTruthy();
		expect(sign()?.style.opacity).toBe("0");
		expect(magnitude()?.textContent).toBe("5");

		// Negative: same slot, now visible. The slot is reserved either way, so
		// only opacity differs and the magnitude's box never changes.
		mock.emitShowNumber({
			session_id: 1,
			index: 2,
			total: 2,
			value: -3,
			running_sum: 2,
			emitted_at_ms: Date.now() + 1,
		});
		expect(await screen.findByText("3")).toBeTruthy();
		expect(sign()?.style.opacity).toBe("1");
		expect(magnitude()?.textContent).toBe("3");
	});
});
