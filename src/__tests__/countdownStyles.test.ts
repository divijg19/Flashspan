import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

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

	it("drains the ring with a single animation over the countdown", () => {
		// The ring's drain is one animation for all three counts. A per-count
		// animation would need a restart class to be re-triggered, which is the
		// mechanism that used to leave it frozen on one frame.
		expect(hasRule("countdownProgress")).toBe(true);
		expect(stylesheet).toContain("animation: ringDrain");
		expect(stylesheet).toContain("@keyframes ringDrain");
		expect(stylesheet).not.toMatch(/\.countdownProgress[AB]\b/);
	});
});
