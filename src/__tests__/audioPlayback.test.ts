import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { browserRuntime } from "../runtime/browser";

describe("browser audio playback", () => {
	let playMock: ReturnType<typeof vi.fn<() => Promise<void>>>;
	let pauseMock: ReturnType<typeof vi.fn<() => void>>;

	beforeEach(() => {
		vi.useFakeTimers();
		vi.setSystemTime(0);
		playMock = vi.fn(() => Promise.resolve());
		pauseMock = vi.fn();
		vi.spyOn(window.HTMLMediaElement.prototype, "play").mockImplementation(
			playMock,
		);
		vi.spyOn(window.HTMLMediaElement.prototype, "pause").mockImplementation(
			pauseMock,
		);
	});

	afterEach(async () => {
		await browserRuntime.stopSession();
		vi.useRealTimers();
		vi.restoreAllMocks();
	});

	it("restarts the beep clip on every flash (cut, not queue)", async () => {
		await browserRuntime.setSoundEnabled(true);
		await browserRuntime.startSession({
			digits_per_number: 1,
			number_duration_s: 0.5,
			total_numbers: 2,
			allow_negative_numbers: false,
		});

		await vi.advanceTimersByTimeAsync(3100);
		expect(playMock).toHaveBeenCalledTimes(1);

		await vi.advanceTimersByTimeAsync(600);
		expect(playMock).toHaveBeenCalledTimes(2);
	});

	it("wires a rejection handler onto every play() (rapid restarts reject)", async () => {
		await browserRuntime.setSoundEnabled(true);
		// Thenable recording whether playAudio attached a rejection handler.
		// Promise.resolve(thenable).catch(...) assimilates it (handler
		// attached); a bare clip.play() never touches it.
		let rejectionHandlerAttached = false;
		playMock.mockImplementationOnce(
			() =>
				({
					// Test double is intentionally thenable to observe assimilation.
					// biome-ignore lint/suspicious/noThenProperty: intentional thenable.
					then: (onFulfilled?: unknown, onRejected?: unknown) => {
						if (typeof onRejected === "function") {
							rejectionHandlerAttached = true;
						}
						return Promise.resolve().then(onFulfilled as never);
					},
					catch: (onRejected?: unknown) => {
						if (typeof onRejected === "function") {
							rejectionHandlerAttached = true;
						}
						return Promise.resolve();
					},
				}) as unknown as Promise<void>,
		);
		await browserRuntime.startSession({
			digits_per_number: 1,
			number_duration_s: 0.5,
			total_numbers: 1,
			allow_negative_numbers: false,
		});

		await vi.advanceTimersByTimeAsync(3100);
		expect(playMock).toHaveBeenCalled();
		expect(rejectionHandlerAttached).toBe(true);
	});

	it("silences all clips when the session stops", async () => {
		await browserRuntime.setSoundEnabled(true);
		await browserRuntime.startSession({
			digits_per_number: 1,
			number_duration_s: 0.5,
			total_numbers: 1,
			allow_negative_numbers: false,
		});
		await vi.advanceTimersByTimeAsync(3100);
		expect(playMock).toHaveBeenCalled();

		pauseMock.mockClear();
		await browserRuntime.stopSession();
		// beep + applause + buzzer clips silenced.
		expect(pauseMock.mock.calls.length).toBeGreaterThanOrEqual(3);
	});

	it("plays nothing while sound is disabled", async () => {
		await browserRuntime.setSoundEnabled(false);
		await browserRuntime.playSound("beep");
		expect(playMock).not.toHaveBeenCalled();
		await browserRuntime.setSoundEnabled(true);
	});
});
