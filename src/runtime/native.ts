/**
 * Native runtime implementation for Tauri desktop.
 *
 * This adapter wraps the existing Tauri API and conforms to the Runtime interface.
 * It enables the UI to stay platform-agnostic while delegating to Tauri for:
 * - IPC command invocation
 * - Event emission and subscription
 * - Sound state and playback health (playback itself belongs to Rust)
 * - Fullscreen and window management
 */

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Runtime, UnlistenFn } from "./index";
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
} from "./types";

/**
 * Subscribe to a Tauri event, forwarding its payload to `handler`.
 *
 * Every listener below is the same `listen` call with a different event name
 * and payload type, so they share this helper rather than repeating the
 * subscribe boilerplate eight times. The cast is needed because Tauri's
 * `UnlistenFn` is structurally compatible but nominally distinct.
 */
async function subscribe<T>(
	event: string,
	handler: (payload: T) => void,
): Promise<UnlistenFn> {
	const unlisten = await listen<T>(event, (e) => handler(e.payload));
	return unlisten as UnlistenFn;
}

/**
 * Native runtime: wraps existing Tauri API.
 * This is the desktop implementation used by the Tauri app.
 */
export const nativeRuntime: Runtime = {
	// --- Commands ---
	async ping(): Promise<string> {
		return invoke<string>("ping");
	},

	async getAppSettings(): Promise<AppSettings> {
		return invoke<AppSettings>("get_app_settings");
	},

	async setColorScheme(scheme: ColorScheme): Promise<AppSettings> {
		return invoke<AppSettings>("set_color_scheme", { colorScheme: scheme });
	},

	async setThemeMode(mode: ThemeMode): Promise<AppSettings> {
		return invoke<AppSettings>("set_theme_mode", { themeMode: mode });
	},

	async startSession(
		config: SessionConfigInput,
		autoRepeat?: AutoRepeatConfig | null,
	): Promise<StartSessionResponse | null> {
		if (autoRepeat) {
			return invoke<StartSessionResponse>("start_session", {
				config,
				autoRepeat: autoRepeat,
			});
		}
		return invoke<StartSessionResponse>("start_session", { config });
	},

	async stopSession(): Promise<void> {
		return invoke<void>("stop_session");
	},

	async cancelAutoRepeat(): Promise<void> {
		return invoke<void>("cancel_auto_repeat");
	},

	async acknowledgeComplete(
		sessionId: number,
	): Promise<AutoRepeatWaitingPayload | null> {
		return invoke<AutoRepeatWaitingPayload | null>("acknowledge_complete", {
			sessionId: sessionId,
		});
	},

	async submitAnswer(
		sessionId: number,
		providedSum: number,
	): Promise<SubmitAnswerResponse> {
		return invoke<SubmitAnswerResponse>("submit_answer", {
			args: {
				sessionId: sessionId,
				providedSum: providedSum,
			},
		});
	},

	async submitAnswerText(
		sessionId: number,
		providedText: string,
	): Promise<SubmitAnswerResponse> {
		return invoke<SubmitAnswerResponse>("submit_answer_text", {
			args: {
				sessionId: sessionId,
				providedText: providedText,
			},
		});
	},

	async getSoundEnabled(): Promise<boolean> {
		return invoke<boolean>("get_sound_enabled");
	},

	async setSoundEnabled(enabled: boolean): Promise<void> {
		return invoke<void>("set_sound_enabled", { enabled });
	},

	async getAudioStatus(): Promise<AudioStatus> {
		return invoke<AudioStatus>("get_audio_status");
	},

	// No `playSound` here, and none on `Runtime` either: Rust owns every sound
	// on desktop, both the session's flash beeps and the answer feedback, so
	// nothing in the UI can ask for one. The HTML Audio fallback that used to
	// live here was reachable only from that unused method, and it eagerly
	// fetched and decoded all three clips (~429KB, applause alone 350KB) at
	// module load for a path that could not run.

	// --- Event listeners ---
	async onCountdownTick(handler: (value: string) => void): Promise<UnlistenFn> {
		return subscribe<string>("countdown_tick", (value) =>
			handler(String(value ?? "")),
		);
	},

	async onShowNumber(
		handler: (payload: ShowNumber) => void,
	): Promise<UnlistenFn> {
		return subscribe<ShowNumber>("show_number", handler);
	},

	async onClearScreen(
		handler: (payload: ClearScreen) => void,
	): Promise<UnlistenFn> {
		return subscribe<ClearScreen>("clear_screen", handler);
	},

	async onAutoRepeatWaiting(
		handler: (payload: AutoRepeatWaitingPayload) => void,
	): Promise<UnlistenFn> {
		return subscribe<AutoRepeatWaitingPayload>("auto_repeat_waiting", handler);
	},

	async onAutoRepeatTick(
		handler: (payload: AutoRepeatTickPayload) => void,
	): Promise<UnlistenFn> {
		return subscribe<AutoRepeatTickPayload>("auto_repeat_tick", handler);
	},

	async onAppSettingsChanged(
		handler: (payload: AppSettings) => void,
	): Promise<UnlistenFn> {
		return subscribe<AppSettings>("app_settings_changed", handler);
	},

	async onSessionComplete(
		handler: (payload: SessionComplete) => void,
	): Promise<UnlistenFn> {
		return subscribe<SessionComplete>("session_complete", handler);
	},

	/**
	 * No-op. A desktop window has no hidden-tab case worth reporting — the
	 * process keeps running and the session stays in step with the user — so
	 * there is nothing to interrupt.
	 *
	 * Implemented rather than omitted so `Runtime` can require the member and the
	 * delegator in `index.ts` can forward every method uniformly. `App.tsx`
	 * subscribes without checking for absence, and this never fires.
	 */
	async onSessionInterrupted(): Promise<UnlistenFn> {
		return () => {};
	},
};
