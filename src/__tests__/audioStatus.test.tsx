import { render, screen } from "@solidjs/testing-library";
import { describe, expect, it, vi } from "vitest";
import App from "../App";
import { initializeRuntime } from "../runtime";
import { browserRuntime } from "../runtime/browser";
import { createMockRuntime } from "./__mocks__/mockRuntime";

describe("browser getAudioStatus", () => {
	it("reports ready when wav playback is supported", async () => {
		await browserRuntime.setSoundEnabled(true);
		const canPlay = vi
			.spyOn(window.HTMLMediaElement.prototype, "canPlayType")
			.mockReturnValue("probably");
		try {
			const status = await browserRuntime.getAudioStatus();
			expect(status).toEqual({
				enabled: true,
				available: true,
				detail: "ready",
			});
		} finally {
			canPlay.mockRestore();
		}
	});

	it("reports unsupported playback instead of failing", async () => {
		await browserRuntime.setSoundEnabled(true);
		const canPlay = vi
			.spyOn(window.HTMLMediaElement.prototype, "canPlayType")
			.mockReturnValue("");
		try {
			const status = await browserRuntime.getAudioStatus();
			expect(status.available).toBe(false);
			expect(status.detail).toBe("wav-playback-unsupported");
		} finally {
			canPlay.mockRestore();
		}
	});

	it("reflects the disabled flag", async () => {
		await browserRuntime.setSoundEnabled(false);
		try {
			const status = await browserRuntime.getAudioStatus();
			expect(status.enabled).toBe(false);
		} finally {
			await browserRuntime.setSoundEnabled(true);
		}
	});
});

describe("sound status line", () => {
	// One shared runtime per file (initializeRuntime ignores re-init), so a
	// single mock with mutable status drives all three renders below.
	it("reflects ready, unavailable, and off states", async () => {
		const mock = createMockRuntime();
		initializeRuntime(mock);

		const first = render(() => <App />);
		await new Promise((r) => setTimeout(r, 0));
		expect(await screen.findByText("Sound ready")).toBeTruthy();
		first.unmount();

		mock.setAudioStatus({
			enabled: true,
			available: false,
			detail: "no-device",
		});
		const second = render(() => <App />);
		await new Promise((r) => setTimeout(r, 0));
		expect(
			await screen.findByText("Sound unavailable (no-device)"),
		).toBeTruthy();
		second.unmount();

		mock.setAudioStatus({
			enabled: false,
			available: true,
			detail: "ready",
		});
		const third = render(() => <App />);
		await new Promise((r) => setTimeout(r, 0));
		expect(third.container.querySelector(".soundStatus")).toBeNull();
		third.unmount();
	});
});
