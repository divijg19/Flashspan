import type { JSX } from "solid-js";
import { AutoRepeatBar } from "./AutoRepeatBar";

/**
 * The shared tail of the end screen: the auto-repeat countdown and the footer.
 *
 * The two answer modes differ only in how the answer is *collected* — a reveal
 * button versus a typed input — but this part was duplicated verbatim between
 * them. That mattered because the copies could drift: the type mode's footer
 * gained a Validate button and a stricter condition on the numbers toggle
 * while the reveal mode's stayed as it was, and nothing failed when they
 * disagreed.
 *
 * The revealed number list is deliberately *not* here. It sits inside the
 * answer body, after the collected answer, and is centred by that body's flex
 * layout — so it is mode-specific markup with its own condition rather than
 * shared content that merely looks shared.
 *
 * `footerLead` is the one part of the footer that genuinely differs per mode,
 * so it is passed in rather than branching on the mode inside this component.
 *
 * Props are accessors for the same reason as `AdvancedSettings`: a plain value
 * would be read once at render and never update. `footerLead` is a thunk for
 * that same reason — passing an already-evaluated `JSX.Element` left its
 * reactivity to how the compiler happened to wrap the expression, which differed
 * between the two modes' call sites (a fragment versus a ternary) and is exactly
 * the trap `AnswerNumbers` fell into.
 */
export interface SessionEndDetailsProps {
	autoRepeatEnabled: () => boolean;
	hasValidated: () => boolean;
	autoRepeatSecondsLeft: () => number | null;
	autoRepeatRemaining: () => number;
	autoRepeatFillMs: () => number;
	cancelAutoRepeat: () => void;

	/** Leaves the end screen for the home screen. */
	goHome: () => void;

	/** Mode-specific action rendered before "Home", e.g. Validate or Show numbers. */
	footerLead: () => JSX.Element;
}

export function SessionEndDetails(props: SessionEndDetailsProps): JSX.Element {
	return (
		<>
			{props.autoRepeatEnabled() &&
			props.hasValidated() &&
			props.autoRepeatSecondsLeft() != null ? (
				<AutoRepeatBar
					secondsLeft={props.autoRepeatSecondsLeft() ?? 0}
					remaining={props.autoRepeatRemaining()}
					fillMs={props.autoRepeatFillMs()}
					onCancel={props.cancelAutoRepeat}
				/>
			) : null}

			<div class="endFooter">
				<div class="endFooterInner">
					<div class="actionField">
						<div class="centerActions">
							{props.footerLead()}
							<button class="button" type="button" onClick={props.goHome}>
								Home
							</button>
						</div>
					</div>
				</div>
			</div>
		</>
	);
}
