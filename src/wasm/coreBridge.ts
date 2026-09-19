import type {
	SessionConfigEffective,
	SessionConfigInput,
} from "../runtime/types";

export interface WasmSessionConfig {
	digits_per_number: number;
	number_duration_ms: number;
	total_numbers: number;
	allow_negative_numbers: boolean;
}

export interface WasmNormalizedSessionConfig {
	config: WasmSessionConfig;
	effective: SessionConfigEffective;
}

/**
 * Plan steps as serialized by serde (externally tagged enum — exactly the
 * shape Rust `SessionStep` produces). Mirrors `src-tauri/src/core/types.rs`;
 * any shape drift breaks `planStepsToEvents` validation and falls back to
 * the local JS planner.
 */
export type WasmSessionStep =
	| {
			CountdownTick: { value: string; delay_ms_before_next: number };
	  }
	| {
			ShowNumber: {
				session_id: number;
				index: number;
				total: number;
				value: number;
				running_sum: number;
				delay_ms_before_next: number;
			};
	  }
	| {
			ClearScreen: {
				session_id: number;
				index: number | null;
				delay_ms_before_next: number;
			};
	  }
	| {
			Complete: { session_id: number; numbers: number[]; sum: number };
	  };

export interface WasmSessionPlan {
	session_id: number;
	config_snapshot: SessionConfigEffective;
	steps: WasmSessionStep[];
	total_duration_ms: number;
	numbers_generated: number[];
	expected_sum: number;
}

export interface WasmCoreBridge {
	normalizeSessionConfig(
		input: SessionConfigInput,
	): Promise<WasmNormalizedSessionConfig>;
	buildSessionPlan(
		sessionId: number,
		input: SessionConfigInput,
		seed?: number | null,
	): Promise<WasmSessionPlan>;
}

let wasmCoreBridge: WasmCoreBridge | null = null;

export function registerWasmCoreBridge(bridge: WasmCoreBridge): void {
	wasmCoreBridge = bridge;
}

export function getWasmCoreBridge(): WasmCoreBridge | null {
	return wasmCoreBridge;
}
