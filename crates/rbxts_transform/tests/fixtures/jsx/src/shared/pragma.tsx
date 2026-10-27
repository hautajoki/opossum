/** @jsx h */
/** @jsxFrag Frag */
// Hand-written JSX fixture: a file's own JSX factory and fragment factory from pragmas.

interface Rendered {
	tag: unknown;
	props: unknown;
	children: Array<unknown>;
}

function h(tag: unknown, props: unknown, ...children: Array<unknown>): Rendered {
	return { tag, props, children };
}

const Frag = "Frag";

declare namespace h {
	namespace JSX {
		type Element = Rendered;
		interface IntrinsicElements {
			[tag: string]: { [prop: string]: unknown };
		}
	}
}

export = () => {
	const element = <text value={1}>text</text>;
	const fragment = (
		<>
			<text />
		</>
	);
	return [element, fragment];
};
