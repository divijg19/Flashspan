import type { JSX } from "solid-js";

/**
 * The auto-repeat countdown shown on the end screen while a repeat is pending.
 *
 * Its own component because it is rendered in both answer modes: the ramp's
 * duration has to be identical in both places or one of the bars animates
 * differently. It was inlined in `App.tsx` and exported from there for its unit
 * test, which meant the test imported the whole app to reach a presentational
 * component.
 */
export function AutoRepeatBar(props: {
	secondsLeft: number;
	remaining: number;
	fillMs: number;
	onCancel: () => void;
}): JSX.Element {
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
