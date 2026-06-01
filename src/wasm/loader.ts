import type { SessionConfigInput } from "../runtime/types";
import {
	registerWasmCoreBridge,
	type WasmCoreBridge,
	type WasmNormalizedSessionConfig,
	type WasmSessionPlan,
} from "./coreBridge";

let wasmBridgeLoaded = false;
let wasmBridgeLoadAttempted = false;
let wasmVersionFn: (() => string) | null = null;

function asFunction<TArgs extends unknown[], TResult>(
	value: unknown,
): ((...args: TArgs) => TResult) | null {
	return typeof value === "function"
		? (value as (...args: TArgs) => TResult)
		: null;
}

export async function loadWasmCoreBridge(): Promise<boolean> {
	if (wasmBridgeLoaded) {
		return true;
	}

	if (wasmBridgeLoadAttempted) {
		return false;
	}
	wasmBridgeLoadAttempted = true;

	try {
		const moduleUrl = new URL("./pkg/flashspan_core.js", import.meta.url).href;
		const wasmUrl = new URL("./pkg/flashspan_core_bg.wasm", import.meta.url)
			.href;
		const wasmModule = (await import(/* @vite-ignore */ moduleUrl)) as Record<
			string,
			unknown
		>;

		const init = asFunction<[string], Promise<unknown>>(wasmModule.default);
		const normalizeSessionConfigWasm = asFunction<
			[SessionConfigInput],
			WasmNormalizedSessionConfig
		>(wasmModule.normalize_session_config_wasm);
		const buildSessionPlanWasm = asFunction<
			[number, SessionConfigInput, number | null | undefined],
			WasmSessionPlan
		>(wasmModule.build_session_plan_wasm);
		const wasmVersion = asFunction<[], string>(wasmModule.wasm_version);

		if (!init || !normalizeSessionConfigWasm || !buildSessionPlanWasm) {
			console.info(
				"[wasm] Generated module exports are incomplete; skipping WASM bridge",
			);
			return false;
		}

		await init(wasmUrl);

		wasmVersionFn = wasmVersion;

		const bridge: WasmCoreBridge = {
			async normalizeSessionConfig(input: SessionConfigInput) {
				return normalizeSessionConfigWasm(input);
			},
			async buildSessionPlan(
				sessionId: number,
				input: SessionConfigInput,
				seed?: number | null,
			) {
				return buildSessionPlanWasm(sessionId, input, seed ?? null);
			},
		};

		registerWasmCoreBridge(bridge);
		wasmBridgeLoaded = true;
		console.info("[wasm] Rust core bridge loaded");
		return true;
	} catch {
		console.info(
			"[wasm] Rust core bridge not available; browser runtime will use JS planner",
		);
		return false;
	}
}

export function getWasmVersion(): string | null {
	if (!wasmVersionFn) {
		return null;
	}
	try {
		return wasmVersionFn();
	} catch (err) {
		console.error("[wasm] Failed to call wasm_version():", err);
		return null;
	}
}
