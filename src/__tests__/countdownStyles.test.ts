import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import {
	COUNTDOWN_FROM,
	COUNTDOWN_TICK_MS,
	COUNTDOWN_TICKS,
	INTER_NUMBER_GAP_MS,
	PRE_FLASH_SETTLE_MS,
} from "../timing";

/**
 * Every class the countdown surface uses must have a rule in the stylesheet.
 *
 * jsdom applies no CSS, so a deleted rule leaves every other test green while
 * the element silently loses its styling in the product. That is not
 * hypothetical: an edit that removed the blade animation took `.countdownRing`,
 * `.countdownTrack` and `.countdownProgress` with it, and the progress ring
 * simply stopped rendering.
 *
 * So cross-check the two files directly. The class list is read out of the
 * component rather than hard-coded, which keeps this honest when a class is
 * added or renamed.
 */

// `import.meta.url` is not a file URL under the jsdom environment, so resolve
// from the project root instead.
const component = readFileSync(resolve(process.cwd(), "src/App.tsx"), "utf8");
const stylesheet = readFileSync(resolve(process.cwd(), "src/App.css"), "utf8");
// Global rules such as `.srOnly` live here, so a class may be defined in either.
const globalStyles = readFileSync(
	resolve(process.cwd(), "src/index.css"),
	"utf8",
);
const allCss = `${stylesheet}\n${globalStyles}`;

/**
 * Classes the countdown surface actually applies, read out of the markup.
 *
 * Taken from `class`/`classList` rather than every identifier in the file, so a
 * local variable or a word in a comment is not mistaken for a class name.
 */
function appliedClasses(): string[] {
	const found = new Set<string>();

	for (const attr of component.matchAll(/class="([^"]*)"/g)) {
		for (const name of attr[1].split(/\s+/)) {
			if (name) found.add(name);
		}
	}

	for (const list of component.matchAll(/classList=\{\{([^}]*)\}\}/g)) {
		for (const key of list[1].matchAll(/([A-Za-z][A-Za-z0-9_-]*)\s*:/g)) {
			found.add(key[1]);
		}
	}

	return [...found].sort();
}

/** True when `name` appears as a class in a selector, not as a substring. */
function hasRule(name: string): boolean {
	return new RegExp(`\\.${name}(?![A-Za-z0-9_-])`).test(allCss);
}

describe("countdown stylesheet covers the countdown surface", () => {
	it("finds the countdown classes to check", () => {
		// Guards the guard: if the component is restructured and this finds
		// nothing, the test below would pass while checking nothing.
		expect(appliedClasses()).toContain("countdownProgress");
		expect(appliedClasses().length).toBeGreaterThan(3);
	});

	it("defines a rule for every countdown class the component uses", () => {
		const missing = appliedClasses().filter((name) => !hasRule(name));
		expect(missing).toEqual([]);
	});

	it("has keyframes for every animation the countdown names", () => {
		const named = new Set(
			[...stylesheet.matchAll(/animation:\s*([A-Za-z][A-Za-z0-9_-]*)/g)].map(
				(match) => match[1],
			),
		);

		const withoutKeyframes = [...named].filter(
			(name) => !stylesheet.includes(`@keyframes ${name}`),
		);
		expect(withoutKeyframes).toEqual([]);
	});

	/**
	 * The sign must reserve its slot in flow, so toggling opacity never moves
	 * the digits.
	 *
	 * It used to be parked outside the box (`position: absolute; right: 100%`),
	 * which drew every negative number about half a character left of centre.
	 * Reserving a fixed-width in-flow slot centres the group, and because the
	 * slot always exists, a positive and a negative flash put the digits at the
	 * same place.
	 */
	it("keeps the sign slot in flow so the digits never move", () => {
		const rule = (selector: string): string | null => {
			const match = new RegExp(`\\${selector}\\s*\\{([^}]*)\\}`).exec(
				stylesheet,
			);
			return match ? match[1] : null;
		};

		const sign = rule(".signOverlay");
		expect(sign, "expected a .signOverlay rule").toBeTruthy();
		expect(sign).toMatch(/flex:\s*0\s+0\s+auto/);
		expect(sign).toMatch(/width:\s*1ch/);
		// Outside-the-box positioning is what displaced negatives.
		expect(sign).not.toMatch(/position:\s*absolute/);
		expect(sign).not.toMatch(/right:\s*100%/);

		const group = rule(".signedNumber");
		expect(group, "expected a .signedNumber rule").toBeTruthy();
		expect(group).toMatch(/display:\s*inline-flex/);
	});

	it("drains the ring with a single animation over the countdown", () => {
		// The ring's drain is one animation for all three counts. A per-count
		// animation would need a restart class to be re-triggered, which is the
		// mechanism that used to leave it frozen on one frame.
		expect(hasRule("countdownProgress")).toBe(true);
		expect(stylesheet).toContain("animation: ringDrain");
		expect(stylesheet).toContain("@keyframes ringDrain");
		expect(stylesheet).not.toMatch(/\.countdownProgress[AB]\b/);
	});

	/**
	 * The ring's drain duration is declared in CSS as a custom property, so it
	 * is invisible to every other test and to the TS module that mirrors the
	 * timing budget. Nothing drove it in JS, which means a mismatch here shows up
	 * only as a ring that empties a beat before or after the last count — the
	 * clock reads 1 while the ring is already gone.
	 *
	 * The stylesheet comment claims this value is "pinned at 3 x 1000ms by the
	 * cross-runtime timeline tests", but no TS test pinned it: the Rust suite
	 * pins `COUNTDOWN_FROM`/`COUNTDOWN_TICK_MS`, and this asserts the CSS value
	 * is derived from the same numbers rather than restating them by hand.
	 */
	it("derives the ring drain from the countdown constants", () => {
		// The TS mirror of `core/timing.rs`. Rust pins these in
		// `timing_budget_constants_pin_the_whole_budget`; pinning them here too
		// means a one-sided edit fails on the side that moved rather than silently
		// splitting the budget across runtimes.
		expect(COUNTDOWN_FROM).toBe(3);
		expect(COUNTDOWN_TICK_MS).toBe(1000);
		expect(PRE_FLASH_SETTLE_MS).toBe(100);
		expect(INTER_NUMBER_GAP_MS).toBe(100);

		// The displayed sequence must be exactly one value per count, descending.
		expect(COUNTDOWN_TICKS).toHaveLength(COUNTDOWN_FROM);
		expect([...COUNTDOWN_TICKS]).toEqual(
			Array.from({ length: COUNTDOWN_FROM }, (_, i) => COUNTDOWN_FROM - i),
		);

		// The stylesheet must express the total, not a literal that can drift.
		const expectedTotalMs = COUNTDOWN_FROM * COUNTDOWN_TICK_MS;
		const declared = /--countdown-total:\s*(\d+)ms/.exec(stylesheet);
		expect(
			declared,
			"expected a --countdown-total declaration in ms",
		).toBeTruthy();
		expect(Number(declared?.[1])).toBe(expectedTotalMs);

		// And the ring must actually consume the variable, so the pinned number
		// cannot become decorative while the animation hardcodes its own length.
		expect(stylesheet).toMatch(
			/animation:\s*ringDrain\s+var\(--countdown-total\)/,
		);
		expect(stylesheet).not.toMatch(/animation:\s*ringDrain\s+\d/);
	});
});
