import { For, type JSX, Show } from "solid-js";

/**
 * The revealed number series, listed in order. Lives inside the answer body so
 * the flex layout centres it. `visible` and `numbers` are accessors and the
 * gate is `<Show>`: a plain value would be read once at mount and the list
 * would never appear.
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
