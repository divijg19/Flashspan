import { render, screen } from "@solidjs/testing-library";
import { describe, expect, it, vi } from "vitest";
import App from "../App";
import { initializeRuntime } from "../runtime";
import { browserRuntime } from "../runtime/browser";
import { createMockRuntime } from "./__mocks__/mockRuntime";

describe("browser getAudioStatus", () => {
	it("reports not-started before any playback attempt", async () => {
		await browserRuntime.setSoundEnabled(true);
		const canPlay = vi
			.spyOn(window.HTMLMediaElement.prototype, "canPlayType")
			.mockReturnValue("probably");
		try {
			// Capable but never played: honest web equivalent of native's
			// "not-started", not a claim of availability that a dead device would
			// also satisfy.
			const status = await browserRuntime.getAudioStatus();
			expect(status.available).toBe(true);
			expect(status.detail).toBe("not-started");
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
	// One shared runtime per file (`initializeRuntime` ignores re-init), so the
	// mock is built once and each test below states the status it starts from
	// rather than depending on what the previous one left behind.
	const mock = createMockRuntime();
	initializeRuntime(mock);

	const beforeSession = {
		enabled: true,
		available: true,
		detail: "ready",
	} as const;

	it("reflects ready, unavailable, and off states", async () => {
		mock.setAudioStatus(beforeSession);
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
		// Layout stability: the slot is always reserved, empty when off.
		const slot = third.container.querySelector(".soundStatus");
		expect(slot).not.toBeNull();
		expect(slot?.textContent).toBe("");
		third.unmount();
	});

	it("stays empty until playback has actually proven something", async () => {
		// "Sound ready" used to be shown for this exact state, which is to say
		// on every machine, for as long as no session had run — including one
		// with no output device. The slot stays reserved and stays empty until
		// playback has happened.
		mock.setAudioStatus({
			enabled: true,
			available: true,
			detail: "not-started",
		});
		const view = render(() => <App />);
		await new Promise((r) => setTimeout(r, 0));

		const slot = view.container.querySelector(".soundStatus");
		expect(slot).not.toBeNull();
		expect(slot?.textContent).toBe("");
		view.unmount();
	});

	/**
	 * Audio is only played during a session, so a session is the only window in
	 * which the runtime can learn the device is broken — and the status line
	 * lives on the home screen, which is where the app comes back to. Without
	 * the re-read on the way home the indicator kept the "Sound ready" it showed
	 * before the session started, whatever the session actually sounded like.
	 */
	it("re-reads the status when a session returns the app home", async () => {
		mock.setAudioStatus(beforeSession);
		const view = render(() => <App />);
		await new Promise((r) => setTimeout(r, 0));
		expect(await screen.findByText("Sound ready")).toBeTruthy();

		// Into a live session...
		mock.emitCountdown("2");
		mock.emitShowNumber({
			session_id: 1,
			value: 7,
			index: 0,
			total: 1,
			running_sum: 7,
			emitted_at_ms: 0,
		});
		await new Promise((r) => setTimeout(r, 0));

		// ...during which the runtime discovers there is no output device.
		mock.setAudioStatus({
			enabled: true,
			available: false,
			detail: "no-output-device",
		});

		// Escape stops the running session and returns to the home screen.
		window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
		await new Promise((r) => setTimeout(r, 0));

		expect(
			await screen.findByText("Sound unavailable (no-output-device)"),
		).toBeTruthy();
		view.unmount();
	});
});
