import type {
	SessionConfigEffective,
	SessionConfigInput,
} from "../runtime/types";

export interface WasmSessionConfig {
	digits_per_number: number;
	number_duration_ms: number;
	delay_between_numbers_ms: number;
	total_numbers: number;
	allow_negative_numbers: boolean;
}

export interface WasmNormalizedSessionConfig {
	config: WasmSessionConfig;
	effective: SessionConfigEffective;
}

export interface WasmSessionPlan {
	session_id: number;
	config_snapshot: SessionConfigEffective;
	steps: unknown[];
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
