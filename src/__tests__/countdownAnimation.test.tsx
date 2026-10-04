import { render } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createMockRuntime } from "./__mocks__/mockRuntime";

/**
 * The countdown's film leader must turn once per count.
 *
 * The blades hold one angle per digit and rotate when the digit changes, rather
 * than sweeping through intermediate angles for the whole second. The angle lives
 * in a class (`bladeQuarter`/`bladeHalf`/`bladeThreeQuarter`) and the movement is
 * a transition on the transform, so there is no animation to restart and nothing
 * that can silently fail to re-trigger on a tick.
 *
 * So assert the mechanism directly: each count rests at its own angle, and
 * exactly one of the three is applied at a time. The ring, likewise, is left to
 * CSS rather than given a restart of its own.
 */
async function renderCountingDown() {
	vi.resetModules();
	const runtimeModule = await import("../runtime");
	const appModule = await import("../App");

	const mock = createMockRuntime();
	runtimeModule.initializeRuntime(mock);

	const rendered = render(() => <appModule.default />);
	await new Promise((r) => setTimeout(r, 0));
	// Dismiss the splash so the countdown surface is the active phase.
	window.dispatchEvent(new KeyboardEvent("keydown", { key: " " }));

	return { container: rendered.container, mock };
}

function shutterClass(container: HTMLElement): string {
	const shutter = container.querySelector(".countdownShutter");
	expect(shutter, "expected the countdown shutter to render").toBeTruthy();
	return shutter?.className ?? "";
}

describe("countdown animation keeps running", () => {
	afterEach(async () => {
		await vi.resetModules();
		vi.restoreAllMocks();
	});

	it("renders the shutter, its static glow and the progress ring", async () => {
		const { container, mock } = await renderCountingDown();
		mock.emitCountdown("3");
		await new Promise((r) => setTimeout(r, 0));

		expect(container.querySelector(".countdownGlow")).toBeTruthy();
		expect(container.querySelector(".countdownShutter")).toBeTruthy();
		expect(container.querySelector(".countdownProgress")).toBeTruthy();
	});

	it("rests at a different blade angle for every count", async () => {
		const { container, mock } = await renderCountingDown();

		const seen: string[] = [];
		for (const tick of ["3", "2", "1"]) {
			mock.emitCountdown(tick);
			await new Promise((r) => setTimeout(r, 0));
			seen.push(shutterClass(container));
		}

		expect(seen).toHaveLength(3);

		const angles = ["bladeQuarter", "bladeHalf", "bladeThreeQuarter"];
		for (const className of seen) {
			// Exactly one resting angle at a time: two would fight over the
			// transform, and none would leave the blades unrotated.
			const applied = angles.filter((angle) => className.includes(angle));
			expect(applied).toHaveLength(1);
		}

		// Each count moves to its own angle, in order, so the blades turn once per
		// digit change rather than sitting at a single angle all countdown.
		expect(angles.filter((angle) => seen[0].includes(angle))).toEqual([
			"bladeQuarter",
		]);
		expect(angles.filter((angle) => seen[1].includes(angle))).toEqual([
			"bladeHalf",
		]);
		expect(angles.filter((angle) => seen[2].includes(angle))).toEqual([
			"bladeThreeQuarter",
		]);
	});

	/*
	 * The ring's drain is one CSS animation over the whole countdown, so there is
	 * nothing per-tick left to observe in the DOM. What these guard is the
	 * mechanism: no inline offset and no parity classes, either of which is how a
	 * per-count animation gets restarted, and a restart that silently fails to
	 * fire leaves the ring stuck on one frame.
	 */
	it("leaves the ring drain entirely to CSS", async () => {
		const { container, mock } = await renderCountingDown();
		mock.emitCountdown("3");
		await new Promise((r) => setTimeout(r, 0));

		const ring = container.querySelector(".countdownProgress");
		expect(ring, "expected the countdown ring to render").toBeTruthy();
		expect(ring?.getAttribute("class")).toBe("countdownProgress");

		// No inline offset: JS must not drive the ring, or it can drift from the
		// digits by a frame or skip a beat when a tick arrives late.
		expect(ring?.getAttribute("style")).toBeNull();

		for (const tick of ["3", "2", "1"]) {
			mock.emitCountdown(tick);
			await new Promise((r) => setTimeout(r, 0));
		}

		const after = container.querySelector(".countdownProgress");
		expect(after?.getAttribute("style")).toBeNull();
		expect(after?.getAttribute("class")).toBe("countdownProgress");
	});

	it("carries no animation-restart classes of its own", async () => {
		const { container, mock } = await renderCountingDown();
		mock.emitCountdown("3");
		await new Promise((r) => setTimeout(r, 0));

		const ring = container.querySelector(".countdownProgress");
		// The drain is a CSS transition on the offset; parity classes would be a
		// leftover restart mechanism that can silently fail to fire.
		expect(ring?.getAttribute("class")).toBe("countdownProgress");
	});
});
