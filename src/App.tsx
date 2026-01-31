import { getCurrentWindow } from "@tauri-apps/api/window";
import { Show, For, createEffect, createSignal, onCleanup, onMount } from "solid-js";
import "./App.css";
import {
  AutoRepeatWaitingPayload,
  ColorScheme,
  SubmitAnswerResponse,
  Phase,
  StartSessionResponse,
  acknowledgeComplete,
  cancelAutoRepeat,
  getAppSettings,
  onAppSettingsChanged,
  onAutoRepeatTick,
  onAutoRepeatWaiting,
  onClearScreen,
  onCountdownTick,
  onSessionComplete,
  onShowNumber,
  playSound,
  ping,
  SessionConfigInput,
  startSession,
  setColorScheme as setColorSchemeCmd,
  ThemeMode,
  setThemeMode as setThemeModeCmd,
  submitAnswerText,
  stopSession,
  getSoundEnabled as getSoundEnabledCmd,
  setSoundEnabled as setSoundEnabledCmd,
} from "./tauri";

if (import.meta.env.DEV) {
  ping().then(console.log).catch(() => { });
}

function isEscapeKey(event: KeyboardEvent): boolean {
  return event.key === "Escape";
}

async function setFullscreen(enabled: boolean): Promise<void> {
  const win = getCurrentWindow();
  try {
    const isFullscreen = await win.isFullscreen();
    if (isFullscreen !== enabled) {
      await win.setFullscreen(enabled);
    }
  } catch {
    // Ignore: some platforms/window managers may refuse fullscreen.
  }
}

async function forceFullscreenBeforeStart(): Promise<void> {
  const win = getCurrentWindow();

  try {
    await win.setFullscreen(true);
  } catch {
    // Fall through and verify below.
  }

  for (let i = 0; i < 30; i += 1) {
    try {
      if (await win.isFullscreen()) return;
    } catch {
      break;
    }
    await new Promise((r) => setTimeout(r, 25));
  }

  throw new Error("Unable to enter fullscreen");
}

