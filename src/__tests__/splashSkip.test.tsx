import { render } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createMockRuntime } from "./__mocks__/mockRuntime";

/**
 * Skipping the splash must dismiss it outright.
 *
 * The previous handler faded the overlay out over two seconds, which produced a
 * visible flicker from four separate causes: a held key (or any second press)
 * took the other branch and cut the fade off mid-flight; the overlay stayed on
 * top of a live, interactive UI while it faded; removing the `visible` class
 * made it click-through while it was still on screen; and the unmount timer it
 * created was never tracked, so it survived cleanup.
 *
 * The invariant worth locking is that a key press leaves nothing behind: no
 * element, no pending timer, and no reaction to a second or auto-repeating
 * press. Each of those fails against the old implementation.
 */

async function renderWithSplash() {
	vi.resetModules();
	const runtimeModule = await import("../runtime");
	const appModule = await import("../App");

	runtimeModule.initializeRuntime(createMockRuntime());
	const rendered = render(() => <appModule.default />);
	// Let the mount effect schedule its fade-in.
	await new Promise((r) => setTimeout(r, 20));

	return rendered;
}

function pressKey(key = " ", repeat = false): void {
	window.dispatchEvent(new KeyboardEvent("keydown", { key, repeat }));
}

describe("skipping the splash", () => {
	afterEach(async () => {
		await vi.resetModules();
		vi.restoreAllMocks();
	});

	it("removes the splash from the DOM immediately", async () => {
		const { container } = await renderWithSplash();

		expect(container.querySelector(".splash")).toBeTruthy();
		pressKey();
		await new Promise((r) => setTimeout(r, 0));

		// Not merely transparent: gone, in the same tick as the key press.
		expect(container.querySelector(".splash")).toBeNull();
	});

	it("leaves nothing behind for a second or repeating press", async () => {
		const { container } = await renderWithSplash();

		pressKey(" ");
		await new Promise((r) => setTimeout(r, 0));
		pressKey("Enter");
		pressKey(" ", true);
		await new Promise((r) => setTimeout(r, 0));

		expect(container.querySelector(".splash")).toBeNull();
	});

	it("does not resurrect the splash after the automatic timers would have fired", async () => {
		vi.useFakeTimers();
		try {
			vi.resetModules();
			const runtimeModule = await import("../runtime");
			const appModule = await import("../App");
			runtimeModule.initializeRuntime(createMockRuntime());
			const rendered = render(() => <appModule.default />);
			await vi.advanceTimersByTimeAsync(20);

			pressKey("a");
			await vi.advanceTimersByTimeAsync(0);
			expect(rendered.container.querySelector(".splash")).toBeNull();

			// This is the part that actually pins the cleanup: the skip must leave no
			// pending timer behind. Waiting out the schedule cannot detect a leaked
			// timer, because the leaked callbacks only ever set the splash *hidden*,
			// which it already is -- so they are harmless in practice and invisible
			// to a DOM assertion.
			expect(vi.getTimerCount()).toBe(0);

			// Backstop only, and it must outlast the real schedule
			// (--splash-enter + hold + exit + slack = 7.2s).
			await vi.advanceTimersByTimeAsync(7600);
			expect(rendered.container.querySelector(".splash")).toBeNull();
		} finally {
			vi.useRealTimers();
		}
	});

	it("ignores auto-repeat while the splash is still up", async () => {
		const { container } = await renderWithSplash();

		pressKey(" ", true);
		await new Promise((r) => setTimeout(r, 0));

		// A repeat event is not a dismissal request, so the splash is still shown.
		expect(container.querySelector(".splash")).toBeTruthy();
	});
});

/**
 * The splash must not move after it appears, and its fade-out must never be cut
 * short.
 *
 * Two independent defects produced the stutter:
 *
 * 1. The banner declared no intrinsic size, so its box had zero height until the
 *    278KB PNG decoded and the flex column re-centred mid-fade. The
 *    width/height attributes below are what fix that; if they are ever dropped,
 *    the layout jump comes straight back.
 * 2. The fade lived in CSS while the schedule lived in JS as separate literals,
 *    so the unmount could land before the transition finished (cutting the last
 *    frames) or long after it (an invisible, click-through overlay). Unmount now
 *    hangs off the opacity transition ending, with a timer as the backstop.
 *
 * Honest caveat: jsdom does not run CSS transitions, so the lifecycle test
 * exercises the JS schedule and the backstop, not real fade timing.
 */
