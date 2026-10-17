import { render } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { describe, expect, it } from "vitest";
import { AnswerNumbers } from "../components/AnswerNumbers";

/**
 * The revealed number series must actually appear when its gate opens.
 *
 * The list was extracted from the end screen into `AnswerNumbers`, and the
 * extraction turned a reactive `<memo>` over `hasValidated() && showNumbersList()`
 * into a component that read `props.visible` once in a bare `return` ternary.
 * A component body is not a reactive scope, so the gate froze at the value it
 * had on mount — always `false` — and the list never rendered however many
 * times "Show numbers" was pressed. Only the button's label changed.
 *
 * `visible` is therefore an accessor and the gate is a `<Show>`, not a ternary
 * over a plain boolean. These two cases fail on the frozen version: the list
 * cannot open, and cannot close once it has.
 */
describe("AnswerNumbers", () => {
	it("renders the list when its gate flips from closed to open", async () => {
		const [visible, setVisible] = createSignal(false);
		const { container } = render(() => (
			<AnswerNumbers visible={visible} numbers={() => [1, 2, 3]} />
		));

		expect(container.querySelector(".answerNumbers")).toBeNull();

		setVisible(true);
		await Promise.resolve();

		expect(container.querySelectorAll(".answerRow")).toHaveLength(3);
	});

	it("hides the list again when the gate closes", async () => {
		const [visible, setVisible] = createSignal(true);
		const { container } = render(() => (
			<AnswerNumbers visible={visible} numbers={() => [4]} />
		));

		expect(container.querySelectorAll(".answerRow")).toHaveLength(1);

		setVisible(false);
		await Promise.resolve();

		expect(container.querySelector(".answerNumbers")).toBeNull();
	});
});
