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

/**
 * The colour schemes the picker offers, in display order.
 *
 * Declared once, next to the `ColorScheme` union it enumerates. The union used
 * to be repeated twice more — as a `const` array in the settings panel and as a
 * six-way inline comparison in the settings loader — so adding a scheme to the
 * type without updating both left the picker without the new option and the
 * loader silently resetting a saved preference to the default.
 *
 * Both drift directions are compile errors now, by different mechanisms:
 * `satisfies` rejects a scheme the union does not know about, and
 * `_AllSchemesListed` below rejects a union member the list has forgotten.
 */
export const COLOR_SCHEMES = [
	"midnight",
	"crimson",
	"aqua",
	"violet",
	"amber",
	"ivory",
] as const satisfies readonly ColorScheme[];

/**
 * Compile-time proof that the list above covers every `ColorScheme`.
 *
 * `satisfies` alone is not enough: it rejects a scheme the union does not know
 * about, but a *missing* member still satisfies it, and a missing member is the
 * dangerous direction. `_ExpectTrue` takes the union check as a constraint, so a
 * forgotten entry resolves the conditional to `false` and the alias fails to
 * compile — with no runtime value emitted.
 */
type _ExpectTrue<T extends true> = T;
type _AllSchemesListed = _ExpectTrue<
	ColorScheme extends (typeof COLOR_SCHEMES)[number] ? true : false
>;

/** The sound clips the runtime can play. */
export type SoundKind = "beep" | "applause" | "buzzer";

export type ThemeMode = "dark" | "light";

export type UnlistenFn = () => void;

/** A runtime event handler: receives one payload of `T`. */
export type Listener<T> = (payload: T) => void;

export interface AppSettings {
	color_scheme: ColorScheme;
	theme_mode: ThemeMode;
}

/**
 * The session-shape fields, declared once for the two names below.
 *
 * `SessionConfigInput` is what the UI asked for; `SessionConfigEffective` is
 * what the runtime actually applied after clamping. The two are field-identical
 * on purpose — the names mark which side of the boundary a value crossed — and
 * sharing this base is what stops the pair drifting apart field by field.
 */
export interface SessionConfigShape {
	digits_per_number: number;
	number_duration_s: number;
	total_numbers: number;
	allow_negative_numbers: boolean;
}

export type SessionConfigInput = SessionConfigShape;
export type SessionConfigEffective = SessionConfigShape;

/**
 * The auto-repeat fields, shared by the requested (`AutoRepeatConfig`) and
 * applied (`AutoRepeatEffective`) names for the same reason as the session
 * pair above.
 */
export interface AutoRepeatShape {
	enabled: boolean;
	repeats: number;
	delay_s: number;
}

export type AutoRepeatConfig = AutoRepeatShape;
export type AutoRepeatEffective = AutoRepeatShape;

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

export interface AudioStatus {
	enabled: boolean;
	available: boolean;
	detail: string;
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
	message: string;
}
