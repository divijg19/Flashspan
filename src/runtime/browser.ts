/**
 * Browser runtime implementation.
 *
 * This keeps the Solid UI unchanged while swapping the backend execution model
 * to browser timers, localStorage, and Web Audio.
 */

import applauseUrl from "../assets/applause.wav?url";
import beepUrl from "../assets/beep.wav?url";
import buzzerUrl from "../assets/buzzer.wav?url";
import { getWasmCoreBridge } from "../wasm/coreBridge";
import type { Runtime, UnlistenFn } from "./index";
import type {
	AppSettings,
	AutoRepeatConfig,
	AutoRepeatEffective,
	AutoRepeatTickPayload,
	AutoRepeatWaitingPayload,
	ClearScreen,
	ColorScheme,
	SessionComplete,
	SessionConfigEffective,
	SessionConfigInput,
	ShowNumber,
	StartSessionResponse,
	SubmitAnswerResponse,
	ThemeMode,
} from "./types";

type Listener<T> = (payload: T) => void;

interface PendingAutoRepeat {
	sessionId: number;
	remaining: number;
	delayMs: number;
	nextStartAtMs: number;
	config: SessionConfigEffective;
	cancelled: boolean;
	tickId: number | null;
}

interface BrowserSession {
	sessionId: number;
	config: SessionConfigEffective;
	autoRepeat: AutoRepeatEffective | null;
	plannedNumbers: number[] | null;
	plannedSum: number | null;
	numbers: number[];
	sum: number;
	runningSum: number;
	lastPayload: string | null;
	completed: boolean;
	timers: number[];
}

const SETTINGS_KEY = "flashspan.runtime.settings";
const SOUND_KEY = "flashspan.runtime.sound-enabled";
const DEFAULT_SETTINGS: AppSettings = {
	color_scheme: "midnight",
	theme_mode: "dark",
};
const COUNTDOWN_TICKS = [3, 2, 1] as const;
const FIRST_FLASH_GRACE_MS = 100;

const listeners = {
	countdownTick: new Set<Listener<string>>(),
	showNumber: new Set<Listener<ShowNumber>>(),
	clearScreen: new Set<Listener<ClearScreen>>(),
	autoRepeatWaiting: new Set<Listener<AutoRepeatWaitingPayload>>(),
	autoRepeatTick: new Set<Listener<AutoRepeatTickPayload>>(),
	appSettingsChanged: new Set<Listener<AppSettings>>(),
	sessionComplete: new Set<Listener<SessionComplete>>(),
};

let nextSessionId = 1;
let currentSession: BrowserSession | null = null;
let pendingAutoRepeat: PendingAutoRepeat | null = null;
let appSettings = loadAppSettings();
let soundEnabled = loadSoundEnabled();
let hasLoggedWasmPlannerReady = false;
let hasLoggedWasmPlannerFallback = false;

const audio = {
	beep: new Audio(beepUrl),
	applause: new Audio(applauseUrl),
	buzzer: new Audio(buzzerUrl),
};

Object.values(audio).forEach((clip) => {
	clip.preload = "auto";
	clip.volume = 0.9;
});

function clamp(value: number, min: number, max: number): number {
	if (Number.isNaN(value)) {
		return min;
	}

	return Math.min(max, Math.max(min, value));
}

function round1(value: number): number {
	return Math.round(value * 10) / 10;
}

function nowMs(): number {
	return Date.now();
}

function toMs(seconds: number): number {
	return Math.max(0, Math.round(seconds * 1000));
}

function safeInt(value: number): number {
	if (!Number.isFinite(value)) {
		return 0;
	}

	return Math.trunc(value);
}

function loadAppSettings(): AppSettings {
	try {
		const raw = window.localStorage.getItem(SETTINGS_KEY);
		if (!raw) {
			return { ...DEFAULT_SETTINGS };
		}

		const parsed = JSON.parse(raw) as Partial<AppSettings>;
		const validColorScheme =
			parsed.color_scheme === "midnight" ||
			parsed.color_scheme === "ivory" ||
			parsed.color_scheme === "crimson" ||
			parsed.color_scheme === "aqua" ||
			parsed.color_scheme === "violet" ||
			parsed.color_scheme === "amber"
				? parsed.color_scheme
				: DEFAULT_SETTINGS.color_scheme;

		return {
			color_scheme: validColorScheme,
			theme_mode: parsed.theme_mode === "light" ? "light" : "dark",
		};
	} catch {
		return { ...DEFAULT_SETTINGS };
	}
}

