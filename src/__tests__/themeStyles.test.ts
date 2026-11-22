import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { COLOR_SCHEMES } from "../runtime/types";

/**
 * The flashing number is the one stimulus the whole application depends on.
 * Its contract lives in CSS, which jsdom does not apply, so a deletion here
 * would leave every behavioural test green while the product silently lost it.
 *
 * These tests read the stylesheet directly and pin the parts that came from
 * the visual-strain research: tabular lining digits at an explicit regular
 * weight, each theme surface clearing the contrast target on its own, and
 * legible controls/chrome in both modes.
 */
const css = readFileSync(resolve(process.cwd(), "src/App.css"), "utf8");

function token(selector: string, name: string): string {
	const start = css.indexOf(selector);
	if (start < 0) throw new Error(`missing selector ${selector}`);
	const end = css.indexOf("}", start);
	const match = new RegExp(`--${name}:\\s*([^;]+);`).exec(
		css.slice(start, end),
	);
	if (!match) throw new Error(`missing --${name} in ${selector}`);
	return match[1].trim();
}

function luminance(hex: string): number {
	const channels = [1, 3, 5].map(
		(i) => Number.parseInt(hex.slice(i, i + 2), 16) / 255,
	);
	const linear = channels.map((c) =>
		c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4,
	);
	return 0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2];
}

function contrast(bg: string, fg: string): number {
	const [light, dark] = [luminance(bg), luminance(fg)].sort((a, b) => b - a);
	return (light + 0.05) / (dark + 0.05);
}

describe("flash stimulus styling", () => {
	it("lets the session display use the active theme's own colours", () => {
		// No separate neutral shell during a session: what the user picked is
		// what they see from the home screen through the last flash.
		expect(css).not.toContain("--flash-bg");
		expect(css).not.toContain("--flash-fg");
		const field = /\.app:not\(\.home\)\s*\{([^}]*)\}/.exec(css);
		expect(field).not.toBeNull();
		expect(field?.[1]).not.toContain("background:");
	});

	it("pins tabular lining digits at a regular weight", () => {
		const number = /\.number\s*\{([^}]*)\}/.exec(css);
		expect(number).not.toBeNull();
		const body = number?.[1] ?? "";
		expect(body).toContain("font-variant-numeric: tabular-nums lining-nums");
		expect(body).toMatch(/font-weight:\s*400;/);
		expect(body).toContain("color: var(--app-fg)");
	});

	it("gives native sliders a contrasting accent in every mode", () => {
		// Sliders otherwise inherit the browser's default blue, which blends
		// into blue themes. A mode-scoped neutral keeps them legible everywhere.
		const range = /\.range\s*\{([^}]*)\}/.exec(css);
		expect(range).not.toBeNull();
		expect(range?.[1]).toContain("accent-color: var(--control-accent)");
		expect(token(".app.theme-dark {", "control-accent")).toBeTruthy();
		expect(token(".app.theme-light {", "control-accent")).toBeTruthy();
	});

	it("keeps the bottom-right site link readable in every theme", () => {
		// It follows the theme foreground by default; an accent that is bright
		// enough on a dark theme can be unreadable on a light one.
		const link = /\.bottomInfo a\s*\{([^}]*)\}/.exec(css);
		expect(link).not.toBeNull();
		expect(link?.[1]).toContain("color: var(--app-fg)");

		// On light themes the whole block (label + link) is forced to black,
		// which clears WCAG small-text contrast on every light surface.
		const light =
			/body\.theme-light \.bottomInfo,\s*body\.theme-light \.bottomInfo a\s*\{([^}]*)\}/.exec(
				css,
			);
		expect(light).not.toBeNull();
		expect(light?.[1]).toContain("color: #000000");
	});

	it("never uses pure black or pure white for a theme surface", () => {
		const declared = [
			...css.matchAll(/--app-(?:bg|fg):\s*(#[0-9a-f]{6});/g),
		].map((m) => m[1].toLowerCase());
		expect(declared.length).toBeGreaterThan(0);
		expect(declared).not.toContain("#000000");
		expect(declared).not.toContain("#ffffff");
	});

	it("gives every offered scheme a dark and light surface above target", () => {
		const schemes = COLOR_SCHEMES;
		expect(schemes).toEqual([
			"ocean",
			"midnight",
			"aqua",
			"ivory",
			"rose",
			"sage",
		]);

		for (const scheme of schemes) {
			const dark = token(`.app.theme-${scheme} {`, "app-bg");
			const darkFg = token(`.app.theme-${scheme} {`, "app-fg");
			expect(contrast(dark, darkFg)).toBeGreaterThan(7);

			const light = token(`.app.theme-${scheme}.theme-light {`, "app-bg");
			const lightFg = token(`.app.theme-${scheme}.theme-light {`, "app-fg");
			expect(contrast(light, lightFg)).toBeGreaterThan(7);
		}

		// Dawn keeps Memoriad's cool blue under the near-black Midnight theme.
		expect(token(".app.theme-midnight.theme-light {", "app-bg")).toBe(
			"#9fdce6",
		);
		// Ocean is a true ultramarine, between the deep navy and the bright
		// cerulean used by Memoriad's Flash Anzan display.
		expect(token(".app.theme-ocean {", "app-bg")).toBe("#1c5094");
		// Midnight stays close to black-on-white, tuned off the 21:1 extreme.
		expect(token(".app.theme-midnight {", "app-bg")).toBe("#05070a");
	});
});
