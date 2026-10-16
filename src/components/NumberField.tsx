import type { JSX } from "solid-js";

/**
 * A labelled numeric setting: a text box and a slider bound to one signal.
 *
 * The home screen had three of these — digits, duration, total — written out
 * in full each time. They differed only in label, bounds, step, accessor,
 * setter and the fallback used when the box is emptied, and they carried six
 * copies of the same empty-input guard between them.
 *
 * `fallback` is why this takes the raw `valueAsNumber` handling rather than a
 * number: an emptied number input reports `NaN`, and assigning that straight
 * into the session config would ship `NaN` to the planner.
 */
export interface NumberFieldProps {
	label: string;
	value: () => number;
	setValue: (value: number) => void;
	min: number;
	max: number;
	step: number;
	/** Used when the input is empty or reports a non-finite value. */
	fallback: number;
	disabled: () => boolean;
}

/**
 * Reads a numeric input, falling back when the field is empty or malformed.
 *
 * Exported so the advanced panel's own numeric controls share this one guard:
 * an emptied number input reports `NaN`, and assigning that straight to a signal
 * would put `NaN` into the session config.
 */
export function numericValue(value: number, fallback: number): number {
	return Number.isFinite(value) ? value : fallback;
}

export function NumberField(props: NumberFieldProps): JSX.Element {
	const read = (event: { currentTarget: HTMLInputElement }): void =>
		props.setValue(
			numericValue(event.currentTarget.valueAsNumber, props.fallback),
		);

	return (
		<div class="field">
			<div class="fieldRow">
				<div class="label">{props.label}</div>
				<input
					class="input"
					type="number"
					min={props.min}
					max={props.max}
					step={props.step}
					value={props.value()}
					disabled={props.disabled()}
					onInput={read}
				/>
			</div>
			<input
				class="range"
				type="range"
				min={props.min}
				max={props.max}
				step={props.step}
				value={props.value()}
				disabled={props.disabled()}
				onInput={read}
			/>
		</div>
	);
}
