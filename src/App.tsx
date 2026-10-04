import {
	createEffect,
	createMemo,
	createSignal,
	For,
	onCleanup,
	onMount,
	Show,
} from "solid-js";
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
		// Ignore: some platforms or browser policies may refuse fullscreen.
	}
}

/**
 * Best-effort fullscreen entry before a session starts.
 *
 * Never throws and never blocks: the Fullscreen API is simply absent on some
 * targets (iOS Safari has no element `requestFullscreen`, as do cross-origin
 * iframes and some WebViews) and can be refused by policy. Treating a refusal
 * as fatal left the Start button permanently dead there. The countdown tick
 * retries the request, which is the mechanism that actually matters.
 */
async function requestFullscreenBestEffort(): Promise<void> {
	if (typeof document === "undefined") {
		return;
	}

	if (document.fullscreenElement) {
		return;
	}

	if (typeof document.documentElement.requestFullscreen !== "function") {
		return;
	}

	try {
		await document.documentElement.requestFullscreen();
	} catch {
		// Best-effort: the countdown retries, and the session runs windowed.
	}
}

/**
 * Auto-repeat status strip: countdown label, progress ramp and cancel.
 *
 * Rendered in both answer modes, so it lives here rather than being duplicated:
 * the ramp's duration has to be identical in both places or one of the bars
 * animates differently.
 */
