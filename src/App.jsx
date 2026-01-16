import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Show, createEffect, createSignal, onCleanup, onMount } from "solid-js";
import "./App.css";

if (import.meta.env.DEV) {
  invoke("ping").then(console.log).catch(() => { });
}

function isEscapeKey(event) {
  return event.key === "Escape";
}

async function setFullscreen(enabled) {
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

async function forceFullscreenBeforeStart() {
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

function App() {
  const [displayText, setDisplayText] = createSignal("");
  const [phase, setPhase] = createSignal("idle");
  const [errorText, setErrorText] = createSignal("");

  const [showAnswer, setShowAnswer] = createSignal(false);
  const [answerText, setAnswerText] = createSignal("");
  const [answerMode, setAnswerMode] = createSignal("reveal");
  const [typedAnswer, setTypedAnswer] = createSignal("");
  const [validationSummary, setValidationSummary] = createSignal("");
  const [showNumbersList, setShowNumbersList] = createSignal(false);
  const [hasValidated, setHasValidated] = createSignal(false);
  const [answerSum, setAnswerSum] = createSignal(0);
  let capturedNumbers = [];

  const [autoRepeatEnabled, setAutoRepeatEnabled] = createSignal(false);
  const [autoRepeatCount, setAutoRepeatCount] = createSignal(5);
  const [autoRepeatDelaySeconds, setAutoRepeatDelaySeconds] = createSignal(5);
  const [autoRepeatRemaining, setAutoRepeatRemaining] = createSignal(0);
  const [autoRepeatSecondsLeft, setAutoRepeatSecondsLeft] = createSignal(null);

  const sumCapturedNumbers = (values) => {
    let total = 0;
    for (const raw of values) {
      const n = Number.parseInt(String(raw).trim(), 10);
      if (!Number.isFinite(n)) continue;
      total += n;
    }
    return total;
  };

  const parseUserSum = (raw) => {
    const cleaned = String(raw).trim().replace(/,/g, "");
    if (!cleaned) return null;
    // Strict integer parse (allows leading + or -)
    if (!/^[+-]?\d+$/.test(cleaned)) return null;
    const n = Number.parseInt(cleaned, 10);
    return Number.isFinite(n) ? n : null;
  };

  const validateTypedAnswer = () => {
    const expected = answerSum();
    const provided = parseUserSum(typedAnswer());
    if (provided === null) {
      setValidationSummary(
        "Enter a single integer answer (e.g. 42 or -17)."
      );
      return;
    }

    setHasValidated(true);

    const delta = provided - expected;
    const ok = delta === 0;

    const lines = [
      ok ? "Correct ✅" : "Incorrect",
      `Expected answer: ${expected}`,
      `You entered: ${provided}`,
    ];

    if (!ok) lines.push(`Difference: ${delta > 0 ? "+" : ""}${delta}`);

    setValidationSummary(lines.join("\n"));
  };

  const [showAdvanced, setShowAdvanced] = createSignal(false);

  const [countdownTickId, setCountdownTickId] = createSignal(0);

  const [digitsPerNumber, setDigitsPerNumber] = createSignal(1);
  const [numberDurationSeconds, setNumberDurationSeconds] = createSignal(0.5);
  const [delayBetweenNumbersSeconds, setDelayBetweenNumbersSeconds] = createSignal(0);
  const [totalNumbers, setTotalNumbers] = createSignal(5);

  const [allowNegativeNumbers, setAllowNegativeNumbers] = createSignal(false);

  let sumInputRef;

  createEffect(() => {
    setAutoRepeatSecondsLeft(null);

    if (phase() !== "complete") return;
    if (!autoRepeatEnabled()) return;
    if (!hasValidated()) return;
    if (autoRepeatRemaining() <= 0) return;

    const delaySeconds = Math.max(5, autoRepeatDelaySeconds());
    const nextAtMs = Date.now() + delaySeconds * 1000;

    const updateSecondsLeft = () => {
      const remainingMs = nextAtMs - Date.now();
      const secondsLeft = Math.max(0, Math.ceil(remainingMs / 1000));
      setAutoRepeatSecondsLeft(secondsLeft);
    };

    updateSecondsLeft();

    const intervalId = window.setInterval(updateSecondsLeft, 200);
    const timerId = window.setTimeout(() => {
      if (phase() !== "complete") return;
      if (!autoRepeatEnabled()) return;
      if (!hasValidated()) return;
      setAutoRepeatRemaining((n) => Math.max(0, n - 1));
      void start({ autoRepeat: true });
    }, delaySeconds * 1000);

    onCleanup(() => {
      window.clearInterval(intervalId);
      window.clearTimeout(timerId);
      setAutoRepeatSecondsLeft(null);
    });
  });

  createEffect(() => {
    if (phase() !== "complete") return;
    if (answerMode() !== "type") return;
    // Focus is best-effort; some platforms may block programmatic focus.
    requestAnimationFrame(() => sumInputRef?.focus?.());
  });

  onMount(async () => {
    const unlistenCountdown = await listen("countdown_tick", (event) => {
      setPhase("countdown");
      setDisplayText(String(event.payload ?? ""));
      setCountdownTickId((n) => n + 1);
      void setFullscreen(true);
    });

    const unlistenFlash = await listen("show_number", (event) => {
      setPhase("flashing");
      const value = String(event.payload ?? "");
      capturedNumbers.push(value);
      setDisplayText(value);
      void setFullscreen(true);
    });

    const unlistenClear = await listen("clear_screen", () => {
      setDisplayText("");
    });

    const unlistenComplete = await listen("session_complete", () => {
      setPhase("complete");
      setDisplayText("");
      setShowAnswer(false);
      setAnswerText(capturedNumbers.join("\n"));
      setAnswerSum(sumCapturedNumbers(capturedNumbers));
      setTypedAnswer("");
      setValidationSummary("");
      setShowNumbersList(false);
      setHasValidated(false);
      void setFullscreen(false);
    });

    const onKeyDown = (e) => {
      if (!isEscapeKey(e)) return;
      if (phase() !== "flashing" && phase() !== "starting" && phase() !== "countdown") return;
      invoke("stop_session");
      setPhase("idle");
      setErrorText("");
      setDisplayText("");
      setShowAnswer(false);
      setAnswerText("");
      setAnswerSum(0);
      setTypedAnswer("");
      setValidationSummary("");
      setShowNumbersList(false);
      setHasValidated(false);
      capturedNumbers = [];
      void setFullscreen(false);
    };

    window.addEventListener("keydown", onKeyDown);

    onCleanup(() => {
      window.removeEventListener("keydown", onKeyDown);
      unlistenCountdown();
      unlistenFlash();
      unlistenClear();
      unlistenComplete();
    });
  });

  const isRunning = () => phase() === "starting" || phase() === "flashing";

  const clampInt = (raw, min, max) => {
    const n = Number.parseInt(String(raw), 10);
    if (!Number.isFinite(n)) return min;
    return Math.max(min, Math.min(max, n));
  };

  const clampFloat1 = (raw, min, max) => {
    const n = Number.parseFloat(String(raw));
    const safe = Number.isFinite(n) ? n : min;
    const clamped = Math.max(min, Math.min(max, safe));
    return Math.round(clamped * 10) / 10;
  };

  const goHome = () => {
    setPhase("idle");
    setShowAnswer(false);
    setAnswerText("");
    setAnswerSum(0);
    setTypedAnswer("");
    setValidationSummary("");
    setShowNumbersList(false);
    setHasValidated(false);
    setAutoRepeatRemaining(0);
    capturedNumbers = [];
  };

  const start = async (opts) => {
    const isAutoRepeatStart = Boolean(opts?.autoRepeat);

    setErrorText("");
    setShowAdvanced(false);
    setShowAnswer(false);
    setAnswerText("");
    setAnswerSum(0);
    setTypedAnswer("");
    setValidationSummary("");
    setShowNumbersList(false);
    setHasValidated(false);
    capturedNumbers = [];

    const config = {
      digits_per_number: digitsPerNumber(),
      number_duration_ms: Math.round(numberDurationSeconds() * 1000),
      delay_between_numbers_ms: Math.round(delayBetweenNumbersSeconds() * 1000),
      total_numbers: totalNumbers(),
      allow_negative_numbers: allowNegativeNumbers(),
    };

    if (
      config.digits_per_number <= 0 ||
      config.number_duration_ms <= 0 ||
      config.total_numbers <= 0
    ) {
      setErrorText("Digits, duration, and total numbers must be > 0");
      return;
    }

    try {
      setPhase("starting");
      setDisplayText("");

      // Initialize auto-repeat budget for this run (only on manual starts).
      if (!isAutoRepeatStart) {
        if (autoRepeatEnabled()) {
          setAutoRepeatRemaining(clampInt(autoRepeatCount(), 1, 20));
        } else {
          setAutoRepeatRemaining(0);
        }
      }

      await forceFullscreenBeforeStart();
      await invoke("start_session", { config });
    } catch (e) {
      setPhase("idle");
      void setFullscreen(false);
      setErrorText(String(e));
    }
  };

  const stop = async () => {
    try {
      await invoke("stop_session");
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
                          onInput={() => setAutoRepeatEnabled(false)}
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
                onInput={(e) =>
                  setNumberDurationSeconds(clampFloat1(e.currentTarget.value, 0.1, 5))
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
            <button class="button" disabled={isRunning()} onClick={() => start()}>
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
                      onClick={() => {
                        setHasValidated(true);
                        setShowAnswer(true);
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
                      <button
                        class="button"
                        type="button"
                        onClick={goHome}
                      >
                        Home
                      </button>
                    </div>
                  </div>

                  {autoRepeatEnabled() && hasValidated() && autoRepeatRemaining() > 0 ? (
                    <div class="autoRepeatStatus">
                      Next question in {autoRepeatSecondsLeft() ?? Math.max(5, autoRepeatDelaySeconds())}
                      s · {autoRepeatRemaining()} remaining
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
                  ref={sumInputRef}
                  class="sumInput"
                  type="text"
                  inputmode="numeric"
                  autocomplete="off"
                  placeholder="Enter the answer"
                  value={typedAnswer()}
                  onInput={(e) => setTypedAnswer(e.currentTarget.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") validateTypedAnswer();
                  }}
                  spellcheck={false}
                />

                {validationSummary() ? (
                  <pre class="validationText">{validationSummary()}</pre>
                ) : null}

                {hasValidated() && showNumbersList() ? (
                  <pre class="answerText">{answerText()}</pre>
                ) : null}
              </div>

              <div class="endFooter">
                <div class="endFooterInner">
                  <div class="actionField">
                    <div class="centerActions">
                      <button class="button" type="button" onClick={validateTypedAnswer}>
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
                      <button
                        class="button"
                        type="button"
                        onClick={goHome}
                      >
                        Home
                      </button>
                    </div>
                  </div>

                  {autoRepeatEnabled() && hasValidated() && autoRepeatRemaining() > 0 ? (
                    <div class="autoRepeatStatus">
                      Next question in {autoRepeatSecondsLeft() ?? Math.max(5, autoRepeatDelaySeconds())}
                      s · {autoRepeatRemaining()} remaining
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

export default App;
