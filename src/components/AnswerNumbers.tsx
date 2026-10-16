import { For, type JSX, Show } from "solid-js";

/**
 * The revealed number series, listed in order.
 *
 * Written out in full in both answer modes on the end screen. The modes differ
 * in how the answer is collected, but once the answer exists the list is
 * identical — same markup, same gate, same place in the layout — so a change to
 * it (a row class, a separator, a count) had to be made twice with no test
 * between the copies.
 *
 * It deliberately lives *inside* the answer body rather than beside it: that
 * body is the flex column that centres the card, so hoisting this out of it
 * would move it in the layout.
 *
 * `visible` is an accessor, not a boolean, and the gate is a `<Show>` rather
 * than a bare ternary over the prop. A plain boolean is read once when the
 * component's body runs, and the body is not itself reactive, so the gate would
 * be frozen at whatever it was when the end screen mounted — here, always
 * `false`. The list then never appeared, however many times "Show numbers" was
 * pressed: only the button's label changed. `numbers` is an accessor for the
 * same reason, which is why `<For>` already worked.
 */
export function AnswerNumbers(props: {
	visible: () => boolean;
	numbers: () => number[];
}): JSX.Element {
	return (
		<Show when={props.visible()}>
			<div class="answerNumbers">
				<For each={props.numbers()}>
					{(n, idx) => (
						<div class="answerRow">
							<div class="answerIndex">{idx() + 1}</div>
							<div class="answerValue">{n}</div>
						</div>
					)}
				</For>
			</div>
		</Show>
	);
}