// Exported for its unit test; the app itself only renders it internally.
export function AutoRepeatBar(props: {
	secondsLeft: number;
	remaining: number;
	fillMs: number;
	onCancel: () => void;
}) {
	return (
		<div class="autoRepeatBar">
			<div class="autoRepeatStatus">
				<div class="autoRepeatStatusText">
					Next question in {props.secondsLeft}s · {props.remaining + 1}{" "}
					remaining
				</div>
				<div class="autoRepeatProgressBar">
					<div
						class="autoRepeatProgressFill"
						style={{ "animation-duration": `${props.fillMs}ms` }}
					/>
				</div>
				<button class="autoRepeatCancel" type="button" onClick={props.onCancel}>
					Cancel auto-repeat
				</button>
			</div>
		</div>
	);
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
	 * Counts in the countdown, mirroring `COUNTDOWN_FROM` in
	 * `crate::core::engine`.
	 */
	const COUNTDOWN_FROM = 3;

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
			} else if (status.available) {
				setSoundStatusText("Sound ready");
			} else {
				setSoundStatusText(`Sound unavailable (${status.detail})`);
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
	let overlayRef: HTMLDivElement | undefined;

	const isRunning = (): boolean =>
		phase() === "starting" || phase() === "flashing";

	const resetForIncomingSessionIfComplete = () => {
		if (phase() !== "complete") return;
		setShowAnswer(false);
		setAnswerSum(0);
		setTypedAnswer("");
		setValidationSummary("");
		setShowNumbersList(false);
		setHasValidated(false);
		setAutoRepeatSecondsLeft(null);
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

	createEffect(() => {
		if (!showAdvanced() || !overlayRef) return;

		const panel = overlayRef.querySelector<HTMLElement>(".advancedPanel");
		if (!panel) return;

		const focusable = panel.querySelectorAll<HTMLElement>(
			'button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])',
		);
		if (focusable.length === 0) return;

		const first = focusable[0];
		const last = focusable[focusable.length - 1];
		first.focus();

		const handler = (e: KeyboardEvent) => {
			if (e.key !== "Tab") return;
			if (e.shiftKey && document.activeElement === first) {
				e.preventDefault();
				last.focus();
			} else if (!e.shiftKey && document.activeElement === last) {
				e.preventDefault();
				first.focus();
			}
		};

		panel.addEventListener("keydown", handler);
		onCleanup(() => panel.removeEventListener("keydown", handler));
	});

	const applySubmitAnswerResponse = (resp: SubmitAnswerResponse) => {
		const { validation } = resp;
		const ok = validation.correct;

		const lines = [
			ok ? "Correct ✅" : "Incorrect",
			`Expected answer: ${validation.expected_sum}`,
		];

		if (!ok) {
			const d = validation.delta;
			lines.push(`Difference: ${d > 0 ? "+" : ""}${d}`);
		}

		setValidationSummary(lines.join("\n"));

		if (resp.auto_repeat_waiting) {
			applyAutoRepeatWaiting(resp.auto_repeat_waiting);
		}
	};

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
			setThemeMode(settings.theme_mode ?? "dark");
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

		const unlistenCountdown = await runtime.onCountdownTick((value) => {
			resetForIncomingSessionIfComplete();
			setPhase("countdown");
			setDisplayText(value);
			setCurrentShown(null);
			// Retry fullscreen at the start of the countdown, away from the first flash
			// paint: the transition must not steal time from that paint. This is
			// the real mechanism now that starting no longer depends on it.
			if (value === "3") void setFullscreen(true);
		});

		const unlistenFlash = await runtime.onShowNumber((payload) => {
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
		});

		const unlistenClear = await runtime.onClearScreen((payload) => {
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
		});

		const unlistenAutoRepeatWaiting = await runtime.onAutoRepeatWaiting(
			(payload) => {
				if (!autoRepeatEnabled()) return;
				applyAutoRepeatWaiting(payload);
			},
		);

		const unlistenAutoRepeatTick = await runtime.onAutoRepeatTick((payload) => {
			if (!autoRepeatEnabled()) return;
			setAutoRepeatRemaining(payload.remaining);
			setAutoRepeatSecondsLeft(payload.seconds_left);
		});

		const unlistenSettings = await runtime.onAppSettingsChanged((payload) => {
			setColorScheme(payload.color_scheme);
			// payload.theme_mode may be 'dark' | 'light'
			// ensure UI reflects server-side change
			setThemeMode(payload.theme_mode === "light" ? "light" : "dark");
		});

		const unlistenComplete = await runtime.onSessionComplete((payload) => {
			setPhase("complete");
			setDisplayText("");
			setCurrentShown(null);
			setShowAnswer(false);
			setNumbers(payload.numbers);
			setAnswerSum(payload.sum);
			setTypedAnswer("");
			setValidationSummary("");
			setShowNumbersList(false);
			setHasValidated(false);
			setAutoRepeatSecondsLeft(null);
			setSessionId(payload.session_id);
			void setFullscreen(false);
		});

		const onKeyDown = (e: KeyboardEvent) => {
			if (!isEscapeKey(e)) return;
			if (
				phase() !== "flashing" &&
				phase() !== "starting" &&
				phase() !== "countdown"
			)
				return;
			void stop();
		};

		window.addEventListener("keydown", onKeyDown);

		onCleanup(() => {
			window.removeEventListener("keydown", onKeyDown);
			unlistenCountdown();
			unlistenFlash();
			unlistenClear();
			unlistenAutoRepeatWaiting();
			unlistenAutoRepeatTick();
			unlistenSettings();
			unlistenComplete();
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

		setShowAnswer(false);
		setAnswerSum(0);
		setTypedAnswer("");
		setValidationSummary("");
		setShowNumbersList(false);
		setHasValidated(false);

		setAutoRepeatSecondsLeft(null);

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

			await requestFullscreenBestEffort();
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
			applyStartSessionResponse(resp);
		} catch (e) {
			setPhase("idle");
			void setFullscreen(false);
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
			setShowAnswer(false);
			setAnswerSum(0);
			setTypedAnswer("");
			setValidationSummary("");
			setShowNumbersList(false);
			setHasValidated(false);
			setAutoRepeatRemaining(0);
			setAutoRepeatSecondsLeft(null);
			setSessionId(null);
			setNumbers([]);
			void setFullscreen(false);
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
							<div class="advancedOverlay" ref={overlayRef}>
								<button
									class="advancedOverlayDismiss"
									type="button"
									tabIndex={-1}
									aria-label="Close additional settings"
									onClick={() => setShowAdvanced(false)}
								/>
								<div class="advancedPanel">
									<div class="advancedSectionTitle">Additional settings</div>

									<div class="advancedSetting">
										<div class="settingRow">
											<div class="label">Auto-repeat</div>
											<div
												class="segmented"
												role="radiogroup"
												aria-label="Auto-repeat"
											>
												<label class="segmentedOption">
													<input
														class="segmentedInput"
														type="radio"
														name="auto-repeat-enabled"
														value="off"
														disabled={isRunning()}
														checked={!autoRepeatEnabled()}
														onInput={() => {
															setAutoRepeatEnabled(false);
															setAutoRepeatRemaining(0);
															setAutoRepeatSecondsLeft(null);
															void runtime.cancelAutoRepeat();
														}}
													/>
													<span class="segmentedLabel">Off</span>
												</label>
												<label class="segmentedOption">
													<input
														class="segmentedInput"
														type="radio"
														name="auto-repeat-enabled"
														value="on"
														disabled={isRunning()}
														checked={autoRepeatEnabled()}
														onInput={() => setAutoRepeatEnabled(true)}
													/>
													<span class="segmentedLabel">On</span>
												</label>
											</div>
										</div>

										{autoRepeatEnabled() ? (
											<div class="field">
												<div class="fieldRow">
													<div class="label">Repeats</div>
													<input
														class="input"
														type="number"
														min="1"
														max="20"
														step="1"
														value={autoRepeatCount()}
														disabled={isRunning()}
														onInput={(e) =>
															setAutoRepeatCount(
																Number.isFinite(e.currentTarget.valueAsNumber)
																	? e.currentTarget.valueAsNumber
																	: 1,
															)
														}
													/>
												</div>
												<input
													class="range"
													type="range"
													min="1"
													max="20"
													step="1"
													value={autoRepeatCount()}
													disabled={isRunning()}
													onInput={(e) =>
														setAutoRepeatCount(
															Number.isFinite(e.currentTarget.valueAsNumber)
																? e.currentTarget.valueAsNumber
																: 1,
														)
													}
												/>
												{autoRepeatCount() < 1 ? (
													<div class="validationWarning">
														Repeats must be at least 1
													</div>
												) : null}
											</div>
										) : null}

										{autoRepeatEnabled() ? (
											<>
												<div class="fieldRow">
													<div class="label">
														Delay before next question (s)
													</div>
													<input
														class="input"
														type="number"
														min="5"
														max="120"
														step="1"
														value={autoRepeatDelaySeconds()}
														disabled={isRunning()}
														onInput={(e) =>
															setAutoRepeatDelaySeconds(
																Number.isFinite(e.currentTarget.valueAsNumber)
																	? e.currentTarget.valueAsNumber
																	: 5,
															)
														}
													/>
												</div>
												<div class="hint">Starts after you validate.</div>
											</>
										) : null}
									</div>

									<div class="advancedSetting">
										<div class="settingRow">
											<div class="label">Answer mode</div>
											<div
												class="segmented"
												role="radiogroup"
												aria-label="Answer mode"
											>
												<label class="segmentedOption">
													<input
														class="segmentedInput"
														type="radio"
														name="answer-mode"
														value="reveal"
														disabled={isRunning()}
														checked={answerMode() === "reveal"}
														onInput={() => setAnswerMode("reveal")}
													/>
													<span class="segmentedLabel">Click</span>
												</label>
												<label class="segmentedOption">
													<input
														class="segmentedInput"
														type="radio"
														name="answer-mode"
														value="type"
														disabled={isRunning()}
														checked={answerMode() === "type"}
														onInput={() => setAnswerMode("type")}
													/>
													<span class="segmentedLabel">Type</span>
												</label>
											</div>
										</div>
									</div>

									<div class="advancedSetting">
										<div class="settingRow">
											<div class="label">
												Color
												<div
													class="modeVertical"
													role="radiogroup"
													aria-label="Theme mode"
												>
													<label class="segmentedOption">
														<input
															class="segmentedInput"
															type="radio"
															name="theme-mode"
															value="dark"
															disabled={isRunning()}
															checked={themeMode() === "dark"}
															onInput={() => {
																setThemeMode("dark");
																void runtime.setThemeMode("dark");
															}}
														/>
														<span class="segmentedLabel">Dark</span>
													</label>
													<label class="segmentedOption">
														<input
															class="segmentedInput"
															type="radio"
															name="theme-mode"
															value="light"
															disabled={isRunning()}
															checked={themeMode() === "light"}
															onInput={() => {
																setThemeMode("light");
																void runtime.setThemeMode("light");
															}}
														/>
														<span class="segmentedLabel">Light</span>
													</label>
												</div>
											</div>
											<div
												class="colorGrid"
												role="radiogroup"
												aria-label="Color"
											>
												{(() => {
													const values = [
														"midnight",
														"crimson",
														"aqua",
														"violet",
														"amber",
														"ivory",
													] as const;
													const darkNames: Record<string, string> = {
														midnight: "Midnight",
														crimson: "Crimson",
														aqua: "Aqua",
														violet: "Violet",
														amber: "Amber",
														ivory: "Obsidian",
													};
													const lightNames: Record<string, string> = {
														midnight: "Dawn",
														crimson: "Blush",
														aqua: "Sea Glass",
														violet: "Lilac",
														amber: "Saffron",
														ivory: "Ivory",
													};

													return values.map((value) => {
														const label =
															themeMode() === "light"
																? lightNames[value]
																: darkNames[value];
														return (
															<label class="colorOption" title={label}>
																<input
																	class="segmentedInput"
																	type="radio"
																	name="color-scheme"
																	value={value}
																	disabled={isRunning()}
																	checked={colorScheme() === value}
																	onInput={() => {
																		setColorScheme(value);
																		void runtime.setColorScheme(value);
																	}}
																/>
																<span
																	classList={{
																		colorSwatch: true,
																		[`sw-preview-${value}`]: true,
																	}}
																	aria-hidden="true"
																/>
																<span class="colorLabel">{label}</span>
															</label>
														);
													});
												})()}
											</div>
										</div>
									</div>

									<div class="advancedSetting">
										<div class="settingRow">
											<div class="label">Allow negative numbers</div>
											<div
												class="segmented"
												role="radiogroup"
												aria-label="Allow negative numbers"
											>
												<label class="segmentedOption">
													<input
														class="segmentedInput"
														type="radio"
														name="allow-negative-numbers"
														value="off"
														disabled={isRunning()}
														checked={!allowNegativeNumbers()}
														onInput={() => setAllowNegativeNumbers(false)}
													/>
													<span class="segmentedLabel">Off</span>
												</label>
												<label class="segmentedOption">
													<input
														class="segmentedInput"
														type="radio"
														name="allow-negative-numbers"
														value="on"
														disabled={isRunning()}
														checked={allowNegativeNumbers()}
														onInput={() => setAllowNegativeNumbers(true)}
													/>
													<span class="segmentedLabel">On</span>
												</label>
											</div>
										</div>
									</div>

									<div class="advancedDivider" />

									<div class="actions">
										<button
											class="button"
											type="button"
											onClick={() => setShowAdvanced(false)}
											disabled={isRunning()}
										>
											Close
										</button>
									</div>
								</div>
							</div>
						) : null}

						<div class="grid">
							<div class="field">
								<div class="fieldRow">
									<div class="label">Digits per number</div>
									<input
										class="input"
										type="number"
										min="1"
										max="15"
										step="1"
										value={digitsPerNumber()}
										disabled={isRunning()}
										onInput={(e) =>
											setDigitsPerNumber(
												Number.isFinite(e.currentTarget.valueAsNumber)
													? e.currentTarget.valueAsNumber
													: 1,
											)
										}
									/>
								</div>
								<input
									class="range"
									type="range"
									min="1"
									max="15"
									step="1"
									value={digitsPerNumber()}
									disabled={isRunning()}
									onInput={(e) =>
										setDigitsPerNumber(
											Number.isFinite(e.currentTarget.valueAsNumber)
												? e.currentTarget.valueAsNumber
												: 1,
										)
									}
								/>
							</div>

							<div class="field">
								<div class="fieldRow">
									<div class="label">Duration per number (s)</div>
									<input
										class="input"
										type="number"
										min="0.1"
										max="5"
										step="0.1"
										value={numberDurationSeconds()}
										disabled={isRunning()}
										onInput={(e) =>
											setNumberDurationSeconds(
												Number.isFinite(e.currentTarget.valueAsNumber)
													? e.currentTarget.valueAsNumber
													: 0.1,
											)
										}
									/>
								</div>
								<input
									class="range"
									type="range"
									min="0.1"
									max="5"
									step="0.1"
									value={numberDurationSeconds()}
									disabled={isRunning()}
									onInput={(e) =>
										setNumberDurationSeconds(
											Number.isFinite(e.currentTarget.valueAsNumber)
												? e.currentTarget.valueAsNumber
												: 0.1,
										)
									}
								/>
							</div>

							<div class="field">
								<div class="fieldRow">
									<div class="label">Total numbers</div>
									<input
										class="input"
										type="number"
										min="1"
										max="1500"
										step="1"
										value={totalNumbers()}
										disabled={isRunning()}
										onInput={(e) =>
											setTotalNumbers(
												Number.isFinite(e.currentTarget.valueAsNumber)
													? e.currentTarget.valueAsNumber
													: 1,
											)
										}
									/>
								</div>
								<input
									class="range"
									type="range"
									min="1"
									max="1500"
									step="1"
									value={totalNumbers()}
									disabled={isRunning()}
									onInput={(e) =>
										setTotalNumbers(
											Number.isFinite(e.currentTarget.valueAsNumber)
												? e.currentTarget.valueAsNumber
												: 1,
										)
									}
								/>
							</div>
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
						{answerMode() === "reveal" ? (
							<div class="answerCard">
								<div class="endHeaderCenter">
									<div class="endTitle" role="status">
										Session complete
									</div>
									<div class="endSub">Click to see answer</div>
								</div>

								<div class="endBody">
									{!showAnswer() ? (
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

											{hasValidated() && showNumbersList() ? (
												<div class="answerNumbers">
													<For each={numbers()}>
														{(n, idx) => (
															<div class="answerRow">
																<div class="answerIndex">{idx() + 1}</div>
																<div class="answerValue">{n}</div>
															</div>
														)}
													</For>
												</div>
											) : null}
										</>
									)}
								</div>

								{autoRepeatEnabled() &&
								hasValidated() &&
								autoRepeatSecondsLeft() != null ? (
									<AutoRepeatBar
										secondsLeft={autoRepeatSecondsLeft() ?? 0}
										remaining={autoRepeatRemaining()}
										fillMs={autoRepeatFillMs()}
										onCancel={cancelAutoRepeat}
									/>
								) : null}

								<div class="endFooter">
									<div class="endFooterInner">
										<div class="actionField">
											<div class="centerActions">
												{showAnswer() ? (
													<button
														class="button"
														type="button"
														onClick={() => setShowNumbersList((v) => !v)}
													>
														{showNumbersList()
															? "Hide numbers"
															: "Show numbers"}
													</button>
												) : null}
												<button
													class="button"
													type="button"
													onClick={() => void stop()}
												>
													Home
												</button>
											</div>
										</div>
									</div>
								</div>
							</div>
						) : (
							<div class="answerCard">
								<div class="endHeaderCenter">
									<div class="endTitle" role="status">
										Session complete
									</div>
									<div class="endSub">Type your answer</div>
								</div>

								<div class="endBody">
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

									{hasValidated() && showNumbersList() ? (
										<div class="answerNumbers">
											<For each={numbers()}>
												{(n, idx) => (
													<div class="answerRow">
														<div class="answerIndex">{idx() + 1}</div>
														<div class="answerValue">{n}</div>
													</div>
												)}
											</For>
										</div>
									) : null}
								</div>

								{autoRepeatEnabled() &&
								hasValidated() &&
								autoRepeatSecondsLeft() != null ? (
									<AutoRepeatBar
										secondsLeft={autoRepeatSecondsLeft() ?? 0}
										remaining={autoRepeatRemaining()}
										fillMs={autoRepeatFillMs()}
										onCancel={cancelAutoRepeat}
									/>
								) : null}

								<div class="endFooter">
									<div class="endFooterInner">
										<div class="actionField">
											<div class="centerActions">
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
														{showNumbersList()
															? "Hide numbers"
															: "Show numbers"}
													</button>
												) : null}
												<button
													class="button"
													type="button"
													onClick={() => void stop()}
												>
													Home
												</button>
											</div>
										</div>
									</div>
								</div>
							</div>
						)}
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
								<span class="magnitude">
									{displayText().startsWith("-")
										? displayText().slice(1)
										: displayText()}
								</span>
								{allowNegativeNumbers() ? (
									<span
										class="signOverlay"
										aria-hidden={!displayText().startsWith("-")}
										style={{ opacity: displayText().startsWith("-") ? 1 : 0 }}
									>
										-
									</span>
								) : null}
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
