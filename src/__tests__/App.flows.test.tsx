import { render, screen } from "@solidjs/testing-library";
import { describe, expect, it } from "vitest";
import App from "../App";
import { initializeRuntime } from "../runtime";
import { createMockRuntime } from "./__mocks__/mockRuntime";

describe("App flow tests", () => {
	it("shows countdown, flashes number, and displays completion", async () => {
		const mock = createMockRuntime();
		initializeRuntime(mock);

		render(() => <App />);

		// allow mount effects to run and listeners to register
		await new Promise((r) => setTimeout(r, 0));

		// Skip splash screen (any key press) then emit countdown tick
		window.dispatchEvent(new KeyboardEvent("keydown", { key: " " }));
		mock.emitCountdown("3");
		expect(await screen.findByText("3")).toBeTruthy();

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

		// Emit session complete
		mock.emitSessionComplete({ session_id: 1, numbers: [7], sum: 7 });
		expect(await screen.findByText("Session complete")).toBeTruthy();
	});
});
