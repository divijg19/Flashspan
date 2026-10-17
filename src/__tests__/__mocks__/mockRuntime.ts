import type {
	AppSettings,
	AutoRepeatTickPayload,
	AutoRepeatWaitingPayload,
	ClearScreen,
	Runtime,
	SessionComplete,
	ShowNumber,
} from "../../runtime";
import { formatValidationMessage } from "../../runtime/grading";

type Listener<T> = (payload: T) => void;

export function createMockRuntime() {
	let audioStatus = {
		enabled: true,
		available: true,
		detail: "ready",
	};
	const listeners = {
		countdownTick: new Set<Listener<string>>(),
		showNumber: new Set<Listener<ShowNumber>>(),
		clearScreen: new Set<Listener<ClearScreen>>(),
		autoRepeatWaiting: new Set<Listener<AutoRepeatWaitingPayload>>(),
		autoRepeatTick: new Set<Listener<AutoRepeatTickPayload>>(),
		appSettingsChanged: new Set<Listener<AppSettings>>(),
		sessionComplete: new Set<Listener<SessionComplete>>(),
		// Only the browser runtime has a hidden-tab case worth reporting, but
		// the mock implements it so App's handler for that path is reachable from
		// a test. It was not before, and the handler that returns the session to
		// idle and explains why had no coverage.
		//
		// A bare function set rather than `Listener<void>`: this event carries no
		// payload, and `Listener<T>` is `(payload: T) => void`, which would make
		// the handler take an argument it does not have.
		sessionInterrupted: new Set<() => void>(),
	};

	/**
	 * Annotated `Runtime` for two reasons, and the second is the one that matters:
	 *
	 * - Exhaustiveness is checked, so a missing member is a compile error rather
	 *   than `runtime.foo()` being `undefined` inside some later test.
	 * - The methods carry the interface's signatures instead of the narrow
	 *   literal types they would otherwise infer. Tests override individual
	 *   methods — `mock.startSession = async () => ...` — and a literal return
	 *   type like `correct: true` rejects an override returning `correct: false`
	 *   even though `Runtime` allows both. The previous `as` cast widened the
	 *   whole object and hid exactly that.
	 *
	 * The emit helpers are spread on afterwards because they are not part of
	 * `Runtime`.
	 */
	const implementation: Runtime = {
		// commands
		async ping() {
			return "pong (mock)";
		},
		async getAppSettings() {
			return { color_scheme: "midnight", theme_mode: "dark" };
		},
		async setColorScheme(s) {
			return { color_scheme: s, theme_mode: "dark" };
		},
		async setThemeMode(m) {
			return { color_scheme: "midnight", theme_mode: m };
		},
		async startSession() {
			return {
				session_id: 1,
				effective_config: {
					digits_per_number: 1,
					number_duration_s: 0.5,
					total_numbers: 1,
					allow_negative_numbers: false,
				},
				effective_auto_repeat: null,
			};
		},
		async stopSession() {},
		async cancelAutoRepeat() {},
		async acknowledgeComplete() {
			return null;
		},
		async submitAnswer(_sessionId: number, providedSum: number) {
			return {
				validation: {
					expected_sum: providedSum,
					provided_sum: providedSum,
					correct: true,
					delta: 0,
				},
				auto_repeat_waiting: null,
				message: formatValidationMessage(true, providedSum, 0),
			};
		},
		async submitAnswerText(_sessionId: number, providedText: string) {
			const v = Number(providedText.replace(/[\s,]+/g, "")) || 0;
			return {
				validation: {
					expected_sum: v,
					provided_sum: v,
					correct: true,
					delta: 0,
				},
				auto_repeat_waiting: null,
				message: formatValidationMessage(true, v, 0),
			};
		},
		async getSoundEnabled() {
			return true;
		},
		async setSoundEnabled() {},
		async getAudioStatus() {
			return { ...audioStatus };
		},
		async playSound() {},

		// event listeners
		async onCountdownTick(handler: (v: string) => void) {
			listeners.countdownTick.add(handler);
			return () => listeners.countdownTick.delete(handler);
		},
		async onShowNumber(handler: (p: ShowNumber) => void) {
			listeners.showNumber.add(handler);
			return () => listeners.showNumber.delete(handler);
		},
		async onClearScreen(handler: (p: ClearScreen) => void) {
			listeners.clearScreen.add(handler);
			return () => listeners.clearScreen.delete(handler);
		},
		async onAutoRepeatWaiting(handler: (p: AutoRepeatWaitingPayload) => void) {
			listeners.autoRepeatWaiting.add(handler);
			return () => listeners.autoRepeatWaiting.delete(handler);
		},
		async onAutoRepeatTick(handler: (p: AutoRepeatTickPayload) => void) {
			listeners.autoRepeatTick.add(handler);
			return () => listeners.autoRepeatTick.delete(handler);
		},
		async onSessionInterrupted(handler: () => void) {
			listeners.sessionInterrupted.add(handler);
			return () => listeners.sessionInterrupted.delete(handler);
		},
		async onAppSettingsChanged(handler: (p: AppSettings) => void) {
			listeners.appSettingsChanged.add(handler);
			return () => listeners.appSettingsChanged.delete(handler);
		},
		async onSessionComplete(handler: (p: SessionComplete) => void) {
			listeners.sessionComplete.add(handler);
			return () => listeners.sessionComplete.delete(handler);
		},
	};

	return {
		...implementation,

		// helpers to emit events in tests
		emitCountdown(v: string) {
			for (const h of [...listeners.countdownTick]) h(v);
		},
		emitShowNumber(payload: ShowNumber) {
			for (const h of [...listeners.showNumber]) h(payload);
		},
		emitClearScreen(payload: ClearScreen) {
			for (const h of [...listeners.clearScreen]) h(payload);
		},
		emitSessionComplete(payload: SessionComplete) {
			for (const h of [...listeners.sessionComplete]) h(payload);
		},
		emitSessionInterrupted() {
			for (const h of [...listeners.sessionInterrupted]) h();
		},
		setAudioStatus(next: {
			enabled: boolean;
			available: boolean;
			detail: string;
		}) {
			audioStatus = { ...next };
		},
	};
}