function persistAppSettings(): void {
	try {
		window.localStorage.setItem(SETTINGS_KEY, JSON.stringify(appSettings));
	} catch {
		// Best-effort only.
	}
}

function loadSoundEnabled(): boolean {
	try {
		const raw = window.localStorage.getItem(SOUND_KEY);
		return raw == null ? true : raw === "true";
	} catch {
		return true;
	}
}

function persistSoundEnabled(): void {
	try {
		window.localStorage.setItem(SOUND_KEY, String(soundEnabled));
	} catch {
		// Best-effort only.
	}
}

function emit<T>(set: Set<Listener<T>>, payload: T): void {
	for (const handler of set) {
		try {
			handler(payload);
		} catch {
			// Listener failures should not break the runtime.
		}
	}
}

function addListener<T>(
	set: Set<Listener<T>>,
	handler: Listener<T>,
): UnlistenFn {
	set.add(handler);
	return () => set.delete(handler);
}

function normalizeSessionConfig(
	input: SessionConfigInput,
): SessionConfigEffective {
	return {
		digits_per_number: clamp(safeInt(input.digits_per_number), 1, 18),
		number_duration_s: round1(clamp(input.number_duration_s, 0.1, 60)),
		delay_between_numbers_s: round1(
			clamp(input.delay_between_numbers_s, 0, 60),
		),
		total_numbers: clamp(safeInt(input.total_numbers), 1, 10000),
		allow_negative_numbers: Boolean(input.allow_negative_numbers),
	};
}

function normalizeAutoRepeat(
	autoRepeat?: AutoRepeatConfig | null,
): AutoRepeatEffective | null {
	if (!autoRepeat?.enabled) {
		return null;
	}

	return {
		enabled: true,
		repeats: clamp(safeInt(autoRepeat.repeats), 0, 20),
		delay_s: round1(clamp(autoRepeat.delay_s, 0, 120)),
	};
}

function clearSessionTimers(session: BrowserSession): void {
	for (const timerId of session.timers) {
		window.clearTimeout(timerId);
	}
	session.timers.length = 0;
}

function queueTimer(
	session: BrowserSession,
	delayMs: number,
	callback: () => void,
): void {
	const timerId = window.setTimeout(
		() => {
			session.timers = session.timers.filter((value) => value !== timerId);
			if (
				currentSession?.sessionId !== session.sessionId ||
				session.completed
			) {
				return;
			}

			callback();
		},
		Math.max(0, delayMs),
	);

	session.timers.push(timerId);
}

function emitClearScreen(sessionId: number, index: number | null): void {
	emit(listeners.clearScreen, {
		session_id: sessionId,
		index,
		emitted_at_ms: nowMs(),
	});
}

function playAudio(kind: "beep" | "applause" | "buzzer"): void {
	if (!soundEnabled) {
		return;
	}

	const clip = audio[kind];

	try {
		clip.currentTime = 0;
		void clip.play();
	} catch {
		// Best-effort only.
	}
}

function randomInt(maxExclusive: number): number {
	if (maxExclusive <= 1) {
		return 0;
	}

	return Math.floor(Math.random() * maxExclusive);
}

function randomMagnitude(digits: number): number {
	if (digits <= 1) {
		return randomInt(10);
	}

	const min = 10 ** (digits - 1);
	const span = 10 ** digits - min;
	return min + randomInt(span);
}

function generateNumber(
	digits: number,
	allowNegative: boolean,
	index: number,
	runningSum: number,
	lastPayload: string | null,
): { payload: string; value: number } {
	let attempt = 0;

	while (attempt < 256) {
		const magnitude = randomMagnitude(digits);
		const negativeAllowed = allowNegative && index > 0;
		const signedValue =
			negativeAllowed && randomInt(2) === 1 ? -magnitude : magnitude;

		if (index === 0 && signedValue < 0) {
			attempt += 1;
			continue;
		}

		if (runningSum + signedValue < 0) {
			attempt += 1;
			continue;
		}

		const payload = String(signedValue);
		if (payload === lastPayload) {
			attempt += 1;
			continue;
		}

		return { payload, value: signedValue };
	}

	const fallbackMagnitude = Math.max(1, randomMagnitude(digits));
	const fallbackValue =
		runningSum - fallbackMagnitude >= 0
			? -fallbackMagnitude
			: fallbackMagnitude;
	return {
		payload: String(fallbackValue),
		value: fallbackValue,
	};
}

