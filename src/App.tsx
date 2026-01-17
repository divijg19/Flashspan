import { getCurrentWindow } from "@tauri-apps/api/window";
import { Show, createEffect, createSignal, onCleanup, onMount } from "solid-js";
import "./App.css";
import {
  AutoRepeatWaitingPayload,
  SubmitAnswerResponse,
  Phase,
  SessionCompleteV2,
  acknowledgeComplete,
  cancelAutoRepeat,
  onAutoRepeatWaiting,
  onClearScreen,
  onCountdownTick,
  onSessionCompleteLegacy,
  onSessionCompleteV2,
  onShowNumberLegacy,
  onShowNumberV2,
  ping,
  SessionConfig,
  startSessionV2,
  submitAnswer,
  stopSession,
} from "./tauri";

if (import.meta.env.DEV) {
  ping().then(console.log).catch(() => {});
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

  // Wait briefly for the window manager to apply fullscreen.
  // This is UI-only coordination; Rust still owns all flash timing.
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

  // Fallback capture for legacy events.
  let capturedNumbers: string[] = [];

  const [autoRepeatEnabled, setAutoRepeatEnabled] = createSignal<boolean>(false);
  const [autoRepeatCount, setAutoRepeatCount] = createSignal<number>(5);
  const [autoRepeatDelaySeconds, setAutoRepeatDelaySeconds] = createSignal<number>(5);
  const [autoRepeatRemaining, setAutoRepeatRemaining] = createSignal<number>(0);
  const [autoRepeatNextStartAtMs, setAutoRepeatNextStartAtMs] =
    createSignal<number | null>(null);
  const [autoRepeatSecondsLeft, setAutoRepeatSecondsLeft] =
    createSignal<number | null>(null);

  const [showAdvanced, setShowAdvanced] = createSignal<boolean>(false);
  const [countdownTickId, setCountdownTickId] = createSignal<number>(0);

  const [digitsPerNumber, setDigitsPerNumber] = createSignal<number>(1);
  const [numberDurationSeconds, setNumberDurationSeconds] = createSignal<number>(0.5);
  const [delayBetweenNumbersSeconds, setDelayBetweenNumbersSeconds] = createSignal<number>(0);
  const [totalNumbers, setTotalNumbers] = createSignal<number>(5);

  const [allowNegativeNumbers, setAllowNegativeNumbers] = createSignal<boolean>(false);

  let sumInputRef: HTMLInputElement | undefined;

  const isRunning = (): boolean => phase() === "starting" || phase() === "flashing";

  const clampInt = (raw: unknown, min: number, max: number): number => {
    const n = Number.parseInt(String(raw), 10);
    if (!Number.isFinite(n)) return min;
    return Math.max(min, Math.min(max, n));
  };

  const clampFloat1 = (raw: unknown, min: number, max: number): number => {
    const n = Number.parseFloat(String(raw));
    const safe = Number.isFinite(n) ? n : min;
    const clamped = Math.max(min, Math.min(max, safe));
    return Math.round(clamped * 10) / 10;
  };

  const sumCapturedNumbers = (values: string[]): number => {
    let total = 0;
    for (const raw of values) {
      const n = Number.parseInt(String(raw).trim(), 10);
      if (!Number.isFinite(n)) continue;
      total += n;
    }
    return total;
  };

  const parseUserSum = (raw: unknown): number | null => {
    const cleaned = String(raw).trim().replace(/,/g, "");
    if (!cleaned) return null;
    if (!/^[+-]?\d+$/.test(cleaned)) return null;
    const n = Number.parseInt(cleaned, 10);
    return Number.isFinite(n) ? n : null;
  };

  const resetForIncomingSessionIfComplete = () => {
    if (phase() !== "complete") return;
    setShowAnswer(false);
    setAnswerText("");
    setAnswerSum(0);
    setTypedAnswer("");
    setValidationSummary("");
    setShowNumbersList(false);
    setHasValidated(false);
    setAutoRepeatNextStartAtMs(null);
    setAutoRepeatSecondsLeft(null);
    capturedNumbers = [];
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
    setAutoRepeatNextStartAtMs(null);
    setAutoRepeatSecondsLeft(null);
    setSessionId(null);
    setNumbers([]);
    capturedNumbers = [];
  };

  const applyAutoRepeatWaiting = (payload: AutoRepeatWaitingPayload) => {
    setAutoRepeatRemaining(payload.remaining);
    setAutoRepeatNextStartAtMs(payload.next_start_at_ms);
  };

  createEffect(() => {
    const nextAt = autoRepeatNextStartAtMs();
    if (nextAt == null) {
      setAutoRepeatSecondsLeft(null);
      return;
    }

    const update = () => {
      const remainingMs = nextAt - Date.now();
      const secondsLeft = Math.max(0, Math.ceil(remainingMs / 1000));
      setAutoRepeatSecondsLeft(secondsLeft);
    };

    update();
    const intervalId = window.setInterval(update, 200);
    onCleanup(() => window.clearInterval(intervalId));
  });

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
      `You entered: ${validation.provided_sum}`,
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
    const provided = parseUserSum(typedAnswer());
    if (provided === null) {
      setValidationSummary("Enter a single integer answer (e.g. 42 or -17).\n");
      return;
    }

    setHasValidated(true);

    const sid = sessionId();
    if (sid == null) {
      setValidationSummary("No active session id to validate.");
      return;
    }

    try {
      const resp = await submitAnswer(sid, provided);
      applySubmitAnswerResponse(resp);
    } catch (e) {
      setValidationSummary(String(e));
    }
  };

  onMount(async () => {
    let gotCompleteV2 = false;

    const unlistenCountdown = await onCountdownTick((value) => {
      resetForIncomingSessionIfComplete();
      setPhase("countdown");
      setDisplayText(value);
      setCountdownTickId((n) => n + 1);
      void setFullscreen(true);
    });

    const unlistenFlashV2 = await onShowNumberV2((payload) => {
      resetForIncomingSessionIfComplete();
      setSessionId(payload.session_id);
      setPhase("flashing");
      setDisplayText(String(payload.value));
      void setFullscreen(true);
    });

    const unlistenFlash = await onShowNumberLegacy((value) => {
      resetForIncomingSessionIfComplete();
      setPhase("flashing");
      capturedNumbers.push(value);
      setDisplayText(value);
      void setFullscreen(true);
    });

    const unlistenClear = await onClearScreen(() => {
      setDisplayText("");
    });

    const unlistenAutoRepeatWaiting = await onAutoRepeatWaiting((payload) => {
      if (!autoRepeatEnabled()) return;
      applyAutoRepeatWaiting(payload);
    });

    const unlistenCompleteV2 = await onSessionCompleteV2((payload) => {
      gotCompleteV2 = true;
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
      setAutoRepeatNextStartAtMs(null);
      setAutoRepeatSecondsLeft(null);
      setSessionId(payload.session_id);
      void setFullscreen(false);
    });

    const unlistenComplete = await onSessionCompleteLegacy(() => {
      if (gotCompleteV2) return;
      setPhase("complete");
      setDisplayText("");
      setShowAnswer(false);
      setAnswerText(capturedNumbers.join("\n"));
      setAnswerSum(sumCapturedNumbers(capturedNumbers));
      setTypedAnswer("");
      setValidationSummary("");
      setShowNumbersList(false);
      setHasValidated(false);
      setAutoRepeatNextStartAtMs(null);
      setAutoRepeatSecondsLeft(null);
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
      unlistenFlashV2();
      unlistenFlash();
      unlistenClear();
      unlistenAutoRepeatWaiting();
      unlistenCompleteV2();
      unlistenComplete();
    });
  });

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

    setAutoRepeatNextStartAtMs(null);
    setAutoRepeatSecondsLeft(null);

    setSessionId(null);
    setNumbers([]);
    capturedNumbers = [];

    const config: SessionConfig = {
      digits_per_number: digitsPerNumber(),
      number_duration_ms: Math.round(numberDurationSeconds() * 1000),
      delay_between_numbers_ms: Math.round(delayBetweenNumbersSeconds() * 1000),
      total_numbers: totalNumbers(),
      allow_negative_numbers: allowNegativeNumbers(),
    };

    if (config.digits_per_number <= 0 || config.number_duration_ms <= 0 || config.total_numbers <= 0) {
      setErrorText("Digits, duration, and total numbers must be > 0");
      return;
    }

    try {
      setPhase("starting");
      setDisplayText("");

      if (autoRepeatEnabled()) {
        setAutoRepeatRemaining(clampInt(autoRepeatCount(), 1, 20));
      } else {
        setAutoRepeatRemaining(0);
      }

      await forceFullscreenBeforeStart();
      const sid = await startSessionV2(
        config,
        autoRepeatEnabled()
          ? {
              enabled: true,
              repeats: clampInt(autoRepeatCount(), 1, 20),
              delay_ms: clampInt(autoRepeatDelaySeconds(), 5, 120) * 1000,
            }
          : null
      );
      setSessionId(sid);
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
      setAutoRepeatNextStartAtMs(null);
      setAutoRepeatSecondsLeft(null);
      setSessionId(null);
      setNumbers([]);
      capturedNumbers = [];
      void setFullscreen(false);
    }
  };

  return (
    <div class="app">
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
                <div class="advancedTitle">Additional settings</div>

                <div class="advancedSectionTitle">Auto-repeat</div>

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
                            setAutoRepeatNextStartAtMs(null);
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
                </div>

                {autoRepeatEnabled() ? (
                  <>
                    <div class="advancedSetting field">
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
                            setAutoRepeatCount(clampInt(e.currentTarget.value, 1, 20))
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
                          setAutoRepeatCount(clampInt(e.currentTarget.value, 1, 20))
                        }
                      />
                    </div>

                    <div class="advancedSetting">
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
                              clampInt(e.currentTarget.value, 5, 120)
                            )
                          }
                        />
                      </div>
                      <div class="hint">Starts after you validate.</div>
                    </div>
                  </>
                ) : null}

                <div class="advancedDivider" />

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
                          clampFloat1(e.currentTarget.value, 0, 5)
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
                        clampFloat1(e.currentTarget.value, 0, 5)
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
                  onInput={(e) => setDigitsPerNumber(clampInt(e.currentTarget.value, 1, 18))}
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
                onInput={(e) => setDigitsPerNumber(clampInt(e.currentTarget.value, 1, 18))}
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
                    setNumberDurationSeconds(clampFloat1(e.currentTarget.value, 0.1, 5))
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
                onInput={(e) => setNumberDurationSeconds(clampFloat1(e.currentTarget.value, 0.1, 5))}
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
                  onInput={(e) => setTotalNumbers(clampInt(e.currentTarget.value, 1, 1500))}
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
                onInput={(e) => setTotalNumbers(clampInt(e.currentTarget.value, 1, 1500))}
              />
            </div>
          </div>

          <div class="actions">
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
                <div class="endSub">Click mode</div>
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
                      <pre class="answerText">{answerText()}</pre>
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

                  {autoRepeatEnabled() && hasValidated() && autoRepeatNextStartAtMs() != null ? (
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
                <div class="endSub">Type mode</div>
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

                {hasValidated() && showNumbersList() ? <pre class="answerText">{answerText()}</pre> : null}
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

                  {autoRepeatEnabled() && hasValidated() && autoRepeatNextStartAtMs() != null ? (
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
  );
}
