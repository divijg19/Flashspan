import { existsSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { normalizeSessionConfig } from "../runtime/browser";
import type { SessionConfigInput } from "../runtime/types";

/**
 * JS <-> WASM parity for session normalization.
 *
 * The browser ships two normalizers: the Rust one behind the WASM bridge and
 * the local JS one used when the bridge is unavailable. They were kept in sync
 * by comment alone, and drift shipped: the auto-repeat decrement fought the
 * `repeats >= 1` clamp (infinite repeats) and the duplicate-value fallback left
 * the digit-width domain. This asserts they agree field by field.
 *
 * Skipped when `src/wasm/pkg` is absent, which is the state of a fresh clone
 * until `bun run build:wasm` has run. The CI wasm job builds it before testing.
 */

// Resolved from the repo root (the vitest cwd): `import.meta.url` is not a
// file:// URL under the dev server, so `fileURLToPath` cannot be used here.
const PKG_DIR = join(process.cwd(), "src", "wasm", "pkg");
const PKG_JS = join(PKG_DIR, "flashspan_core.js");
const PKG_WASM = join(PKG_DIR, "flashspan_core_bg.wasm");
const HAS_WASM = existsSync(PKG_JS) && existsSync(PKG_WASM);

type WasmNormalized = { effective: Record<string, unknown> };
type WasmModule = {
	default: (init: { module_or_path: Uint8Array }) => Promise<unknown>;
	normalize_session_config_wasm: (input: unknown) => WasmNormalized;
	build_session_plan_wasm: (
		sessionId: bigint,
		input: unknown,
		seed: bigint | null,
	) => Record<string, unknown>;
};

let cached: WasmModule | null = null;

async function loadWasm(): Promise<WasmModule> {
	if (cached) {
		return cached;
	}
	const wasm = (await import(
		/* @vite-ignore */ PKG_JS
	)) as unknown as WasmModule;
	await wasm.default({ module_or_path: await readFile(PKG_WASM) });
	cached = wasm;
	return wasm;
}

/**
 * Every branch a normalizer can take: both clamps, the digit cap, the
 * exact-integer total bound, the duration grid, and non-finite floats.
 */
const ADVERSARIAL_INPUTS: SessionConfigInput[] = [
	{
		digits_per_number: 0,
		number_duration_s: 0.01,
		total_numbers: 0,
		allow_negative_numbers: false,
	},
	{
		digits_per_number: 1,
		number_duration_s: 0.1,
		total_numbers: 1,
		allow_negative_numbers: false,
	},
	{
		digits_per_number: 2,
		number_duration_s: 0.5,
		total_numbers: 10,
		allow_negative_numbers: true,
	},
	{
		digits_per_number: 15,
		number_duration_s: 60,
		total_numbers: 9,
		allow_negative_numbers: true,
	},
	{
		digits_per_number: 16,
		number_duration_s: 60.1,
		total_numbers: 10_001,
		allow_negative_numbers: false,
	},
	{
		digits_per_number: 99,
		number_duration_s: 1000,
		total_numbers: 100_000,
		allow_negative_numbers: false,
	},
	{
		digits_per_number: -5,
		number_duration_s: -1,
		total_numbers: -5,
		allow_negative_numbers: true,
	},
	// Off-grid durations must snap to the reported 0.1s grid identically.
	{
		digits_per_number: 3,
		number_duration_s: 0.15,
		total_numbers: 4,
		allow_negative_numbers: false,
	},
	{
		digits_per_number: 3,
		number_duration_s: 0.25,
		total_numbers: 4,
		allow_negative_numbers: false,
	},
	{
		digits_per_number: 3,
		number_duration_s: 2.87,
		total_numbers: 4,
		allow_negative_numbers: false,
	},
	{
		digits_per_number: 3,
		number_duration_s: 0.049,
		total_numbers: 4,
		allow_negative_numbers: false,
	},
	// Non-finite durations clamp to the same end of the range on both sides.
	{
		digits_per_number: 3,
		number_duration_s: Number.NaN,
		total_numbers: 4,
		allow_negative_numbers: false,
	},
	{
		digits_per_number: 3,
		number_duration_s: Number.POSITIVE_INFINITY,
		total_numbers: 4,
		allow_negative_numbers: false,
	},
	{
		digits_per_number: 3,
		number_duration_s: Number.NEGATIVE_INFINITY,
		total_numbers: 4,
		allow_negative_numbers: false,
	},
];

describe.skipIf(!HAS_WASM)("JS and WASM normalization parity", () => {
	it.each(ADVERSARIAL_INPUTS)(
		"agrees on $digits_per_number digits, $number_duration_s s, $total_numbers total",
		async (input) => {
			const wasm = await loadWasm();
			const rust = wasm.normalize_session_config_wasm(input).effective;
			const js = normalizeSessionConfig(input) as unknown as Record<
				string,
				unknown
			>;

			expect(js.digits_per_number).toBe(rust.digits_per_number);
			expect(js.total_numbers).toBe(rust.total_numbers);
			expect(js.allow_negative_numbers).toBe(rust.allow_negative_numbers);
			// Both sides must also agree on the exact float they report.
			expect(js.number_duration_s).toBe(rust.number_duration_s);
		},
	);
});

describe.skipIf(!HAS_WASM)("WASM bridge ABI", () => {
	it("maps u64 arguments as BigInt, not number", async () => {
		// `session_id` and `seed` are u64, which wasm-bindgen exposes as BigInt.
		// The loader was passing plain numbers, so every plan call threw a
		// TypeError that the caller swallowed as "WASM unavailable" and the
		// browser silently ran the JS planner for every session.
		const wasm = await loadWasm();
		const input: SessionConfigInput = {
			digits_per_number: 2,
			number_duration_s: 0.5,
			total_numbers: 3,
			allow_negative_numbers: false,
		};

		expect(() =>
			wasm.build_session_plan_wasm(1 as unknown as bigint, input, 42n),
		).toThrow();

		const plan = wasm.build_session_plan_wasm(1n, input, 42n);
		expect(plan.numbers_generated).toHaveLength(3);
		expect(plan.steps).toBeInstanceOf(Array);
	});

	it("accepts a null seed for an unseeded plan", async () => {
		const wasm = await loadWasm();
		const plan = wasm.build_session_plan_wasm(
			7n,
			{
				digits_per_number: 1,
				number_duration_s: 0.5,
				total_numbers: 2,
				allow_negative_numbers: false,
			},
			null,
		);

		expect(plan.numbers_generated).toHaveLength(2);
	});

	it("produces plans that satisfy the invariants the JS planner also holds", async () => {
		const wasm = await loadWasm();
		const plan = wasm.build_session_plan_wasm(
			3n,
			{
				digits_per_number: 3,
				number_duration_s: 0.5,
				total_numbers: 20,
				allow_negative_numbers: true,
			},
			4242n,
		) as unknown as {
			numbers_generated: number[];
			expected_sum: number;
			total_duration_ms: number;
			steps: Record<string, unknown>[];
		};

		const numbers = plan.numbers_generated;
		expect(numbers).toHaveLength(20);
		expect(plan.expected_sum).toBe(
			numbers.reduce((total, value) => total + value, 0),
		);
		expect(numbers[0]).toBeGreaterThanOrEqual(0);
		for (let index = 1; index < numbers.length; index += 1) {
			expect(numbers[index]).not.toBe(numbers[index - 1]);
		}
		for (const value of numbers) {
			expect(Math.abs(value)).toBeGreaterThanOrEqual(100);
			expect(Math.abs(value)).toBeLessThanOrEqual(999);
		}
		// 1 initial clear + 3 countdown ticks + 2 per number + final clear +
		// complete.
		expect(plan.steps).toHaveLength(1 + 3 + 2 * 20 + 1 + 1);
	});
});