function buildBridgeSeed(): number {
	const cryptoObj = globalThis.crypto;
	if (cryptoObj?.getRandomValues) {
		const buffer = new Uint32Array(1);
		cryptoObj.getRandomValues(buffer);
		return buffer[0];
	}

	return Math.trunc(Date.now() ^ Math.floor(Math.random() * 0x7fffffff));
}

async function resolvePlannedSessionData(
	sessionId: number,
	config: SessionConfigInput,
): Promise<{
	config: SessionConfigEffective;
	numbers: number[];
	sum: number;
} | null> {
	const bridge = getWasmCoreBridge();
	if (!bridge) {
		if (!hasLoggedWasmPlannerFallback) {
			hasLoggedWasmPlannerFallback = true;
			console.info(
				"[runtime/browser] WASM planner unavailable; using JS planner",
			);
		}
		return null;
	}

	if (!hasLoggedWasmPlannerReady) {
		hasLoggedWasmPlannerReady = true;
		console.info("[runtime/browser] WASM planner active");
	}

	let plan: Awaited<ReturnType<typeof bridge.buildSessionPlan>>;
	try {
		plan = await bridge.buildSessionPlan(sessionId, config, buildBridgeSeed());
	} catch {
		if (!hasLoggedWasmPlannerFallback) {
			hasLoggedWasmPlannerFallback = true;
			console.info("[runtime/browser] WASM planner failed; using JS planner");
		}
		return null;
	}

	if (
		!Array.isArray(plan.numbers_generated) ||
		plan.numbers_generated.length === 0
	) {
		return null;
	}

	return {
		config: plan.config_snapshot,
		numbers: plan.numbers_generated.slice(),
		sum: plan.expected_sum,
	};
}

function scheduleAutoRepeatCountdown(): void {
	if (!pendingAutoRepeat || pendingAutoRepeat.cancelled) {
		return;
	}

	const tick = (): void => {
		if (!pendingAutoRepeat || pendingAutoRepeat.cancelled) {
			return;
		}

		const remainingMs = pendingAutoRepeat.nextStartAtMs - nowMs();
		const secondsLeft = Math.max(0, Math.ceil(remainingMs / 1000));

		emit(listeners.autoRepeatTick, {
			session_id: pendingAutoRepeat.sessionId,
			seconds_left: secondsLeft,
			remaining: pendingAutoRepeat.remaining,
		});

		if (remainingMs <= 0) {
			const next = pendingAutoRepeat;
			pendingAutoRepeat = null;
			void startSessionImpl(next.config, {
				enabled: true,
				repeats: next.remaining,
				delay_s: next.delayMs / 1000,
			});
			return;
		}

		pendingAutoRepeat.tickId = window.setTimeout(tick, 1000);
	};

	tick();
}

function armAutoRepeatForSession(
	sessionId: number,
): AutoRepeatWaitingPayload | null {
	const session = currentSession;
	if (
		!session ||
		session.sessionId !== sessionId ||
		!session.completed ||
		!session.autoRepeat
	) {
		return null;
	}

	if (session.autoRepeat.repeats <= 0) {
		return null;
	}

	if (
		pendingAutoRepeat?.sessionId === sessionId &&
		!pendingAutoRepeat.cancelled
	) {
		return {
			session_id: sessionId,
			next_start_at_ms: pendingAutoRepeat.nextStartAtMs,
			remaining: pendingAutoRepeat.remaining,
		};
	}

	const remaining = Math.max(0, session.autoRepeat.repeats - 1);
	session.autoRepeat = {
		...session.autoRepeat,
		repeats: remaining,
	};

	pendingAutoRepeat = {
		sessionId,
		remaining,
		delayMs: toMs(session.autoRepeat.delay_s),
		nextStartAtMs: nowMs() + toMs(session.autoRepeat.delay_s),
		config: session.config,
		cancelled: false,
		tickId: null,
	};

	const payload: AutoRepeatWaitingPayload = {
		session_id: sessionId,
		next_start_at_ms: pendingAutoRepeat.nextStartAtMs,
		remaining: pendingAutoRepeat.remaining,
	};

	emit(listeners.autoRepeatWaiting, payload);
	scheduleAutoRepeatCountdown();
	return payload;
}

