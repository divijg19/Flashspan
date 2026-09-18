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
		// 3 warmup plays + first flash beep.
		expect(playMock).toHaveBeenCalledTimes(4);

		await vi.advanceTimersByTimeAsync(600);
		expect(playMock).toHaveBeenCalledTimes(5);
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

	it("primes every clip muted, then restores sound for flashes", async () => {
		await browserRuntime.setSoundEnabled(true);
		// Record each clip's muted state at play time via the receiver.
		const mutedAtPlay: boolean[] = [];
		playMock.mockImplementation(function (this: unknown) {
			mutedAtPlay.push((this as HTMLMediaElement).muted);
			return Promise.resolve();
		} as () => Promise<void>);
		await browserRuntime.startSession({
			digits_per_number: 1,
			number_duration_s: 0.5,
			total_numbers: 1,
			allow_negative_numbers: false,
		});
		// Flush warmup chains (no flash yet: first show is at 3100ms).
		await vi.advanceTimersByTimeAsync(0);

		// One unlock play per clip (beep, applause, buzzer), all muted.
		expect(playMock).toHaveBeenCalledTimes(3);
		expect(mutedAtPlay).toEqual([true, true, true]);
		// Each clip paused + reset after its unlock play.
		expect(pauseMock.mock.calls.length).toBeGreaterThanOrEqual(3);

		// The flash beep itself plays unmuted: warmup restored the state.
		await vi.advanceTimersByTimeAsync(3100);
		expect(mutedAtPlay).toEqual([true, true, true, false]);
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
