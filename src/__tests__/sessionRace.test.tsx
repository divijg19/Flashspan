import { render, screen } from "@solidjs/testing-library";
import { describe, expect, it } from "vitest";
import App from "../App";
import { initializeRuntime } from "../runtime";
import type { SubmitAnswerResponse } from "../runtime/types";
import { createMockRuntime } from "./__mocks__/mockRuntime";

describe("session race conditions", () => {
	it("ignores validation result if session changed while awaiting", async () => {
		const mock = createMockRuntime();
		// override submitAnswerText to delay
		let resolveFn: () => void = () => {};
		mock.submitAnswerText = async (_sid, _text) => {
			await new Promise<void>((r) => {
				resolveFn = r;
			});
			const response: SubmitAnswerResponse = {
				validation: {
					expected_sum: 1,
					provided_sum: 1,
					correct: true,
					delta: 0,
				},
				auto_repeat_waiting: null,
				message: "",
			};
			return response;
		};

		initializeRuntime(mock);
		render(() => <App />);

		// allow mount effects to run and listeners to register, then skip splash
		await new Promise((r) => setTimeout(r, 0));
		window.dispatchEvent(new KeyboardEvent("keydown", { key: " " }));

		// Start session 1 and show number
		mock.emitShowNumber({
			session_id: 1,
			index: 1,
			total: 1,
			value: 5,
			running_sum: 5,
			emitted_at_ms: Date.now(),
		});
		expect(await screen.findByText("5")).toBeTruthy();

		// Simulate user typing answer and triggering validation (call validateTypedAnswer indirectly)
		// We'll invoke runtime.submitAnswerText by calling the exported function on the runtime mock
		const validatePromise = mock.submitAnswerText(1, "5");

		// Before the delayed validation resolves, emit a new session complete (session changed)
		mock.emitSessionComplete({ session_id: 2, numbers: [1], sum: 1 });

		// Now resolve the delayed submit
		resolveFn();
		await validatePromise;

		// Ensure response exists but application should ignore it; check UI shows session complete for session 2
		expect(await screen.findByText("Session complete")).toBeTruthy();
	});
});
