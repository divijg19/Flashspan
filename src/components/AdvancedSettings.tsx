import { type JSX, onCleanup, onMount } from "solid-js";
import type { ColorScheme, ThemeMode } from "../runtime/types";
import { COLOR_SCHEMES } from "../runtime/types";
import { numericValue } from "./NumberField";

/**
 * The advanced settings overlay: auto-repeat, answer mode, colour and theme.
 *
 * This was a ~320-line block inline in `App.tsx`, which pushed the app's own
 * session handling far enough down the file that it was hard to find. It is
 * pure presentation plus signal plumbing, so it extracts cleanly.
 *
 * Reactive values arrive as accessors rather than plain values, which is the
 * Solid convention: passing `props.autoRepeat.enabled` would read the signal
 * once at render time and never update, whereas `props.autoRepeat.enabled()`
 * stays subscribed inside the child's effects and event handlers.
 *
 * Props are grouped by the section they belong to rather than flattened. There
 * are seventeen of them, and a flat list of unrelated signals on one surface
 * makes it hard to see which controls a given setter actually drives.
 */

/** How the answer to a finished session is collected. */
export type AnswerMode = "reveal" | "type";

export interface AdvancedSettingsProps {
	/** Closes the overlay. */
	close: () => void;
	/** True while a session is live; every control is disabled then. */
	disabled: boolean;

	autoRepeat: {
		enabled: () => boolean;
		setEnabled: (value: boolean) => void;
		count: () => number;
		setCount: (value: number) => void;
		delaySeconds: () => number;
		setDelaySeconds: (value: number) => void;
		/**
		 * Turns auto-repeat off and clears the pending countdown.
		 *
		 * Passed in rather than rebuilt here because the parent's version also
		 * cancels the pending restart in the runtime; the copy that used to live
		 * in this block had to keep those four statements in step by hand.
		 */
		cancel: () => void;
	};

	answerMode: () => AnswerMode;
	setAnswerMode: (value: AnswerMode) => void;

	allowNegativeNumbers: () => boolean;
	setAllowNegativeNumbers: (value: boolean) => void;

	colorScheme: () => ColorScheme;
	setColorScheme: (value: ColorScheme) => void;

	themeMode: () => ThemeMode;
	setThemeMode: (value: ThemeMode) => void;

	/** Persists a colour scheme to the backend. */
	persistColorScheme: (value: ColorScheme) => void;
	/** Persists a theme mode to the backend. */
	persistThemeMode: (value: ThemeMode) => void;
}

/**
 * The display name for each colour scheme on the dark theme.
 *
 * The names differ per theme: the `ivory` swatch is "Obsidian" on dark and
 * "Ivory" on light, so a single name per scheme would be wrong half the time.
 *
 * The scheme list itself lives in `runtime/types.ts` next to the `ColorScheme`
 * union it enumerates, and is asserted against it with `satisfies` there — it
 * used to be declared a third time here, so adding a scheme to the type without
 * updating this list left the picker without the new option.
 */
const DARK_NAMES: Record<ColorScheme, string> = {
	midnight: "Midnight",
	crimson: "Crimson",
	aqua: "Aqua",
	violet: "Violet",
	amber: "Amber",
	ivory: "Obsidian",
};

const LIGHT_NAMES: Record<ColorScheme, string> = {
	midnight: "Dawn",
	crimson: "Blush",
	aqua: "Sea Glass",
	violet: "Lilac",
	amber: "Saffron",
	ivory: "Ivory",
};

