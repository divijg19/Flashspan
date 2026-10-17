/**
 * The one web-side formatter for the validation message.
 *
 * Desktop formats the same text in Rust (`grading_message` in
 * `src-tauri/src/main.rs`) and ships it as `SubmitAnswerResponse.message`; the
 * UI renders whatever the runtime sent instead of rebuilding it from the
 * numeric fields. That leaves the Rust copy and this one as the only two, kept
 * in step by the Rust `grading_message_*` tests and the browser validation
 * test rather than by a third hand-copied branch in `App.tsx`.
 */
export function formatValidationMessage(
	correct: boolean,
	expectedSum: number,
	delta: number,
): string {
	if (correct) {
		return `Correct ✅\nExpected answer: ${expectedSum}`;
	}
	return `Incorrect\nExpected answer: ${expectedSum}\nDifference: ${delta > 0 ? "+" : ""}${delta}`;
}
