import type { WasmSessionPlan } from "../../wasm/coreBridge";
import { registerWasmCoreBridge } from "../../wasm/coreBridge";

/**
 * Shared fixtures for the tests that drive the browser runtime through a
 * stubbed WASM bridge.
 *
 * Three test files each carried their own copy of a plan builder, a bridge
 * stub and a config snapshot. The builders were near-identical — the same
 * 3/2/1 countdown, the same 1-based index conversion, the same 100ms gap — and
 * the snapshots differed only in `digits_per_number` and `total_numbers`. So a
 * change to the plan shape had to be made in three places, and the copies had
 * already drifted: one took an `exposureMs` parameter while the others
 * hard-coded it, and one computed `total_duration_ms` from a running `at` while
 * another used a literal.
 *
 * These live beside the mock runtime because they are the same kind of thing:
 * test doubles for the runtime layer.
 */

/** The countdown every plan starts with, as the engine emits it. */
const COUNTDOWN_STEPS: WasmSessionPlan["steps"] = [
	{ ClearScreen: { session_id: 1, index: null, delay_ms_before_next: 0 } },
	{ CountdownTick: { value: "3", delay_ms_before_next: 1000 } },
	{ CountdownTick: { value: "2", delay_ms_before_next: 1000 } },
	// The final tick also carries the pre-flash settle, matching
	// `core/timing.rs`: the first number paints late and never short.
	{ CountdownTick: { value: "1", delay_ms_before_next: 1100 } },
];

/** The first flash follows the countdown, settle included. */
const FIRST_FLASH_AT_MS = 3100;

/** Blank gap between a clear and the next flash. `INTER_NUMBER_GAP_MS`. */
const GAP_MS = 100;

/**
 * A config snapshot for a plan.
 *
 * `total_numbers` must agree with the plan actually produced, or the
 * plan/config consistency guard in the runtime rejects the session.
 */
function configSnapshot(overrides?: {
	digits_per_number?: number;
	total_numbers?: number;
}): WasmSessionPlan["config_snapshot"] {
	return {
		digits_per_number: overrides?.digits_per_number ?? 2,
		number_duration_s: 0.5,
		total_numbers: overrides?.total_numbers ?? 2,
		allow_negative_numbers: false,
	};
}

/**
 * Build a `WasmSessionPlan` whose numbers are exactly `numbers`.
 *
 * The steps mirror `build_session_plan` in `core/engine.rs`: an initial clear,
 * the three countdown ticks, then a show/clear pair per number, then a trailing
 * clear and the completion. Indices are 1-based, as Rust reports them.
 *
 * `exposureMs` is how long each number stays on screen; it defaults to 500ms,
 * which is what the tests that do not care about timing use.
 */
export function fakeWasmPlan(
	numbers: number[],
	options?: { exposureMs?: number; digits?: number; total?: number },
): WasmSessionPlan {
	const exposureMs = options?.exposureMs ?? 500;

	const steps: WasmSessionPlan["steps"] = [...COUNTDOWN_STEPS];
	let at = FIRST_FLASH_AT_MS;
	let running = 0;

	for (let i = 0; i < numbers.length; i += 1) {
		running += numbers[i];
		steps.push({
			ShowNumber: {
				session_id: 1,
				// Rust reports these 1-based; the driver event is 0-based and
				// the emitter adds one back.
				index: i + 1,
				total: numbers.length,
				value: numbers[i],
				running_sum: running,
				delay_ms_before_next: exposureMs,
			},
		});
		at += exposureMs;
		steps.push({
			ClearScreen: {
				session_id: 1,
				index: i + 1,
				delay_ms_before_next: GAP_MS,
			},
		});
		at += GAP_MS;
	}

	steps.push({
		ClearScreen: { session_id: 1, index: null, delay_ms_before_next: 0 },
	});
	steps.push({
		Complete: {
			session_id: 1,
			numbers,
			sum: numbers.reduce((a, b) => a + b, 0),
		},
	});

	return {
		session_id: 1,
		config_snapshot: configSnapshot({
			digits_per_number: options?.digits,
			total_numbers: options?.total ?? numbers.length,
		}),
		steps,
		total_duration_ms: at,
		numbers_generated: numbers,
		expected_sum: numbers.reduce((a, b) => a + b, 0),
	};
}

/**
 * Remove the registered bridge so the runtime uses the local JS planner.
 *
 * Distinct from registering a bridge that answers with a null plan, and the
 * difference is observable: `resolvePlannedSessionData` bails out only when
 * `getWasmCoreBridge()` actually returns null. A bridge whose `buildSessionPlan`
 * returns null instead reaches the plan validator and throws on
 * `numbers_generated`, so a test that wants the fallback has to clear the bridge
 * rather than stub a failure.
 */
export function clearWasmCoreBridge(): void {
	registerWasmCoreBridge(null as never);
}

/**
 * Register a bridge that answers every plan request with `plan`.
 *
 * The bridge is a process-wide singleton, so a test that registers one and
 * forgets to replace it leaves the next test talking to a plan it did not ask
 * for. `resetRuntime` does not clear it, so tests that need isolation should
 * register a fresh bridge per test.
 */
export function registerFakeWasmBridge(
	plan: WasmSessionPlan | (() => WasmSessionPlan | Promise<WasmSessionPlan>),
): void {
	registerWasmCoreBridge({
		async buildSessionPlan() {
			return typeof plan === "function" ? plan() : plan;
		},
	});
}
