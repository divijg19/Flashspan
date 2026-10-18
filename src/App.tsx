import {
	createEffect,
	createMemo,
	createSignal,
	onCleanup,
	onMount,
	Show,
} from "solid-js";
import { AdvancedSettings } from "./components/AdvancedSettings";
import { AnswerNumbers } from "./components/AnswerNumbers";
import { NumberField } from "./components/NumberField";
import { SessionEndDetails } from "./components/SessionEndDetails";
import { COUNTDOWN_FROM } from "./timing";
import "./App.css";
import {
	type AutoRepeatWaitingPayload,
	type ColorScheme,
	type Phase,
	runtime,
	type SessionConfigInput,
	type StartSessionResponse,
	type SubmitAnswerResponse,
	type ThemeMode,
} from "./runtime";

function isEscapeKey(event: KeyboardEvent): boolean {
	return event.key === "Escape";
}

/**
 * Enter or leave fullscreen, best-effort.
 *
 * Never throws and never blocks a caller on a refusal: the Fullscreen API is
 * simply absent on some targets (iOS Safari has no element `requestFullscreen`,
 * as do cross-origin iframes and some WebViews) and can be refused by policy.
 * Treating a refusal as fatal left the Start button permanently dead there, so
 * a failure is swallowed and the session runs windowed; the countdown tick
 * retries the request, which is the mechanism that actually matters. An absent
 * `requestFullscreen` is not special-cased — calling it throws `TypeError`,
 * which the same handler catches.
 */
async function setFullscreen(enabled: boolean): Promise<void> {
	if (typeof document === "undefined") {
		return;
	}

	try {
		if (enabled) {
			if (!document.fullscreenElement) {
				await document.documentElement.requestFullscreen();
			}
		} else if (document.fullscreenElement) {
			await document.exitFullscreen();
		}
	} catch {
		// Best-effort: unsupported or refused. The countdown retries; the
		// session runs windowed.
	}
}

