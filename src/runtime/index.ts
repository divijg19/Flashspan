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

// Re-export every runtime type, so `./runtime` is the one import the UI needs.
// `export type *` keeps that surface in step with `./types` automatically,
// rather than maintaining a second hand-written list that can silently fall
// behind (the old copy had already drifted: it omitted `SoundKind`).
export type * from "./types";

// The names the `Runtime` interface itself references. Type-only, so the
// import is erased at build time.
import type {
	AppSettings,
	AudioStatus,
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
	// Null means no session was started: the browser runtime bails when a stop
	// supersedes the start while its plan was still building.
	startSession(
		config: SessionConfigInput,
		autoRepeat?: AutoRepeatConfig | null,
	): Promise<StartSessionResponse | null>;
	stopSession(): Promise<void>;
	cancelAutoRepeat(): Promise<void>;
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
	getAudioStatus(): Promise<AudioStatus>;
	// No `playSound`: neither runtime can be asked to play one. The desktop
	// plays every sound inside Rust (the session's beeps and the answer
	// feedback) and the browser plays them from its own session code, so a
	// command for a one-off playback had no caller — and on desktop it carried
	// an HTML Audio fallback that was unreachable, while eagerly fetching all
	// three clips at module load.

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
	/**
	 * A running session was abandoned while the tab was hidden and had to be
	 * interrupted.
	 *
	 * Required rather than optional so the `Runtime` delegator can forward every
	 * member uniformly and callers do not each have to handle an absent method.
	 * Only the browser runtime has a hidden-tab case worth reporting; the native
	 * one implements a no-op, which never fires.
	 */
	onSessionInterrupted(handler: () => void): Promise<UnlistenFn>;
	onAppSettingsChanged(
		handler: (payload: AppSettings) => void,
	): Promise<UnlistenFn>;
	onSessionComplete(
		handler: (payload: SessionComplete) => void,
	): Promise<UnlistenFn>;
}

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
 * Drop the current runtime so the next `initializeRuntime` takes effect.
 *
 * `initializeRuntime` deliberately refuses a second call, which is right for
 * the app (one runtime per process) but means a test file that builds a mock
 * per test silently shares the first one for the rest of the file. Tests then
 * emit events on a mock the component under test never subscribed to, and the
 * failure surfaces as a missing element rather than as the isolation bug it is.
 *
 * Test-only seam: nothing in `src/` calls it.
 */
export function resetRuntime(): void {
	_runtime = null;
}

/**
 * Get the current runtime instance.
 * Must be called after initializeRuntime().
 *
 * @throws Error if runtime not initialized
 */
function getRuntime(): Runtime {
	if (_runtime === null) {
		throw new Error(
			"[runtime] Runtime not initialized. Call initializeRuntime() at app startup.",
		);
	}
	return _runtime;
}

/**
 * The delegating runtime the UI uses.
 *
 * Typed `Runtime` so adding a method to the interface is a compile error here if
 * it is not also added below — the hand-written getters this replaced were
 * unchecked, so a missing member surfaced as `runtime.newMethod()` being
 * `undefined` inside the browser rather than at compile time.
 *
 * A Proxy keeps that exhaustiveness without repeating the delegation: every
 * accessor and method on every implementation is forwarded automatically.
 */
export const runtime: Runtime = new Proxy({} as Runtime, {
	get(_target, property) {
		const implementation = getRuntime();
		const member = Reflect.get(implementation, property);
		// Methods must be rebound: `browserRuntime` and `nativeRuntime` are
		// object literals whose methods close over module-level state, and
		// several read `this` implicitly through sibling members.
		return typeof member === "function"
			? (member as (...args: never[]) => unknown).bind(implementation)
			: member;
	},
});