function finishSession(session: BrowserSession): void {
	if (session.completed) {
		return;
	}

	session.completed = true;
	emitClearScreen(session.sessionId, null);
	emit(listeners.sessionComplete, {
		session_id: session.sessionId,
		numbers: session.numbers.slice(),
		sum: session.plannedSum ?? session.sum,
	});
}

async function startSessionImpl(
	config: SessionConfigInput,
	autoRepeat?: AutoRepeatConfig | null,
): Promise<StartSessionResponse> {
	const effectiveAutoRepeat = normalizeAutoRepeat(autoRepeat);
	const plannedSession = await resolvePlannedSessionData(nextSessionId, config);
	const effectiveConfig =
		plannedSession?.config ?? normalizeSessionConfig(config);

	if (currentSession) {
		clearSessionTimers(currentSession);
	}

	if (pendingAutoRepeat) {
		pendingAutoRepeat.cancelled = true;
		if (pendingAutoRepeat.tickId != null) {
			window.clearTimeout(pendingAutoRepeat.tickId);
		}
		pendingAutoRepeat = null;
	}

	const sessionId = nextSessionId;
	nextSessionId += 1;

	const session: BrowserSession = {
		sessionId,
		config: effectiveConfig,
		autoRepeat: effectiveAutoRepeat,
		plannedNumbers: plannedSession?.numbers ?? null,
		plannedSum: plannedSession?.sum ?? null,
		numbers: [],
		sum: 0,
		runningSum: 0,
		lastPayload: null,
		completed: false,
		timers: [],
	};

	currentSession = session;
	emitClearScreen(sessionId, null);

	let timelineMs = 0;
	for (const value of COUNTDOWN_TICKS) {
		queueTimer(session, timelineMs, () => {
			emit(listeners.countdownTick, String(value));
		});
		timelineMs += 1000;
	}

	const numberDurationMs = toMs(effectiveConfig.number_duration_s);
	const gapDurationMs = toMs(effectiveConfig.delay_between_numbers_s);
	let currentAtMs = timelineMs;

	for (let index = 0; index < effectiveConfig.total_numbers; index += 1) {
		const plannedValue = session.plannedNumbers?.[index];
		const generated =
			plannedValue === undefined
				? generateNumber(
						effectiveConfig.digits_per_number,
						effectiveConfig.allow_negative_numbers,
						index,
						session.runningSum,
						session.lastPayload,
					)
				: { payload: String(plannedValue), value: plannedValue };
		const { payload, value } = generated;

		const showAt = currentAtMs;
		const clearAt =
			showAt + numberDurationMs + (index === 0 ? FIRST_FLASH_GRACE_MS : 0);

		queueTimer(session, showAt, () => {
			session.lastPayload = payload;
			session.runningSum = Math.max(0, session.runningSum + value);
			session.numbers.push(value);
			session.sum += value;

			emit(listeners.showNumber, {
				session_id: sessionId,
				index: index + 1,
				total: effectiveConfig.total_numbers,
				value,
				running_sum: session.runningSum,
				emitted_at_ms: nowMs(),
			});
			playAudio("beep");
		});

		queueTimer(session, clearAt, () => {
			emitClearScreen(sessionId, index + 1);
		});

		currentAtMs = clearAt + gapDurationMs;
	}

	queueTimer(session, currentAtMs, () => {
		finishSession(session);
	});

	return {
		session_id: sessionId,
		effective_config: effectiveConfig,
		effective_auto_repeat: effectiveAutoRepeat,
	};
}

function parseProvidedAnswerText(value: string): number {
	const normalized = value.replace(/[\s,]+/g, "").trim();
	if (!normalized) {
		throw new Error("Answer is required");
	}

	const parsed = Number(normalized);
	if (!Number.isFinite(parsed)) {
		throw new Error("Answer must be a number");
	}

	return Math.trunc(parsed);
}

function validateAnswer(
	sessionId: number,
	provided: number,
): SubmitAnswerResponse {
	if (
		!currentSession ||
		currentSession.sessionId !== sessionId ||
		!currentSession.completed
	) {
		throw new Error("No completed session available to validate");
	}

	const expected = currentSession.sum;
	const delta = provided - expected;
	const validation = {
		expected_sum: expected,
		provided_sum: provided,
		correct: delta === 0,
		delta,
	};

	return {
		validation,
		auto_repeat_waiting: armAutoRepeatForSession(currentSession.sessionId),
	};
}

