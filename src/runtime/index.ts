/**
 * Runtime abstraction layer for Flashspan.
 *
 * This module defines the platform-agnostic runtime interface that both
 * desktop (Tauri) and browser (WASM) runtimes implement. The UI depends
 * only on this interface, not on platform-specific APIs.
 *
 * Responsibilities:
 * - Manage session lifecycle (start, stop, answer submission)
 * - Emit session events (countdown, show, clear, complete)
 * - Handle app settings (color scheme, theme, sound)
 * - Coordinate with audio playback and fullscreen
 * - Bridge deterministic core logic with platform-specific effects
 */

// --- Re-export types that UI will use ---
export type {
	AppSettings,
	AutoRepeatConfig,
	AutoRepeatEffective,
	AutoRepeatTickPayload,
	AutoRepeatWaitingPayload,
	ClearScreen,
	ColorScheme,
	Phase,
	SessionComplete,
	SessionConfigEffective,
	SessionConfigInput,
	ShowNumber,
	StartSessionResponse,
	SubmitAnswerResponse,
	ThemeMode,
	UnlistenFn,
	ValidationResult,
} from "./types";

// --- Runtime instance singleton ---
let _runtime: Runtime | null = null;

/**
 * The runtime interface: all methods that UI code can call.
 * Both Tauri and browser implementations conform to this.
 */
export interface Runtime {
	// --- Commands (request/response) ---
	ping(): Promise<string>;
	getAppSettings(): Promise<AppSettings>;
	setColorScheme(scheme: ColorScheme): Promise<AppSettings>;
	setThemeMode(mode: ThemeMode): Promise<AppSettings>;
	startSession(
		config: SessionConfigInput,
		autoRepeat?: AutoRepeatConfig | null,
	): Promise<StartSessionResponse>;
	stopSession(): Promise<void>;
	cancelAutoRepeat(): Promise<void>;
	markValidated(sessionId: number): Promise<AutoRepeatWaitingPayload | null>;
	acknowledgeComplete(
		sessionId: number,
	): Promise<AutoRepeatWaitingPayload | null>;
	submitAnswer(
		sessionId: number,
		providedSum: number,
	): Promise<SubmitAnswerResponse>;
	submitAnswerText(
		sessionId: number,
		providedText: string,
	): Promise<SubmitAnswerResponse>;
	getSoundEnabled(): Promise<boolean>;
	setSoundEnabled(enabled: boolean): Promise<void>;
	playSound(kind: "beep" | "applause" | "buzzer"): Promise<void>;

	// --- Event listeners ---
	onCountdownTick(handler: (value: string) => void): Promise<UnlistenFn>;
	onShowNumber(handler: (payload: ShowNumber) => void): Promise<UnlistenFn>;
	onClearScreen(handler: (payload: ClearScreen) => void): Promise<UnlistenFn>;
	onAutoRepeatWaiting(
		handler: (payload: AutoRepeatWaitingPayload) => void,
	): Promise<UnlistenFn>;
	onAutoRepeatTick(
		handler: (payload: AutoRepeatTickPayload) => void,
	): Promise<UnlistenFn>;
	onAppSettingsChanged(
		handler: (payload: AppSettings) => void,
	): Promise<UnlistenFn>;
	onSessionComplete(
		handler: (payload: SessionComplete) => void,
	): Promise<UnlistenFn>;
}

// Import type (lazy-loaded to avoid circular deps)
import type {
	AppSettings,
	AutoRepeatConfig,
	AutoRepeatTickPayload,
	AutoRepeatWaitingPayload,
	ClearScreen,
	ColorScheme,
	SessionComplete,
	SessionConfigInput,
	ShowNumber,
	StartSessionResponse,
	SubmitAnswerResponse,
	ThemeMode,
	UnlistenFn,
} from "./types";

/**
 * Initialize the runtime for the current platform.
 * Call this once at app startup before any UI code tries to use the runtime.
 *
 * @param runtimeImpl - The runtime implementation (native or browser)
 */
export function initializeRuntime(runtimeImpl: Runtime): void {
	if (_runtime !== null) {
		console.warn(
			"[runtime] Runtime already initialized; skipping re-initialization",
		);
		return;
	}
	_runtime = runtimeImpl;
	console.log("[runtime] Runtime initialized");
}

/**
 * Get the current runtime instance.
 * Must be called after initializeRuntime().
 *
 * @throws Error if runtime not initialized
 */
export function getRuntime(): Runtime {
	if (_runtime === null) {
		throw new Error(
			"[runtime] Runtime not initialized. Call initializeRuntime() at app startup.",
		);
	}
	return _runtime;
}

/**
 * Convenience wrapper for common runtime operations.
 * This is what UI code typically imports and uses.
 */
export const runtime = {
	// Lazy getters to avoid errors if runtime not initialized yet
	get ping() {
		return () => getRuntime().ping();
	},
	get getAppSettings() {
		return () => getRuntime().getAppSettings();
	},
	get setColorScheme() {
		return (scheme: ColorScheme) => getRuntime().setColorScheme(scheme);
	},
	get setThemeMode() {
		return (mode: ThemeMode) => getRuntime().setThemeMode(mode);
	},
	get startSession() {
		return (config: SessionConfigInput, autoRepeat?: AutoRepeatConfig | null) =>
			getRuntime().startSession(config, autoRepeat);
	},
	get stopSession() {
		return () => getRuntime().stopSession();
	},
	get cancelAutoRepeat() {
		return () => getRuntime().cancelAutoRepeat();
	},
	get markValidated() {
		return (sessionId: number) => getRuntime().markValidated(sessionId);
	},
	get acknowledgeComplete() {
		return (sessionId: number) => getRuntime().acknowledgeComplete(sessionId);
	},
	get submitAnswer() {
		return (sessionId: number, providedSum: number) =>
			getRuntime().submitAnswer(sessionId, providedSum);
	},
	get submitAnswerText() {
		return (sessionId: number, providedText: string) =>
			getRuntime().submitAnswerText(sessionId, providedText);
	},
	get getSoundEnabled() {
		return () => getRuntime().getSoundEnabled();
	},
	get setSoundEnabled() {
		return (enabled: boolean) => getRuntime().setSoundEnabled(enabled);
	},
	get playSound() {
		return (kind: "beep" | "applause" | "buzzer") =>
			getRuntime().playSound(kind);
	},
	get onCountdownTick() {
		return (handler: (value: string) => void) =>
			getRuntime().onCountdownTick(handler);
	},
	get onShowNumber() {
		return (handler: (payload: ShowNumber) => void) =>
			getRuntime().onShowNumber(handler);
	},
	get onClearScreen() {
		return (handler: (payload: ClearScreen) => void) =>
			getRuntime().onClearScreen(handler);
	},
	get onAutoRepeatWaiting() {
		return (handler: (payload: AutoRepeatWaitingPayload) => void) =>
			getRuntime().onAutoRepeatWaiting(handler);
	},
	get onAutoRepeatTick() {
		return (handler: (payload: AutoRepeatTickPayload) => void) =>
			getRuntime().onAutoRepeatTick(handler);
	},
	get onAppSettingsChanged() {
		return (handler: (payload: AppSettings) => void) =>
			getRuntime().onAppSettingsChanged(handler);
	},
	get onSessionComplete() {
		return (handler: (payload: SessionComplete) => void) =>
			getRuntime().onSessionComplete(handler);
	},
};
