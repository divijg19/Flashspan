/**
 * Shared type definitions for the runtime layer.
 * These types are used by both native and browser runtimes.
 */

export type Phase = "idle" | "starting" | "countdown" | "flashing" | "complete";

export type ColorScheme =
	| "midnight"
	| "ivory"
	| "crimson"
	| "aqua"
	| "violet"
	| "amber";

export type ThemeMode = "dark" | "light";

export type UnlistenFn = () => void;

export interface AppSettings {
	color_scheme: ColorScheme;
	theme_mode: ThemeMode;
}

export interface SessionConfigInput {
	digits_per_number: number;
	number_duration_s: number;
	delay_between_numbers_s: number;
	total_numbers: number;
	allow_negative_numbers: boolean;
}

export interface SessionConfigEffective {
	digits_per_number: number;
	number_duration_s: number;
	delay_between_numbers_s: number;
	total_numbers: number;
	allow_negative_numbers: boolean;
}

export interface AutoRepeatConfig {
	enabled: boolean;
	repeats: number;
	delay_s: number;
}

export interface AutoRepeatEffective {
	enabled: boolean;
	repeats: number;
	delay_s: number;
}

export interface StartSessionResponse {
	session_id: number;
	effective_config: SessionConfigEffective;
	effective_auto_repeat: AutoRepeatEffective | null;
}

export interface ShowNumber {
	session_id: number;
	index: number;
	total: number;
	value: number;
	running_sum: number;
	emitted_at_ms: number;
}

export interface ClearScreen {
	session_id: number;
	index: number | null;
	emitted_at_ms: number;
}

export interface SessionComplete {
	session_id: number;
	numbers: number[];
	sum: number;
}

export interface AutoRepeatWaitingPayload {
	session_id: number;
	next_start_at_ms: number;
	remaining: number;
}

export interface AutoRepeatTickPayload {
	session_id: number;
	seconds_left: number;
	remaining: number;
}

export interface ValidationResult {
	expected_sum: number;
	provided_sum: number;
	correct: boolean;
	delta: number;
}

export interface SubmitAnswerResponse {
	validation: ValidationResult;
	auto_repeat_waiting: AutoRepeatWaitingPayload | null;
}