export default function App() {
	const [showSplash, setShowSplash] = createSignal<boolean>(true);
	const [splashVisible, setSplashVisible] = createSignal<boolean>(false);
	const [splashExiting, setSplashExiting] = createSignal<boolean>(false);

	const [colorScheme, setColorScheme] = createSignal<ColorScheme>("midnight");
	const [themeMode, setThemeMode] = createSignal<ThemeMode>("dark");
	const themeClass = () => `theme-${colorScheme()}`;
	const modeClass = () => `theme-${themeMode()}`;

	onMount(() => {
		if (!showSplash()) return;

		// The three splash timings are declared once in CSS (see `.splash`) and
		// read back here, so changing a value there cannot desynchronise the
		// schedule from the animation.
		//
		// Read them off the splash element rather than the document root: custom
		// properties inherit downwards, so a value declared on `.splash` is not
		// visible on `<html>`. Reading from there returned nothing and every
		// timing silently fell back to the constants below, which only looked
		// right while the two happened to agree.
		const splashElement = document.querySelector<HTMLElement>(".splash");
		const style = window.getComputedStyle(
			splashElement ?? document.documentElement,
		);
		const cssMs = (name: string, fallback: number): number => {
			const parsed = Number.parseFloat(style.getPropertyValue(name).trim());
			return Number.isFinite(parsed) && parsed > 0 ? parsed : fallback;
		};
		const enterMs = cssMs("--splash-enter", 3000);
		const holdMs = cssMs("--splash-hold", 1000);
		const exitMs = cssMs("--splash-exit", 3000);

		// Fade in on the next tick, so the transition has a start value to
		// animate from.
		const enterTimer = window.setTimeout(() => setSplashVisible(true), 10);

		// Fade out once the entrance and the hold are done. `exiting` swaps the
		// transition to the shorter exit duration without touching opacity.
		const exitTimer = window.setTimeout(() => {
			setSplashExiting(true);
			setSplashVisible(false);
		}, enterMs + holdMs);

		// Preferred unmount point: the fade has actually finished. See
		// `onSplashTransitionEnd`.
		const safetyTimer = window.setTimeout(
			() => setShowSplash(false),
			enterMs + holdMs + exitMs + 200,
		);

		const onKey = (event: KeyboardEvent) => {
			// Ignore auto-repeat: a held key used to land on the other branch
			// mid-fade and cut it off, which read as a flicker.
			if (!showSplash() || event.repeat) return;

			// Any key press dismisses the splash outright. Fading it out was the
			// source of the skip flicker: the overlay then sat on top of a live UI,
			// and dropping the `visible` class mid-fade made it click-through while
			// it was still visible. Unmounting in the same tick removes every one of
			// those windows, and needs no extra timer.
			window.clearTimeout(enterTimer);
			window.clearTimeout(exitTimer);
			window.clearTimeout(safetyTimer);
			setSplashVisible(false);
			setShowSplash(false);
		};

		window.addEventListener("keydown", onKey);

		onCleanup(() => {
			window.clearTimeout(enterTimer);
			window.clearTimeout(exitTimer);
			window.clearTimeout(safetyTimer);
			window.removeEventListener("keydown", onKey);
		});
	});

	/**
	 * Unmount once the opacity fade has genuinely finished, so the last frames
	 * of the exit can never be cut off. Ignores transitions on other properties,
	 * and the safety timer in the mount effect covers the case where the
	 * transition never runs at all.
	 */
	const onSplashTransitionEnd = (event: TransitionEvent): void => {
		if (event.propertyName === "opacity") {
			setShowSplash(false);
		}
	};

	const [displayText, setDisplayText] = createSignal<string>("");

	/**
	 * Which quarter turn the blades rest at: "3" is the first, "1" the third.
	 *
	 * Derived from the digit rather than a running tick counter so it is
	 * inherently per-session. Zero before the first digit shows, which leaves the
	 * blades at their unrotated base angle.
	 */
	const bladeStep = createMemo<number>(() => {
		const parsed = Number(displayText());
		return Number.isFinite(parsed) && parsed > 0
			? COUNTDOWN_FROM + 1 - parsed
			: 0;
	});
	const [currentShown, setCurrentShown] = createSignal<{
		session_id: number;
		index: number;
		emitted_at_ms: number;
	} | null>(null);
	const [phase, setPhase] = createSignal<Phase>("idle");
	const [errorText, setErrorText] = createSignal<string>("");

	// Countdown ticks only. The flashing numbers are aria-hidden, and the end
	// screen announces its own title, so this deliberately stays the one message
	// that would otherwise go unheard. It must not repeat text rendered
	// elsewhere, or duplicate nodes break `getByText` queries.
	const announcement = createMemo(() =>
		phase() === "countdown" ? `Starting in ${displayText()}` : "",
	);

	const [showAnswer, setShowAnswer] = createSignal<boolean>(false);
	const [answerMode, setAnswerMode] = createSignal<"reveal" | "type">("reveal");
	const [typedAnswer, setTypedAnswer] = createSignal<string>("");
	const [validationSummary, setValidationSummary] = createSignal<string>("");
	const [showNumbersList, setShowNumbersList] = createSignal<boolean>(false);
	const [hasValidated, setHasValidated] = createSignal<boolean>(false);
	const [answerSum, setAnswerSum] = createSignal<number>(0);

	const [sessionId, setSessionId] = createSignal<number | null>(null);
	const [numbers, setNumbers] = createSignal<number[]>([]);

	// Scale UI elements proportionally to window size relative to startup dimensions.
	onMount(() => {
		const root = document.documentElement;
		const baselineW = Math.max(800, window.innerWidth);
		const baselineH = Math.max(600, window.innerHeight);
		let raf = 0;

		const update = () => {
			const ratio = Math.min(
				window.innerWidth / baselineW,
				window.innerHeight / baselineH,
			);
			root.style.setProperty(
				"--ui-scale",
				`${Math.max(0.75, Math.min(ratio, 1.5))}`,
			);
		};

		const handler = () => {
			if (raf) cancelAnimationFrame(raf);
			raf = requestAnimationFrame(update);
		};

		update();
		window.addEventListener("resize", handler, { passive: true });

		onCleanup(() => {
			if (raf) cancelAnimationFrame(raf);
			window.removeEventListener("resize", handler);
		});
	});

	const [autoRepeatEnabled, setAutoRepeatEnabled] =
		createSignal<boolean>(false);
	const [autoRepeatCount, setAutoRepeatCount] = createSignal<number>(5);
	const [autoRepeatDelaySeconds, setAutoRepeatDelaySeconds] =
		createSignal<number>(5);
	const [autoRepeatRemaining, setAutoRepeatRemaining] = createSignal<number>(0);
	const [autoRepeatSecondsLeft, setAutoRepeatSecondsLeft] = createSignal<
		number | null
	>(null);
	/**
	 * Duration of the auto-repeat progress ramp, in milliseconds, captured once
	 * when the countdown arms. Deliberately not derived from `secondsLeft`: that
	 * changes every tick, and a changing `animation-duration` restarts the
	 * animation, which would turn the smooth ramp back into per-second steps.
	 */
	const [autoRepeatFillMs, setAutoRepeatFillMs] = createSignal(0);

	const [showAdvanced, setShowAdvanced] = createSignal<boolean>(false);
	const [soundEnabled, setSoundEnabled] = createSignal<boolean>(true);
	const [soundStatusText, setSoundStatusText] = createSignal<string>("");

	const refreshSoundStatus = async (): Promise<void> => {
		try {
			const status = await runtime.getAudioStatus();
			if (!status.enabled) {
				setSoundStatusText("");
			} else if (!status.available) {
				setSoundStatusText(`Sound unavailable (${status.detail})`);
			} else if (status.detail === "not-started") {
				// Nothing has been played yet, so nothing has been proven. This
				// branch is what used to claim "Sound ready" for the whole time
				// before the first session — which is to say, always, on a
				// machine with no output device. The slot stays reserved and
				// stays empty until playback has actually happened.
				setSoundStatusText("");
			} else {
				setSoundStatusText("Sound ready");
			}
		} catch {
			setSoundStatusText("");
		}
	};

	const [digitsPerNumber, setDigitsPerNumber] = createSignal<number>(1);
	const [numberDurationSeconds, setNumberDurationSeconds] =
		createSignal<number>(0.5);
	const [totalNumbers, setTotalNumbers] = createSignal<number>(5);

	const [allowNegativeNumbers, setAllowNegativeNumbers] =
		createSignal<boolean>(false);

	let sumInputRef: HTMLInputElement | undefined;

	const isRunning = (): boolean =>
		phase() === "starting" || phase() === "flashing";

	/**
	 * Clear everything the answer surface showed.
	 *
	 * Three callers need this exact set — the incoming-session reset, the Start
	 * path and the stop path — and they used to write the seven setters out
	 * separately each time, so adding a fifth piece of answer state (say, a
	 * per-number breakdown) meant remembering three places. The variants that
	 * follow are short and say what *they* add on top.
	 */
	const clearAnswerState = () => {
		setShowAnswer(false);
		setAnswerSum(0);
		setTypedAnswer("");
		setValidationSummary("");
		setShowNumbersList(false);
		setHasValidated(false);
		setAutoRepeatSecondsLeft(null);
	};

	/**
	 * Answer state is only cleared once a session is over.
	 *
	 * The guard matters: this runs from event handlers that fire *during* a
	 * session (a countdown tick arriving late for the previous session, say),
	 * and wiping the answer mid-flash would blank a reveal the user is reading.
	 */
	const resetForIncomingSessionIfComplete = () => {
		if (phase() !== "complete") return;
		clearAnswerState();
	};

	const applyAutoRepeatWaiting = (payload: AutoRepeatWaitingPayload) => {
		setAutoRepeatRemaining(payload.remaining);
		const remainingMs = Math.max(0, payload.next_start_at_ms - Date.now());
		// Set before the seconds-left value mounts the bar below, so the ramp
		// already has its final duration on first paint.
		setAutoRepeatFillMs(remainingMs);
		setAutoRepeatSecondsLeft(Math.ceil(remainingMs / 1000));
	};

	createEffect(() => {
		if (phase() !== "complete") return;
		if (answerMode() !== "type") return;
		requestAnimationFrame(() => sumInputRef?.focus?.());
	});

	/**
	 * The runtime owns the message text.
	 *
	 * `SubmitAnswerResponse.message` is formatted by the runtime that graded the
	 * answer — Rust on desktop, `formatValidationMessage` on the web — so the
	 * app renders it verbatim instead of rebuilding the same string from the
	 * numeric fields (which had to stay word-for-word in step with two other
	 * copies and silently did not belong to either runtime's tests).
	 */
	const applySubmitAnswerResponse = (resp: SubmitAnswerResponse) => {
		setValidationSummary(resp.message);

		if (resp.auto_repeat_waiting) {
			applyAutoRepeatWaiting(resp.auto_repeat_waiting);
		}
	};

	/** The revealed list only shows after a grading attempt, on request. */
	const numbersVisible = () => hasValidated() && showNumbersList();

	const validateTypedAnswer = async () => {
		if (hasValidated()) {
			return;
		}

		const sid = sessionId();
		if (sid == null) {
			setValidationSummary("No active session id to validate.");
			return;
		}

		try {
			const resp = await runtime.submitAnswerText(sid, typedAnswer());

			// Ensure session hasn't changed while awaiting validation result.
			if (sessionId() !== sid) {
				setValidationSummary(
					"Session changed before validation completed; result ignored.",
				);
				return;
			}

			setHasValidated(true);
			applySubmitAnswerResponse(resp);
		} catch (e) {
			setHasValidated(false);
			setValidationSummary(String(e));
		}
	};

	onMount(async () => {
		let lastFirstFlashSessionId: number | null = null;

		if (import.meta.env.DEV) {
			void runtime
				.ping()
				.then(console.log)
				.catch(() => {});
		}

		try {
			const settings = await runtime.getAppSettings();
			setColorScheme(settings.color_scheme);
			// Normalize exactly as the onAppSettingsChanged handler below does, so
			// both paths agree on how an out-of-contract value is handled instead of
			// one using `??` and the other an explicit compare.
			setThemeMode(settings.theme_mode === "light" ? "light" : "dark");
			try {
				const s = await runtime.getSoundEnabled();
				setSoundEnabled(s);
			} catch {
				// ignore: best-effort to sync backend sound flag
			}
			void refreshSoundStatus();
		} catch {
			// Best-effort.
		}

		// Subscribe to every session event at once. On desktop each subscription
		// is an IPC round-trip, so awaiting them in turn paid that latency
		// serially; `Promise.all` overlaps the registrations and yields a single
		// array to clean up. Order does not matter — a session only starts after
		// the user presses Start, long after this resolves.
		const unlisten = await Promise.all([
			runtime.onCountdownTick((value) => {
				resetForIncomingSessionIfComplete();
				setPhase("countdown");
				setDisplayText(value);
				setCurrentShown(null);
				// Retry fullscreen at the start of the countdown, away from the first flash
				// paint: the transition must not steal time from that paint. This is
				// the real mechanism now that starting no longer depends on it.
				if (value === "3") void setFullscreen(true);
			}),

			runtime.onShowNumber((payload) => {
				resetForIncomingSessionIfComplete();
				setSessionId(payload.session_id);
				setPhase("flashing");
				setDisplayText(String(payload.value));
				setCurrentShown({
					session_id: payload.session_id,
					index: payload.index,
					emitted_at_ms: payload.emitted_at_ms,
				});

				// Debug-only: measure backend->UI delivery latency for first flash.
				if (
					import.meta.env.DEV &&
					payload.index === 1 &&
					lastFirstFlashSessionId !== payload.session_id
				) {
					lastFirstFlashSessionId = payload.session_id;
					const deltaMs = Date.now() - payload.emitted_at_ms;
					console.debug(`[flashspan] first flash latency: ${deltaMs}ms`);
				}
			}),

			runtime.onClearScreen((payload) => {
				const activeSessionId = sessionId();
				if (activeSessionId == null) return;
				if (payload.session_id !== activeSessionId) return;

				const cur = currentShown();
				if (cur && payload.emitted_at_ms < cur.emitted_at_ms) return;

				if (payload.index == null) {
					setDisplayText("");
					setCurrentShown(null);
					return;
				}

				if (cur && cur.index === payload.index) {
					setDisplayText("");
					setCurrentShown(null);
				}
			}),

			runtime.onAutoRepeatWaiting((payload) => {
				if (!autoRepeatEnabled()) return;
				applyAutoRepeatWaiting(payload);
			}),

			runtime.onAutoRepeatTick((payload) => {
				if (!autoRepeatEnabled()) return;
				setAutoRepeatRemaining(payload.remaining);
				setAutoRepeatSecondsLeft(payload.seconds_left);
			}),

			runtime.onAppSettingsChanged((payload) => {
				setColorScheme(payload.color_scheme);
				// payload.theme_mode may be 'dark' | 'light'
				// ensure UI reflects server-side change
				setThemeMode(payload.theme_mode === "light" ? "light" : "dark");
			}),

			runtime.onSessionInterrupted(() => {
				// The runtime has already torn the session down; report it rather than
				// leaving a screen that looks stuck. There is no result to grade.
				resetForIncomingSessionIfComplete();
				setPhase("idle");
				void refreshSoundStatus();
				setErrorText(
					"Session interrupted while the browser tab was in the background.",
				);
			}),

			runtime.onSessionComplete((payload) => {
				setPhase("complete");
				setDisplayText("");
				setCurrentShown(null);
				// Reset the shared answer surface, then lay this session's plan on
				// top: `clearAnswerState` zeroes `answerSum`, so the real total is
				// written after it.
				clearAnswerState();
				setNumbers(payload.numbers);
				setAnswerSum(payload.sum);
				setSessionId(payload.session_id);
				void setFullscreen(false);
			}),
		]);

		const onKeyDown = (e: KeyboardEvent) => {
			if (!isEscapeKey(e)) return;
			// Escape stops a live session. `isRunning` covers starting/flashing;
			// the countdown is the one active phase it does not, and Escape while
			// it runs cancels the pending start.
			if (!isRunning() && phase() !== "countdown") return;
			void stop();
		};

		window.addEventListener("keydown", onKeyDown);

		onCleanup(() => {
			window.removeEventListener("keydown", onKeyDown);
			for (const unsubscribe of unlisten) unsubscribe();
		});
	});

	const applyStartSessionResponse = (resp: StartSessionResponse) => {
		setSessionId(resp.session_id);
		setDigitsPerNumber(resp.effective_config.digits_per_number);
		setNumberDurationSeconds(resp.effective_config.number_duration_s);
		setTotalNumbers(resp.effective_config.total_numbers);
		setAllowNegativeNumbers(resp.effective_config.allow_negative_numbers);

		if (resp.effective_auto_repeat) {
			setAutoRepeatCount(resp.effective_auto_repeat.repeats);
			setAutoRepeatDelaySeconds(resp.effective_auto_repeat.delay_s);
		}
	};

	const start = async () => {
		setErrorText("");
		setShowAdvanced(false);

		clearAnswerState();
		setSessionId(null);
		setNumbers([]);

		const config: SessionConfigInput = {
			digits_per_number: Math.trunc(digitsPerNumber()),
			number_duration_s: numberDurationSeconds(),
			total_numbers: Math.trunc(totalNumbers()),
			allow_negative_numbers: allowNegativeNumbers(),
		};

		try {
			setPhase("starting");
			setDisplayText("");

			const autoRepeatEffective = autoRepeatEnabled() && autoRepeatCount() >= 1;
			setAutoRepeatRemaining(
				autoRepeatEffective ? Math.trunc(autoRepeatCount()) : 0,
			);

			await setFullscreen(true);
			const resp = await runtime.startSession(
				config,
				autoRepeatEffective
					? {
							enabled: true,
							repeats: Math.trunc(autoRepeatCount()),
							delay_s: autoRepeatDelaySeconds(),
						}
					: null,
			);
			// Null: the start was superseded by a stop while the plan was still
			// building. The stop has already restored idle, so there is nothing to
			// apply and no error worth showing for a cancelled start.
			if (resp) {
				applyStartSessionResponse(resp);
			}
		} catch (e) {
			setPhase("idle");
			void setFullscreen(false);
			// The runtime warms audio up as part of starting, so the device may
			// have just failed while this screen came back.
			void refreshSoundStatus();
			setErrorText(String(e));
		}
	};

	const cancelAutoRepeat = (): void => {
		setAutoRepeatEnabled(false);
		setAutoRepeatRemaining(0);
		setAutoRepeatSecondsLeft(null);
		setAutoRepeatFillMs(0);
		void runtime.cancelAutoRepeat();
	};

	const stop = async () => {
		try {
			await runtime.stopSession();
		} catch {
			// stopSession failures should not prevent state cleanup.
		} finally {
			setPhase("idle");
			setDisplayText("");
			setCurrentShown(null);
			clearAnswerState();
			setAutoRepeatRemaining(0);
			setSessionId(null);
			setNumbers([]);
			void setFullscreen(false);
			// Audio is only ever played during a session, so that is the only
			// window in which the runtime can learn the device is broken — and
			// this screen is where its answer would be shown. Without this the
			// indicator kept the "Sound ready" it showed before the session
			// started, however the session actually went.
			void refreshSoundStatus();
		}
	};

	return (
		<>
			{/*
			 * Phase announcements live here rather than on the number region:
			 * `aria-live` there meant every flashed number was announced, which
			 * falls arbitrarily behind on long sessions. This announces the
			 * transitions worth hearing, once each.
			 */}
			<div class="srOnly" aria-live="polite" aria-atomic="true">
				{announcement()}
			</div>
			<div
				classList={{
					app: true,
					[themeClass()]: true,
					[modeClass()]: true,
					home: phase() === "idle",
				}}
			>
				{showSplash() ? (
					<div
						classList={{
							splash: true,
							visible: splashVisible(),
							exiting: splashExiting(),
						}}
						onTransitionEnd={onSplashTransitionEnd}
					>
						{/*
						 * Decorative: `.splashTitle` below already names the app.
						 * The intrinsic size lets the browser reserve the correct
						 * box before the image decodes, which is what stopped the
						 * splash re-centring mid-fade. `alt=""` also stops the
						 * placeholder text flashing in its place.
						 */}
						<img
							src="/Ascent_Banner.png"
							alt=""
							class="splashBanner"
							width={1080}
							height={340}
						/>
						<div class="splashText">
							<div class="splashTitle">Ascent Abacus &amp; Brain Gym</div>
							<div class="splashSubtitle">
								Your one stop solution for IQ improvement
							</div>
						</div>
					</div>
				) : null}
				{errorText() ? (
					<div class="error" role="alert">
						{errorText()}
					</div>
				) : null}

				{phase() === "idle" ? (
					<div class="panel">
						<div class="title">Ascent Flash</div>
						<button
							class="iconButton"
							type="button"
							aria-label="Additional settings"
							aria-expanded={showAdvanced()}
							disabled={isRunning()}
							onClick={() => setShowAdvanced((v) => !v)}
						>
							⚙
						</button>

						{showAdvanced() ? (
							<AdvancedSettings
								close={() => setShowAdvanced(false)}
								disabled={isRunning()}
								autoRepeat={{
									enabled: autoRepeatEnabled,
									setEnabled: setAutoRepeatEnabled,
									count: autoRepeatCount,
									setCount: setAutoRepeatCount,
									delaySeconds: autoRepeatDelaySeconds,
									setDelaySeconds: setAutoRepeatDelaySeconds,
									cancel: cancelAutoRepeat,
								}}
								answerMode={answerMode}
								setAnswerMode={setAnswerMode}
								allowNegativeNumbers={allowNegativeNumbers}
								setAllowNegativeNumbers={setAllowNegativeNumbers}
								colorScheme={colorScheme}
								setColorScheme={setColorScheme}
								themeMode={themeMode}
								setThemeMode={setThemeMode}
								persistColorScheme={(value) => {
									void runtime.setColorScheme(value);
								}}
								persistThemeMode={(value) => {
									void runtime.setThemeMode(value);
								}}
							/>
						) : null}

						<div class="grid">
							<NumberField
								label="Digits per number"
								value={digitsPerNumber}
								setValue={setDigitsPerNumber}
								min={1}
								max={15}
								step={1}
								fallback={1}
								disabled={isRunning}
							/>
							<NumberField
								label="Duration per number (s)"
								value={numberDurationSeconds}
								setValue={setNumberDurationSeconds}
								min={0.1}
								max={5}
								step={0.1}
								fallback={0.1}
								disabled={isRunning}
							/>
							<NumberField
								label="Total numbers"
								value={totalNumbers}
								setValue={setTotalNumbers}
								min={1}
								max={1500}
								step={1}
								fallback={1}
								disabled={isRunning}
							/>
						</div>

						<div class="actions">
							<div class="soundGroup">
								<div class="soundLabelWrap">
									<div class="soundLabel">Sound</div>
									<div
										class="soundStatus"
										aria-live="polite"
										title={soundStatusText() || undefined}
									>
										{soundStatusText()}
									</div>
								</div>
								<div class="segmented" role="radiogroup" aria-label="Sound">
									<label class="segmentedOption">
										<input
											class="segmentedInput"
											type="radio"
											name="sound"
											value="on"
											checked={soundEnabled()}
											onInput={async () => {
												setSoundEnabled(true);
												try {
													await runtime.setSoundEnabled(true);
												} catch (e) {
													setSoundEnabled(false);
													setErrorText(String(e));
												}
												void refreshSoundStatus();
											}}
										/>
										<span class="segmentedLabel">🔊 On</span>
									</label>
									<label class="segmentedOption">
										<input
											class="segmentedInput"
											type="radio"
											name="sound"
											value="off"
											checked={!soundEnabled()}
											onInput={async () => {
												setSoundEnabled(false);
												try {
													await runtime.setSoundEnabled(false);
												} catch (e) {
													setSoundEnabled(true);
													setErrorText(String(e));
												}
												void refreshSoundStatus();
											}}
										/>
										<span class="segmentedLabel">Off</span>
									</label>
								</div>
							</div>

							<button
								class="button"
								type="button"
								disabled={isRunning()}
								onClick={start}
							>
								Start
							</button>
						</div>
					</div>
				) : phase() === "complete" ? (
					<div class="endScreen">
						<div class="answerCard">
							<div class="endHeaderCenter">
								<div class="endTitle" role="status">
									Session complete
								</div>
								<div class="endSub">
									{answerMode() === "reveal"
										? "Click to see answer"
										: "Type your answer"}
								</div>
							</div>

							<div class="endBody">
								{answerMode() === "reveal" ? (
									!showAnswer() ? (
										<div class="actionField">
											<button
												class="button"
												type="button"
												onClick={async () => {
													setHasValidated(true);
													setShowAnswer(true);
													const sid = sessionId();
													if (sid != null) {
														try {
															const waiting =
																await runtime.acknowledgeComplete(sid);
															if (waiting) applyAutoRepeatWaiting(waiting);
														} catch (e) {
															setErrorText(String(e));
														}
													}
												}}
											>
												Show answer
											</button>
										</div>
									) : (
										<>
											<div class="sumCard">
												<div class="sumLabel">Correct answer</div>
												<div class="sumValue">{answerSum()}</div>
											</div>

											<AnswerNumbers
												visible={numbersVisible}
												numbers={numbers}
											/>
										</>
									)
								) : (
									<>
										<input
											ref={(el) => (sumInputRef = el)}
											class="sumInput"
											type="text"
											inputmode="numeric"
											autocomplete="off"
											placeholder="Enter the answer"
											disabled={hasValidated()}
											value={typedAnswer()}
											onInput={(e) => {
												if (hasValidated()) return;
												setTypedAnswer(e.currentTarget.value);
											}}
											onKeyDown={(e) => {
												if (hasValidated()) return;
												if (e.key === "Enter") void validateTypedAnswer();
											}}
											spellcheck={false}
										/>

										{validationSummary() ? (
											<pre class="validationText" role="alert">
												{validationSummary()}
											</pre>
										) : null}

										<AnswerNumbers visible={numbersVisible} numbers={numbers} />
									</>
								)}
							</div>

							<SessionEndDetails
								autoRepeatEnabled={autoRepeatEnabled}
								hasValidated={hasValidated}
								autoRepeatSecondsLeft={autoRepeatSecondsLeft}
								autoRepeatRemaining={autoRepeatRemaining}
								autoRepeatFillMs={autoRepeatFillMs}
								cancelAutoRepeat={cancelAutoRepeat}
								goHome={() => void stop()}
								footerLead={() =>
									answerMode() === "reveal" ? (
										showAnswer() ? (
											<button
												class="button"
												type="button"
												onClick={() => setShowNumbersList((v) => !v)}
											>
												{showNumbersList() ? "Hide numbers" : "Show numbers"}
											</button>
										) : null
									) : (
										<>
											<button
												class="button"
												type="button"
												disabled={hasValidated()}
												onClick={() => void validateTypedAnswer()}
											>
												Validate
											</button>
											{hasValidated() && validationSummary() ? (
												<button
													class="button"
													type="button"
													onClick={() => setShowNumbersList((v) => !v)}
												>
													{showNumbersList() ? "Hide numbers" : "Show numbers"}
												</button>
											) : null}
										</>
									)
								}
							/>
						</div>
					</div>
				) : (
					<div
						aria-hidden="true"
						classList={{ number: true, countdown: phase() === "countdown" }}
						style={{
							"--len": Math.max(
								1,
								(displayText().startsWith("-")
									? displayText().slice(1)
									: displayText()
								).length + (allowNegativeNumbers() ? 1 : 0),
							),
						}}
					>
						<Show when={phase() === "countdown"}>
							{/*
							 * Static halo, kept out of the rotating element so the
							 * countdown never re-rasterizes a filter while it animates.
							 */}
							<div class="countdownGlow" aria-hidden="true" />

							{/*
							 * One angle per count, held until the next digit
							 * changes: the blades turn once and then sit
							 * still, rather than sweeping through angles for
							 * the whole second.
							 */}
							<div
								classList={{
									countdownShutter: true,
									bladeQuarter: bladeStep() === 1,
									bladeHalf: bladeStep() === 2,
									bladeThreeQuarter: bladeStep() === 3,
								}}
								aria-hidden="true"
							/>

							<svg
								class="countdownRing"
								viewBox="0 0 100 100"
								aria-hidden="true"
							>
								<circle class="countdownTrack" cx="50" cy="50" r="44" />
								{/*
								 * No inline offset and no restart classes: the drain is a
								 * single CSS animation whose duration is the whole
								 * countdown, so the ring runs itself from "3" to empty
								 * and stays in step with the digits.
								 */}
								<circle class="countdownProgress" cx="50" cy="50" r="44" />
							</svg>
						</Show>

						{phase() === "countdown" ? (
							<span class="countdownDigit">{displayText()}</span>
						) : (
							<span class="signedNumber">
								{/*
								 * The sign precedes the digits in the DOM as well as on
								 * screen: matching the two avoids an `order` rule in
								 * CSS purely to undo the markup's order.
								 */}
								{allowNegativeNumbers() ? (
									<span
										class="signOverlay"
										aria-hidden={!displayText().startsWith("-")}
										style={{ opacity: displayText().startsWith("-") ? 1 : 0 }}
									>
										-
									</span>
								) : null}
								<span class="magnitude">
									{displayText().startsWith("-")
										? displayText().slice(1)
										: displayText()}
								</span>
							</span>
						)}
					</div>
				)}
			</div>
			{/* Bottom-left lodged logo */}
			<img src="/Ascent_Logo.png" alt="Ascent logo" class="topLogo" />

			{/* Bottom-right info and certification links */}
			<div class="bottomInfo">
				<a
					href="https://www.ascentabacus.com"
					target="_blank"
					rel="noopener noreferrer"
				>
					www.ascentabacus.com
				</a>
				<div class="isoRow">
					<span class="iso">ISO 9001</span>
					<span class="iso">ISO 14001</span>
				</div>
			</div>
		</>
	);
}
