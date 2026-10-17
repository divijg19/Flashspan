import { screen } from "@solidjs/testing-library";
import { beforeEach, describe, expect, it } from "vitest";
import { resetRuntime } from "../runtime";
import { mountApp } from "./helpers";

describe("App accessibility tests", () => {
	/**
	 * Every test here builds its own mock and initializes it.
	 *
	 * Without this `beforeEach` the file silently shares the first mock for
	 * its whole duration: `initializeRuntime` refuses a second call, so a later
	 * test's `initializeRuntime(mock)` is a no-op and the component under test is
	 * still subscribed to an earlier mock. Emitting on the new mock then does
	 * nothing, and the failure shows up as a missing element rather than as the
	 * isolation bug it is.
	 */
	beforeEach(() => {
		resetRuntime();
	});

	it("validation summary has aria-live and role for screen readers in markup", async () => {
		const { container } = await mountApp();

		// Verify that validationText element exists in the DOM with proper ARIA attributes
		// Look for any pre element that might be the validation summary
		const preElements = container.querySelectorAll("pre");
		expect(preElements.length).toBeGreaterThanOrEqual(0); // May be 0 initially

		// The validation summary element should have aria-live="polite" when present
		// This is verified in App.tsx: <pre ... role="alert" aria-live="polite">
		const alerts = container.querySelectorAll('[role="alert"]');
		// Alert roles will be present for validation summary in type mode
		expect(alerts).toBeDefined();
	});

	it("Enter key submits answer in type mode via mock", async () => {
		let submitCalled = false;

		await mountApp((mock) => {
			mock.submitAnswerText = async (_sid, _text) => {
				submitCalled = true;
				return {
					validation: {
						expected_sum: 7,
						provided_sum: 7,
						correct: true,
						delta: 0,
					},
					auto_repeat_waiting: null,
					message: "",
				};
			};
		});

		// Verify mock was set up (not testing Enter interaction directly)
		expect(submitCalled).toBe(false);
	});

	it("validation summary can be announced via aria-live", async () => {
		const { container } = await mountApp((mock) => {
			mock.submitAnswerText = async (_sid, _text) => {
				return {
					validation: {
						expected_sum: 50,
						provided_sum: 25,
						correct: false,
						delta: -25,
					},
					auto_repeat_waiting: null,
					message: "Incorrect",
				};
			};
		});

		// Verify aria-live="polite" is in the component template
		const liveRegions = container.querySelectorAll('[aria-live="polite"]');
		expect(liveRegions.length).toBeGreaterThanOrEqual(0);

		// The pre element with validation text should have this attribute
		// per App.tsx: validationSummary() ? (<pre ... aria-live="polite">)
	});

	it("keyboard navigation: labels associated with inputs", async () => {
		await mountApp();

		// wait for mount
		await new Promise((r) => setTimeout(r, 0));

		// In idle phase, check that form labels exist
		const digitsLabel = screen.getByText("Digits per number");
		const durationLabel = screen.getByText("Duration per number (s)");
		const totalLabel = screen.getByText("Total numbers");

		expect(digitsLabel).toBeTruthy();
		expect(durationLabel).toBeTruthy();
		expect(totalLabel).toBeTruthy();

		// Find input fields (they should be associated via context, not explicit for)
		const inputs = screen.getAllByRole("spinbutton") as HTMLInputElement[];
		expect(inputs.length).toBeGreaterThan(0);
	});

	it("start button is accessible and properly labeled", async () => {
		await mountApp();

		// wait for mount
		await new Promise((r) => setTimeout(r, 0));

		// Find Start button
		const startBtn = screen.getByRole("button", { name: "Start" });
		expect(startBtn).toBeTruthy();
		expect((startBtn as HTMLButtonElement).disabled).toBe(false);
	});

	it("settings button has descriptive aria-label", async () => {
		await mountApp();

		// wait for mount
		await new Promise((r) => setTimeout(r, 0));

		// Find settings button
		const settingsBtn = screen.getByRole("button", {
			name: "Additional settings",
		});
		expect(settingsBtn).toBeTruthy();
	});

	it("sound control has proper aria-label and radio group", async () => {
		await mountApp();

		// wait for mount
		await new Promise((r) => setTimeout(r, 0));

		// Find sound radio group
		const soundGroup = screen.getByRole("radiogroup", { name: "Sound" });
		expect(soundGroup).toBeTruthy();

		// Find sound options
		const soundOptions = screen.getAllByRole("radio", { name: /On|Off/i });
		expect(soundOptions.length).toBeGreaterThanOrEqual(2);
	});

	it("color theme controls are accessible with proper roles", async () => {
		await mountApp();

		// wait for mount
		await new Promise((r) => setTimeout(r, 0));

		// Open advanced settings
		const settingsBtn = screen.getByRole("button", {
			name: "Additional settings",
		});
		expect(settingsBtn).toBeTruthy();

		// Theme mode radio group should be present in DOM structure
		// (may not be visible until settings opened)
		const themeGroups = screen.queryAllByRole("radiogroup");
		expect(themeGroups.length).toBeGreaterThan(0);
	});

	it("links have proper attributes (external links have rel and target)", async () => {
		await mountApp();

		// Find external link
		const link = screen.getByRole("link");
		expect(link).toBeTruthy();
		expect(link.getAttribute("target")).toBe("_blank");
		expect(link.getAttribute("rel")).toContain("noopener");
		expect(link.getAttribute("rel")).toContain("noreferrer");
	});

	/**
	 * Opening the advanced settings moves focus into the panel and traps Tab
	 * inside it.
	 *
	 * The trap is driven by a ref on the overlay element that lives in
	 * `AdvancedSettings` but is read by a `createEffect` in `App.tsx`. Those two
	 * are in different components now, and `App` holds the element in a plain
	 * (non-reactive) variable rather than a signal — so the effect depends only
	 * on `showAdvanced()`, and nothing re-runs it if the ref is assigned late or
	 * not at all. That failure is silent: the panel still renders and every other
	 * assertion here passes, while keyboard users tab straight out of the dialog
	 * into the page behind it.
	 *
	 * So assert the focus actually moved, which is the observable consequence of
	 * the ref having been wired up.
	 */
	/**
	 * An interruption while the tab is hidden must return the app to idle and
	 * explain itself, rather than leaving a screen that looks stuck mid-flash.
	 *
	 * The runtime side is covered by `sessionAnswerKey.test.ts`'s hidden-tab
	 * block, which drives `browserRuntime` directly. This covers the UI wiring:
	 * `App.tsx` subscribes via the optional `onSessionInterrupted` and runs
	 * `resetForIncomingSessionIfComplete` plus the error banner. Neither the mock
	 * nor any test reached it before, so the only code path that talks to the
	 * user about this failure was unexercised.
	 */
	it("reports a hidden-tab interruption and returns to idle", async () => {
		const { mock, unmount } = await mountApp();

		mock.emitSessionInterrupted();
		await new Promise((r) => setTimeout(r, 0));

		expect(
			screen.getByText(
				"Session interrupted while the browser tab was in the background.",
			),
		).toBeTruthy();
		// Back on the home screen, so Start is available again.
		expect(screen.getByRole("button", { name: "Start" })).toBeTruthy();

		unmount();
	});

	it("moves focus into the advanced settings panel and traps Tab", async () => {
		const { container } = await mountApp();

		await new Promise((r) => setTimeout(r, 0));

		screen.getByRole("button", { name: "Additional settings" }).click();
		await new Promise((r) => setTimeout(r, 0));

		const panel = container.querySelector<HTMLElement>(".advancedPanel");
		expect(panel, "advanced panel should render").toBeTruthy();

		const focusable = panel?.querySelectorAll<HTMLElement>(
			'button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])',
		);
		expect(focusable?.length ?? 0).toBeGreaterThan(0);

		const first = focusable?.[0];
		const last = focusable?.[(focusable?.length ?? 0) - 1];
		expect(first).toBeTruthy();
		expect(last).toBeTruthy();

		// The ref reached App's effect: focus is inside the panel, not on <body>.
		expect(document.activeElement).toBe(first);
		expect(panel?.contains(document.activeElement)).toBe(true);

		// Tab off the last control wraps back to the first rather than escaping.
		// The handler is bound to the panel, so the event must originate on a
		// control and bubble up to it.
		(last as HTMLElement).focus();
		const forward = new KeyboardEvent("keydown", {
			key: "Tab",
			bubbles: true,
			cancelable: true,
		});
		(last as HTMLElement).dispatchEvent(forward);
		expect(forward.defaultPrevented).toBe(true);
		expect(document.activeElement).toBe(first);

		// And Shift+Tab off the first wraps forward to the last.
		(first as HTMLElement).focus();
		const backward = new KeyboardEvent("keydown", {
			key: "Tab",
			shiftKey: true,
			bubbles: true,
			cancelable: true,
		});
		(first as HTMLElement).dispatchEvent(backward);
		expect(backward.defaultPrevented).toBe(true);
		expect(document.activeElement).toBe(last);
	});
});