export default function App() {
  const [showSplash, setShowSplash] = createSignal<boolean>(true);
  const [splashVisible, setSplashVisible] = createSignal<boolean>(false);

  const [colorScheme, setColorScheme] = createSignal<ColorScheme>("midnight");
  const [themeMode, setThemeMode] = createSignal<ThemeMode>("dark");
  const themeClass = () => `theme-${colorScheme()}`;
  const modeClass = () => `theme-${themeMode()}`;

  // Fade durations (ms)
  const fadeMs = 2000; // 2s fade in/out
  const holdMs = 800; // visible hold between fades

  onMount(() => {
    if (!showSplash()) return;

    // trigger fade-in on next tick
    const enterTimer = window.setTimeout(() => setSplashVisible(true), 10);

    // schedule fade-out after fade-in + hold
    const exitTimer = window.setTimeout(() => setSplashVisible(false), fadeMs + holdMs + 10);

    // schedule unmount after fade-out completes
    const unmountTimer = window.setTimeout(() => setShowSplash(false), fadeMs + holdMs + fadeMs + 20);

    const onKey = (e: KeyboardEvent) => {
      if (!showSplash()) return;
      // Any key press skips splash: start fade-out immediately if visible
      if (splashVisible()) {
        // clear pending timers and start immediate fade-out
        window.clearTimeout(exitTimer);
        window.clearTimeout(unmountTimer);
        setSplashVisible(false);
        window.setTimeout(() => setShowSplash(false), fadeMs);
      } else {
        // If not yet visible, abort and unmount quickly
        window.clearTimeout(enterTimer);
        window.clearTimeout(exitTimer);
        window.clearTimeout(unmountTimer);
        setSplashVisible(false);
        setShowSplash(false);
      }
    };

    window.addEventListener("keydown", onKey);

    onCleanup(() => {
      window.clearTimeout(enterTimer);
      window.clearTimeout(exitTimer);
      window.clearTimeout(unmountTimer);
      window.removeEventListener("keydown", onKey);
    });
  });

  const [displayText, setDisplayText] = createSignal<string>("");
  const [phase, setPhase] = createSignal<Phase>("idle");
  const [errorText, setErrorText] = createSignal<string>("");

  const [showAnswer, setShowAnswer] = createSignal<boolean>(false);
  const [answerText, setAnswerText] = createSignal<string>("");
  const [answerMode, setAnswerMode] = createSignal<"reveal" | "type">("reveal");
  const [typedAnswer, setTypedAnswer] = createSignal<string>("");
  const [validationSummary, setValidationSummary] = createSignal<string>("");
  const [showNumbersList, setShowNumbersList] = createSignal<boolean>(false);
  const [hasValidated, setHasValidated] = createSignal<boolean>(false);
  const [answerSum, setAnswerSum] = createSignal<number>(0);

  const [sessionId, setSessionId] = createSignal<number | null>(null);
  const [numbers, setNumbers] = createSignal<number[]>([]);

  // Interactive logo/title transform based on window size while preserving
  // proportions relative to the initial app startup dimensions. This keeps
  // the starting layout identical but makes subsequent resizes animate
  // the logo gently toward the top-left.
  onMount(() => {
    const root = document.documentElement;
    const baselineW = Math.max(800, window.innerWidth);
    const baselineH = Math.max(600, window.innerHeight);
    let raf = 0;

    const update = () => {
      const w = window.innerWidth;
      const h = window.innerHeight;
      const ratio = Math.min(w / baselineW, h / baselineH);
      const clamped = Math.max(0.75, Math.min(ratio, 1.5));

      // Shift amounts are proportional to baseline dims, capped to sensible px.
      const shiftX = Math.min(220, Math.max(48, baselineW * 0.06));
      const shiftY = Math.min(120, Math.max(20, baselineH * 0.035));

      const offsetX = (clamped - 1) * -shiftX; // negative -> move left when larger
      const offsetY = (clamped - 1) * -shiftY; // negative -> move up when larger
      const logoScale = 1 + (clamped - 1) * 0.22; // modest scale up
      const titleOffset = (clamped - 1) * -8; // small title lift

      root.style.setProperty("--ui-scale", `${clamped}`);
      root.style.setProperty("--home-logo-translate-x", `${offsetX}px`);
      root.style.setProperty("--home-logo-translate-y", `${offsetY}px`);
      root.style.setProperty("--home-logo-scale", `${logoScale}`);
      root.style.setProperty("--home-title-translate-y", `${titleOffset}px`);
    };

    const handler = () => {
      if (raf) cancelAnimationFrame(raf);
      raf = requestAnimationFrame(update);
    };

    // initialize and listen
    update();
    window.addEventListener("resize", handler, { passive: true });

    onCleanup(() => {
      if (raf) cancelAnimationFrame(raf);
      window.removeEventListener("resize", handler);
    });
  });

  const [autoRepeatEnabled, setAutoRepeatEnabled] = createSignal<boolean>(false);
  const [autoRepeatCount, setAutoRepeatCount] = createSignal<number>(5);
  const [autoRepeatDelaySeconds, setAutoRepeatDelaySeconds] = createSignal<number>(5);
  const [autoRepeatRemaining, setAutoRepeatRemaining] = createSignal<number>(0);
  const [autoRepeatSecondsLeft, setAutoRepeatSecondsLeft] =
    createSignal<number | null>(null);

  const [showAdvanced, setShowAdvanced] = createSignal<boolean>(false);
  const [soundEnabled, setSoundEnabled] = createSignal<boolean>(true);
  const [countdownTickId, setCountdownTickId] = createSignal<number>(0);

  const [digitsPerNumber, setDigitsPerNumber] = createSignal<number>(1);
  const [numberDurationSeconds, setNumberDurationSeconds] = createSignal<number>(0.5);
  const [delayBetweenNumbersSeconds, setDelayBetweenNumbersSeconds] = createSignal<number>(0);
  const [totalNumbers, setTotalNumbers] = createSignal<number>(5);

  const [allowNegativeNumbers, setAllowNegativeNumbers] = createSignal<boolean>(false);

  let sumInputRef: HTMLInputElement | undefined;

  const isRunning = (): boolean => phase() === "starting" || phase() === "flashing";

  const resetForIncomingSessionIfComplete = () => {
    if (phase() !== "complete") return;
    setShowAnswer(false);
    setAnswerText("");
    setAnswerSum(0);
    setTypedAnswer("");
    setValidationSummary("");
    setShowNumbersList(false);
    setHasValidated(false);
    setAutoRepeatSecondsLeft(null);
  };

  const goHome = () => {
    setPhase("idle");
    setDisplayText("");
    setShowAnswer(false);
    setAnswerText("");
    setAnswerSum(0);
    setTypedAnswer("");
    setValidationSummary("");
    setShowNumbersList(false);
    setHasValidated(false);
    setAutoRepeatRemaining(0);
    setAutoRepeatSecondsLeft(null);
    setSessionId(null);
    setNumbers([]);
  };

  const applyAutoRepeatWaiting = (payload: AutoRepeatWaitingPayload) => {
    setAutoRepeatRemaining(payload.remaining);
    // seconds are driven from Rust tick events.
    setAutoRepeatSecondsLeft(null);
  };

  createEffect(() => {
    if (phase() !== "complete") return;
    if (answerMode() !== "type") return;
    requestAnimationFrame(() => sumInputRef?.focus?.());
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

    void playSound(ok ? 'applause' : 'buzzer');

    if (resp.auto_repeat_waiting) {
      applyAutoRepeatWaiting(resp.auto_repeat_waiting);
    }
  };

  const validateTypedAnswer = async () => {
    const sid = sessionId();
    if (sid == null) {
      setValidationSummary("No active session id to validate.");
      return;
    }

    try {
      const resp = await submitAnswerText(sid, typedAnswer());
      setHasValidated(true);
      applySubmitAnswerResponse(resp);
    } catch (e) {
      setHasValidated(false);
      setValidationSummary(String(e));
    }
  };

  onMount(async () => {
    try {
      const settings = await getAppSettings();
      setColorScheme(settings.color_scheme);
      setThemeMode(settings.theme_mode ?? "dark");
      try {
        const s = await getSoundEnabledCmd();
        setSoundEnabled(s);
      } catch {
        // ignore: best-effort to sync backend sound flag
      }
    } catch {
      // Best-effort.
    }

    const unlistenCountdown = await onCountdownTick((value) => {
      resetForIncomingSessionIfComplete();
      setPhase("countdown");
      setDisplayText(value);
      setCountdownTickId((n) => n + 1);
      void setFullscreen(true);
    });

    const unlistenFlash = await onShowNumber((payload) => {
      resetForIncomingSessionIfComplete();
      setSessionId(payload.session_id);
      setPhase("flashing");
      setDisplayText(String(payload.value));
      void playSound('beep');
      void setFullscreen(true);
    });

    const unlistenClear = await onClearScreen(() => {
      setDisplayText("");
    });

    const unlistenAutoRepeatWaiting = await onAutoRepeatWaiting((payload) => {
      if (!autoRepeatEnabled()) return;
      applyAutoRepeatWaiting(payload);
    });

    const unlistenAutoRepeatTick = await onAutoRepeatTick((payload) => {
      if (!autoRepeatEnabled()) return;
      setAutoRepeatRemaining(payload.remaining);
      setAutoRepeatSecondsLeft(payload.seconds_left);
    });

    const unlistenSettings = await onAppSettingsChanged((payload) => {
      setColorScheme(payload.color_scheme);
      // payload.theme_mode may be 'dark' | 'light'
      // ensure UI reflects server-side change     
      setThemeMode(payload.theme_mode ?? ("dark" as any));
    });

    const unlistenComplete = await onSessionComplete((payload) => {
      setPhase("complete");
      setDisplayText("");
      setShowAnswer(false);
      setNumbers(payload.numbers);
      setAnswerText(payload.numbers.join("\n"));
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
      if (phase() !== "flashing" && phase() !== "starting" && phase() !== "countdown") return;
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
    setDelayBetweenNumbersSeconds(resp.effective_config.delay_between_numbers_s);
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
    setAnswerText("");
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
      delay_between_numbers_s: delayBetweenNumbersSeconds(),
      total_numbers: Math.trunc(totalNumbers()),
      allow_negative_numbers: allowNegativeNumbers(),
    };

    try {
      setPhase("starting");
      setDisplayText("");

      setAutoRepeatRemaining(autoRepeatEnabled() ? Math.trunc(autoRepeatCount()) : 0);

      await forceFullscreenBeforeStart();
      const resp = await startSession(
        config,
        autoRepeatEnabled()
          ? {
            enabled: true,
            repeats: Math.trunc(autoRepeatCount()),
            delay_s: autoRepeatDelaySeconds(),
          }
          : null
      );
      applyStartSessionResponse(resp);
    } catch (e) {
      setPhase("idle");
      void setFullscreen(false);
      setErrorText(String(e));
    }
  };

  const stop = async () => {
    try {
      await stopSession();
    } finally {
      setPhase("idle");
      setDisplayText("");
      setShowAnswer(false);
      setAnswerText("");
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
      <div classList={{ app: true, [themeClass()]: true, [modeClass()]: true, home: phase() === "idle" }}>
        {showSplash() ? (
          <div classList={{ splash: true, visible: splashVisible() }}>
            <img src="/Ascent_Banner.png" alt="Ascent Banner" class="splashBanner" />
            <div class="splashText">
              <div class="splashTitle">Ascent Abacus &amp; Brain Gym</div>
              <div class="splashSubtitle">Your one stop solution for IQ improvement</div>
            </div>
          </div>
        ) : null}
        {phase() === "idle" ? (
          <div class="panel">
            <div class="title">Ascent Flash</div>
            <button
              class="iconButton"
              type="button"
              aria-label="Additional settings"
              disabled={isRunning()}
              onClick={() => setShowAdvanced((v) => !v)}
            >
              ⚙
            </button>

            {showAdvanced() ? (
              <div
                class="advancedOverlay"
                onMouseDown={(e) => {
                  if (e.target === e.currentTarget) setShowAdvanced(false);
                }}
              >
                <div
                  class="advancedPanel"
                  onMouseDown={(e) => {
                    e.stopPropagation();
                  }}
                >

                  <div class="advancedSectionTitle">Additional settings</div>

                  <div class="advancedSetting">
                    <div class="settingRow">
                      <div class="label">Auto-repeat</div>
                      <div class="segmented" role="radiogroup" aria-label="Auto-repeat">
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
                              void cancelAutoRepeat();
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
                                  : 1
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
                                : 1
                            )
                          }
                        />
                      </div>
                    ) : null}

                    {autoRepeatEnabled() ? (
                      <>
                        <div class="fieldRow">
                          <div class="label">Delay before next question (s)</div>
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
                                  : 5
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
                      <div class="segmented" role="radiogroup" aria-label="Answer mode">
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
                        <div class="modeVertical" role="radiogroup" aria-label="Theme mode">
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
                                void setThemeModeCmd("dark");
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
                                void setThemeModeCmd("light");
                              }}
                            />
                            <span class="segmentedLabel">Light</span>
                          </label>
                        </div>
                      </div>
                      <div class="colorGrid" role="radiogroup" aria-label="Color">
                        {
                          ((): any => {
                            const values = ["midnight", "crimson", "aqua", "violet", "amber", "ivory"] as const;
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
                              const label = themeMode() === "light" ? lightNames[value] : darkNames[value];
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
                                      void setColorSchemeCmd(value);
                                    }}
                                  />
                                  <span classList={{ colorSwatch: true, [`sw-preview-${value}`]: true }} aria-hidden="true" />
                                  <span class="colorLabel">{label}</span>
                                </label>
                              );
                            });
                          })()
                        }
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

                  <div class="advancedSetting field">
                    <div class="fieldRow">
                      <div class="label">Delay between numbers (s)</div>
                      <input
                        class="input"
                        type="number"
                        min="0"
                        max="5"
                        step="0.1"
                        value={delayBetweenNumbersSeconds()}
                        disabled={isRunning()}
                        onInput={(e) =>
                          setDelayBetweenNumbersSeconds(
                            Number.isFinite(e.currentTarget.valueAsNumber)
                              ? e.currentTarget.valueAsNumber
                              : 0
                          )
                        }
                      />
                    </div>
                    <input
                      class="range"
                      type="range"
                      min="0"
                      max="5"
                      step="0.1"
                      value={delayBetweenNumbersSeconds()}
                      disabled={isRunning()}
                      onInput={(e) =>
                        setDelayBetweenNumbersSeconds(
                          Number.isFinite(e.currentTarget.valueAsNumber)
                            ? e.currentTarget.valueAsNumber
                            : 0
                        )
                      }
                    />
                  </div>

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
                    max="18"
                    step="1"
                    value={digitsPerNumber()}
                    disabled={isRunning()}
                    onInput={(e) =>
                      setDigitsPerNumber(
                        Number.isFinite(e.currentTarget.valueAsNumber)
                          ? e.currentTarget.valueAsNumber
                          : 1
                      )
                    }
                  />
                </div>
                <input
                  class="range"
                  type="range"
                  min="1"
                  max="18"
                  step="1"
                  value={digitsPerNumber()}
                  disabled={isRunning()}
                  onInput={(e) =>
                    setDigitsPerNumber(
                      Number.isFinite(e.currentTarget.valueAsNumber)
                        ? e.currentTarget.valueAsNumber
                        : 1
                    )
                  }
                />
              </div>

              <div class="field">
                <div class="fieldRow">
                  <div class="label">Number duration (s)</div>
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
                          : 0.1
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
                        : 0.1
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
                          : 1
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
                        : 1
                    )
                  }
                />
              </div>
            </div>

            <div class="actions">
              <div class="soundGroup">
                <div class="soundLabel">Sound</div>
                <div class="segmented" role="radiogroup" aria-label="Sound">
                  <label class="segmentedOption">
                    <input
                      class="segmentedInput"
                      type="radio"
                      name="sound"
                      value="on"
                      checked={soundEnabled()}
                      onInput={async () => { setSoundEnabled(true); await setSoundEnabledCmd(true); }}
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
                      onInput={async () => { setSoundEnabled(false); await setSoundEnabledCmd(false); }}
                    />
                    <span class="segmentedLabel">Off</span>
                  </label>
                </div>
              </div>

              <button class="button" disabled={isRunning()} onClick={start}>
                Start
              </button>
            </div>

            {errorText() ? <div class="error">{errorText()}</div> : null}
          </div>
        ) : phase() === "complete" ? (
          <div class="endScreen">
            {answerMode() === "reveal" ? (
              <div class="answerCard">
                <div class="endHeaderCenter">
                  <div class="endTitle">Session complete</div>
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
                              const waiting = await acknowledgeComplete(sid);
                              if (waiting) applyAutoRepeatWaiting(waiting);
                            } catch {
                              // Best-effort.
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
                          <For each={numbers()}>{(n, idx) => (
                            <div class="answerRow">
                              <div class="answerIndex">{idx() + 1}</div>
                              <div class="answerValue">{n}</div>
                            </div>
                          )}</For>
                        </div>
                      ) : null}
                    </>
                  )}
                </div>

                <div class="endFooter">
                  <div class="endFooterInner">
                    <div class="actionField">
                      <div class="centerActions">
                        {showAnswer() ? (
                          <button
                            class="button"
                            type="button"
                            onClick={() => {
                              setShowAnswer(false);
                              setShowNumbersList(false);
                            }}
                          >
                            Hide
                          </button>
                        ) : null}
                        {showAnswer() && hasValidated() ? (
                          <button
                            class="button"
                            type="button"
                            onClick={() => setShowNumbersList((v) => !v)}
                          >
                            {showNumbersList() ? "Hide numbers" : "Show numbers"}
                          </button>
                        ) : null}
                        <button class="button" type="button" onClick={() => void stop()}>
                          Home
                        </button>
                      </div>
                    </div>

                    {autoRepeatEnabled() && hasValidated() && autoRepeatSecondsLeft() != null ? (
                      <div class="autoRepeatStatus">
                        Next question in {autoRepeatSecondsLeft() ?? 0}s · {autoRepeatRemaining()} remaining
                      </div>
                    ) : null}
                  </div>
                </div>
              </div>
            ) : (
              <div class="answerCard">
                <div class="endHeaderCenter">
                  <div class="endTitle">Session complete</div>
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
                    value={typedAnswer()}
                    onInput={(e) => setTypedAnswer(e.currentTarget.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") void validateTypedAnswer();
                    }}
                    spellcheck={false}
                  />

                  {validationSummary() ? <pre class="validationText">{validationSummary()}</pre> : null}

                  {hasValidated() && showNumbersList() ? (
                    <div class="answerNumbers">
                      <For each={numbers()}>{(n, idx) => (
                        <div class="answerRow">
                          <div class="answerIndex">{idx() + 1}</div>
                          <div class="answerValue">{n}</div>
                        </div>
                      )}</For>
                    </div>
                  ) : null}
                </div>

                <div class="endFooter">
                  <div class="endFooterInner">
                    <div class="actionField">
                      <div class="centerActions">
                        <button class="button" type="button" onClick={() => void validateTypedAnswer()}>
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
                        <button class="button" type="button" onClick={() => void stop()}>
                          Home
                        </button>
                      </div>
                    </div>

                    {autoRepeatEnabled() && hasValidated() && autoRepeatSecondsLeft() != null ? (
                      <div class="autoRepeatStatus">
                        Next question in {autoRepeatSecondsLeft() ?? 0}s · {autoRepeatRemaining()} remaining
                      </div>
                    ) : null}
                  </div>
                </div>
              </div>
            )}
          </div>
        ) : (
          <div
            classList={{ number: true, countdown: phase() === "countdown" }}
            style={{
              "--len": Math.max(
                1,
                (displayText().startsWith("-") ? displayText().slice(1) : displayText()).length +
                (allowNegativeNumbers() ? 1 : 0)
              ),
            }}
          >
            <Show when={phase() === "countdown"}>
              <div
                classList={{
                  countdownShutter: true,
                  shutterA: countdownTickId() % 2 === 0,
                  shutterB: countdownTickId() % 2 === 1,
                }}
                aria-hidden="true"
              />

              <svg class="countdownRing" viewBox="0 0 100 100" aria-hidden="true">
                <circle class="countdownTrack" cx="50" cy="50" r="44" />
                <circle
                  classList={{
                    countdownProgress: true,
                    countdownProgressA: countdownTickId() % 2 === 0,
                    countdownProgressB: countdownTickId() % 2 === 1,
                  }}
                  cx="50"
                  cy="50"
                  r="44"
                />
              </svg>
            </Show>

            {phase() === "countdown" ? (
              <span class="countdownDigit">{displayText()}</span>
            ) : (
              <span class="signedNumber">
                <span class="magnitude">
                  {displayText().startsWith("-") ? displayText().slice(1) : displayText()}
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
        <a href="https://www.ascentabacus.com" target="_blank" rel="noopener noreferrer">www.ascentabacus.com</a>
        <div class="isoRow">
          <span class="iso">ISO 9001</span>
          <span class="iso">ISO 14001</span>
        </div>
      </div>
    </>
  );
}
