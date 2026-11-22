import type { JSX } from "solid-js";
import { AutoRepeatBar } from "./AutoRepeatBar";

/**
 * Shared tail of the end screen: auto-repeat countdown and footer. Props are
 * accessors (and `footerLead` a thunk) so they stay reactive after mount.
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