describe("splash presentation", () => {
	afterEach(async () => {
		await vi.resetModules();
		vi.restoreAllMocks();
	});

	it("reserves the banner's space and shows no placeholder text", async () => {
		const { container } = await renderWithSplash();
		const banner = container.querySelector<HTMLImageElement>(".splashBanner");
		expect(banner).toBeTruthy();

		// Intrinsic 1080x340: the browser reserves the box from these before the
		// image loads, so nothing shifts when it arrives.
		expect(banner?.getAttribute("width")).toBe("1080");
		expect(banner?.getAttribute("height")).toBe("340");

		// Decorative, and it must not paint its alt text as a placeholder.
		expect(banner?.getAttribute("alt")).toBe("");
	});

	it("keeps the splash mounted through the exit, then unmounts", async () => {
		vi.useFakeTimers();
		try {
			vi.resetModules();
			const runtimeModule = await import("../runtime");
			const appModule = await import("../App");
			runtimeModule.initializeRuntime(createMockRuntime());
			const rendered = render(() => <appModule.default />);
			await vi.advanceTimersByTimeAsync(20);

			const splash = (): HTMLElement | null =>
				rendered.container.querySelector<HTMLElement>(".splash");

			expect(splash()).toBeTruthy();

			// --splash-enter (3000ms) + --splash-hold (1000ms) has passed, so the
			// exit has started. Both live in `.splash`, and the JS schedule reads
			// them from there rather than repeating the numbers.
			await vi.advanceTimersByTimeAsync(3000 + 1000 + 100);
			expect(splash()).toBeTruthy();
			expect(splash()?.classList.contains("exiting")).toBe(true);
			expect(splash()?.classList.contains("visible")).toBe(false);

			// --splash-exit (3000ms) plus the safety timer's 200ms of slack
			// completes the unmount.
			await vi.advanceTimersByTimeAsync(3400);
			expect(splash()).toBeNull();
		} finally {
			vi.useRealTimers();
		}
	});

	/*
	 * The schedule is read out of CSS, so these three numbers live in exactly one
	 * place. This pins that the read works: the timings are declared on `.splash`,
	 * and custom properties inherit downwards, so reading them from the document
	 * root returns nothing and the JS quietly falls back to its own constants.
	 * Nothing noticed, because the two happened to agree.
	 */
	it("follows the timings declared in CSS rather than its own constants", async () => {
		const override = document.createElement("style");
		override.textContent = ".splash { --splash-hold: 5000ms; }";
		document.head.append(override);

		vi.useFakeTimers();
		try {
			vi.resetModules();
			const runtimeModule = await import("../runtime");
			const appModule = await import("../App");
			runtimeModule.initializeRuntime(createMockRuntime());
			const rendered = render(() => <appModule.default />);
			await vi.advanceTimersByTimeAsync(20);

			const splash = (): HTMLElement | null =>
				rendered.container.querySelector<HTMLElement>(".splash");

			// Long past the 1000ms hold the JS falls back to, so an exit here means
			// the stylesheet was ignored.
			await vi.advanceTimersByTimeAsync(3000 + 1000 + 100);
			expect(splash()?.classList.contains("exiting")).toBe(false);

			// But the 5000ms hold does hold.
			await vi.advanceTimersByTimeAsync(4000);
			expect(splash()?.classList.contains("exiting")).toBe(true);

			// Still mounted part-way through the 3000ms exit.
			await vi.advanceTimersByTimeAsync(1500);
			expect(splash()).toBeTruthy();

			await vi.advanceTimersByTimeAsync(1800);
			expect(splash()).toBeNull();
		} finally {
			override.remove();
			vi.useRealTimers();
		}
	});

	it("unmounts on an opacity transition end, and ignores other properties", async () => {
		const { container } = await renderWithSplash();
		const splash = container.querySelector(".splash") as HTMLElement;

		// A transition on some unrelated property must not tear the splash down.
		splash.dispatchEvent(
			new TransitionEvent("transitionend", {
				bubbles: true,
				propertyName: "transform",
			}),
		);
		await new Promise((r) => setTimeout(r, 0));
		expect(container.querySelector(".splash")).toBeTruthy();

		splash.dispatchEvent(
			new TransitionEvent("transitionend", {
				bubbles: true,
				propertyName: "opacity",
			}),
		);
		await new Promise((r) => setTimeout(r, 0));
		expect(container.querySelector(".splash")).toBeNull();
	});
});