export const browserRuntime: Runtime = {
	async ping(): Promise<string> {
		return "pong (browser)";
	},

	async getAppSettings(): Promise<AppSettings> {
		return { ...appSettings };
	},

	async setColorScheme(scheme: ColorScheme): Promise<AppSettings> {
		appSettings = { ...appSettings, color_scheme: scheme };
		persistAppSettings();
		emit(listeners.appSettingsChanged, { ...appSettings });
		return { ...appSettings };
	},

	async setThemeMode(mode: ThemeMode): Promise<AppSettings> {
		appSettings = { ...appSettings, theme_mode: mode };
		persistAppSettings();
		emit(listeners.appSettingsChanged, { ...appSettings });
		return { ...appSettings };
	},

	startSession: startSessionImpl,

	async stopSession(): Promise<void> {
		if (currentSession) {
			clearSessionTimers(currentSession);
			emitClearScreen(currentSession.sessionId, null);
		}

		if (pendingAutoRepeat) {
			pendingAutoRepeat.cancelled = true;
			if (pendingAutoRepeat.tickId != null) {
				window.clearTimeout(pendingAutoRepeat.tickId);
			}
			pendingAutoRepeat = null;
		}

		currentSession = null;
	},

	async cancelAutoRepeat(): Promise<void> {
		if (!pendingAutoRepeat) {
			return;
		}

		pendingAutoRepeat.cancelled = true;
		if (pendingAutoRepeat.tickId != null) {
			window.clearTimeout(pendingAutoRepeat.tickId);
		}
		pendingAutoRepeat = null;
	},

	async markValidated(
		sessionId: number,
	): Promise<AutoRepeatWaitingPayload | null> {
		return armAutoRepeatForSession(sessionId);
	},

	async acknowledgeComplete(
		sessionId: number,
	): Promise<AutoRepeatWaitingPayload | null> {
		return armAutoRepeatForSession(sessionId);
	},

	async submitAnswer(
		sessionId: number,
		providedSum: number,
	): Promise<SubmitAnswerResponse> {
		if (!currentSession || currentSession.sessionId !== sessionId) {
			throw new Error("No matching session available to validate");
		}

		return validateAnswer(sessionId, providedSum);
	},

	async submitAnswerText(
		sessionId: number,
		providedText: string,
	): Promise<SubmitAnswerResponse> {
		return validateAnswer(sessionId, parseProvidedAnswerText(providedText));
	},

	async getSoundEnabled(): Promise<boolean> {
		return soundEnabled;
	},

	async setSoundEnabled(enabled: boolean): Promise<void> {
		soundEnabled = Boolean(enabled);
		persistSoundEnabled();
	},

	async playSound(kind: "beep" | "applause" | "buzzer"): Promise<void> {
		if (!soundEnabled) {
			return;
		}

		const clip = audio[kind];
		try {
			clip.currentTime = 0;
			void clip.play();
		} catch {
			// Best-effort only.
		}
	},

	async onCountdownTick(handler: (value: string) => void): Promise<UnlistenFn> {
		return addListener(listeners.countdownTick, handler);
	},

	async onShowNumber(
		handler: (payload: ShowNumber) => void,
	): Promise<UnlistenFn> {
		return addListener(listeners.showNumber, handler);
	},

	async onClearScreen(
		handler: (payload: ClearScreen) => void,
	): Promise<UnlistenFn> {
		return addListener(listeners.clearScreen, handler);
	},

	async onAutoRepeatWaiting(
		handler: (payload: AutoRepeatWaitingPayload) => void,
	): Promise<UnlistenFn> {
		return addListener(listeners.autoRepeatWaiting, handler);
	},

	async onAutoRepeatTick(
		handler: (payload: AutoRepeatTickPayload) => void,
	): Promise<UnlistenFn> {
		return addListener(listeners.autoRepeatTick, handler);
	},

	async onAppSettingsChanged(
		handler: (payload: AppSettings) => void,
	): Promise<UnlistenFn> {
		return addListener(listeners.appSettingsChanged, handler);
	},

	async onSessionComplete(
		handler: (payload: SessionComplete) => void,
	): Promise<UnlistenFn> {
		return addListener(listeners.sessionComplete, handler);
	},
};