export function AdvancedSettings(props: AdvancedSettingsProps): JSX.Element {
	/**
	 * Keeps Tab inside the panel while the overlay is open.
	 *
	 * This lived in `App.tsx` for as long as the panel did, reaching through an
	 * `overlayRef` callback into DOM the component itself rendered. That made the
	 * parent own the focus behaviour of markup it does not control, and it needed
	 * a `createEffect` plus `onCleanup` to compensate for not knowing when the
	 * panel mounts. The component already knows, so `onMount` is the honest
	 * shape and the parent is free of both the ref and the effect.
	 *
	 * `document.activeElement` is read rather than an index into `focusable`,
	 * because the browser moves focus with the keyboard and a mouse independently
	 * of the order the elements were listed in.
	 */
	let overlayRef: HTMLDivElement | undefined;
	onMount(() => {
		const panel = overlayRef?.querySelector<HTMLElement>(".advancedPanel");
		if (!panel) return;

		const focusable = panel.querySelectorAll<HTMLElement>(
			'button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])',
		);
		if (focusable.length === 0) return;

		const first = focusable[0];
		const last = focusable[focusable.length - 1];
		first.focus();

		const handler = (event: KeyboardEvent): void => {
			if (event.key !== "Tab") return;
			if (event.shiftKey && document.activeElement === first) {
				event.preventDefault();
				last.focus();
			} else if (!event.shiftKey && document.activeElement === last) {
				event.preventDefault();
				first.focus();
			}
		};

		panel.addEventListener("keydown", handler);
		onCleanup(() => panel.removeEventListener("keydown", handler));
	});

	const repeatCount = (event: { currentTarget: HTMLInputElement }): void =>
		props.autoRepeat.setCount(
			numericValue(event.currentTarget.valueAsNumber, 1),
		);

	const repeatDelay = (event: { currentTarget: HTMLInputElement }): void =>
		props.autoRepeat.setDelaySeconds(
			numericValue(event.currentTarget.valueAsNumber, 5),
		);

	return (
		<div class="advancedOverlay" ref={overlayRef}>
			<button
				class="advancedOverlayDismiss"
				type="button"
				tabIndex={-1}
				aria-label="Close additional settings"
				onClick={props.close}
			/>
			<div class="advancedPanel">
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
									disabled={props.disabled}
									checked={!props.autoRepeat.enabled()}
									onInput={props.autoRepeat.cancel}
								/>
								<span class="segmentedLabel">Off</span>
							</label>
							<label class="segmentedOption">
								<input
									class="segmentedInput"
									type="radio"
									name="auto-repeat-enabled"
									value="on"
									disabled={props.disabled}
									checked={props.autoRepeat.enabled()}
									onInput={() => props.autoRepeat.setEnabled(true)}
								/>
								<span class="segmentedLabel">On</span>
							</label>
						</div>
					</div>

					{props.autoRepeat.enabled() ? (
						<div class="field">
							<div class="fieldRow">
								<div class="label">Repeats</div>
								<input
									class="input"
									type="number"
									min="1"
									max="20"
									step="1"
									value={props.autoRepeat.count()}
									disabled={props.disabled}
									onInput={repeatCount}
								/>
							</div>
							<input
								class="range"
								type="range"
								min="1"
								max="20"
								step="1"
								value={props.autoRepeat.count()}
								disabled={props.disabled}
								onInput={repeatCount}
							/>
							{props.autoRepeat.count() < 1 ? (
								<div class="validationWarning">Repeats must be at least 1</div>
							) : null}
						</div>
					) : null}

					{props.autoRepeat.enabled() ? (
						<>
							<div class="fieldRow">
								<div class="label">Delay before next question (s)</div>
								<input
									class="input"
									type="number"
									min="5"
									max="120"
									step="1"
									value={props.autoRepeat.delaySeconds()}
									disabled={props.disabled}
									onInput={repeatDelay}
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
									disabled={props.disabled}
									checked={props.answerMode() === "reveal"}
									onInput={() => props.setAnswerMode("reveal")}
								/>
								<span class="segmentedLabel">Click</span>
							</label>
							<label class="segmentedOption">
								<input
									class="segmentedInput"
									type="radio"
									name="answer-mode"
									value="type"
									disabled={props.disabled}
									checked={props.answerMode() === "type"}
									onInput={() => props.setAnswerMode("type")}
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
										disabled={props.disabled}
										checked={props.themeMode() === "dark"}
										onInput={() => {
											props.setThemeMode("dark");
											props.persistThemeMode("dark");
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
										disabled={props.disabled}
										checked={props.themeMode() === "light"}
										onInput={() => {
											props.setThemeMode("light");
											props.persistThemeMode("light");
										}}
									/>
									<span class="segmentedLabel">Light</span>
								</label>
							</div>
						</div>
						<div class="colorGrid" role="radiogroup" aria-label="Color">
							{COLOR_SCHEMES.map((scheme) => {
								const label =
									props.themeMode() === "light"
										? LIGHT_NAMES[scheme]
										: DARK_NAMES[scheme];
								return (
									<label class="colorOption" title={label}>
										<input
											class="segmentedInput"
											type="radio"
											name="color-scheme"
											value={scheme}
											disabled={props.disabled}
											checked={props.colorScheme() === scheme}
											onInput={() => {
												props.setColorScheme(scheme);
												props.persistColorScheme(scheme);
											}}
										/>
										<span
											classList={{
												colorSwatch: true,
												[`sw-preview-${scheme}`]: true,
											}}
											aria-hidden="true"
										/>
										<span class="colorLabel">{label}</span>
									</label>
								);
							})}
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
									disabled={props.disabled}
									checked={!props.allowNegativeNumbers()}
									onInput={() => props.setAllowNegativeNumbers(false)}
								/>
								<span class="segmentedLabel">Off</span>
							</label>
							<label class="segmentedOption">
								<input
									class="segmentedInput"
									type="radio"
									name="allow-negative-numbers"
									value="on"
									disabled={props.disabled}
									checked={props.allowNegativeNumbers()}
									onInput={() => props.setAllowNegativeNumbers(true)}
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
						onClick={props.close}
						disabled={props.disabled}
					>
						Close
					</button>
				</div>
			</div>
		</div>
	);
}
