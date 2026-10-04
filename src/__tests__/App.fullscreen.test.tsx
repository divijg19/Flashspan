import { render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createMockRuntime } from "./__mocks__/mockRuntime";

/**
 * Starting a session must never depend on the Fullscreen API.
 *
 * `forceFullscreenBeforeStart` polled for 750ms and then threw when
 * `document.fullscreenElement` never appeared, and `start()` turned that into
 * `phase = "idle"`. On any target without the element requestFullscreen API
 * (iOS Safari, cross-origin iframes, some WebViews, headless browsers) the
 * Start button was therefore permanently dead. Fullscreen is an enhancement,
 * requested again at the countdown; it is not a precondition.
 *
 * Each case resets the module registry so `initializeRuntime` (once per module
 * instance) binds to that case's own mock.
 */

type MockRuntime = ReturnType<typeof createMockRuntime>;

async function startSessionWithFullscreen(
	requestFullscreen: unknown,
): Promise<number> {
	vi.resetModules();
	const { initializeRuntime } = await import("../runtime");
	const { default: App } = await import("../App");

	const mock = createMockRuntime() as MockRuntime;
	let startCalls = 0;
	const original = mock.startSession.bind(mock);
	mock.startSession = (async (...args: Parameters<typeof original>) => {
		startCalls += 1;
		return original(...args);
	}) as typeof mock.startSession;
	initializeRuntime(mock);

	(
		document.documentElement as unknown as Record<string, unknown>
	).requestFullscreen = requestFullscreen;
	Object.defineProperty(document, "fullscreenElement", {
		configurable: true,
		value: null,
		writable: true,
	});

	render(() => <App />);
	await new Promise((r) => setTimeout(r, 0));
	window.dispatchEvent(new KeyboardEvent("keydown", { key: " " }));

	const button = screen
		.getAllByRole("button")
		.find((candidate) => /start|begin/i.test(candidate.textContent ?? ""));
	expect(button, "expected a Start button").toBeTruthy();
	button?.click();
	await new Promise((r) => setTimeout(r, 20));

	return startCalls;
}

describe("starting without a usable Fullscreen API", () => {
	afterEach(() => {
		Object.defineProperty(document, "fullscreenElement", {
			configurable: true,
			value: null,
			writable: true,
		});
		(
			document.documentElement as unknown as Record<string, unknown>
		).requestFullscreen = undefined;
	});

	it("starts when requestFullscreen does not exist", async () => {
		expect(await startSessionWithFullscreen(undefined)).toBeGreaterThan(0);
	});

	it("starts when requestFullscreen rejects", async () => {
		expect(
			await startSessionWithFullscreen(() =>
				Promise.reject(new Error("refused")),
			),
		).toBeGreaterThan(0);
	});

	it("starts when requestFullscreen resolves", async () => {
		expect(
			await startSessionWithFullscreen(() => Promise.resolve()),
		).toBeGreaterThan(0);
	});
});
