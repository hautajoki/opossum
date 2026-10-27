// Hand-written JSX fixture: a small React-like factory, imported as `React` like `@rbxts/react` (`export =`).

namespace React {
	export interface Element {
		tag: unknown;
		props: unknown;
		children: Array<unknown>;
	}

	export function createElement(tag: unknown, props?: unknown, ...children: Array<unknown>): Element {
		return { tag, props, children };
	}

	export function Fragment(props: { children?: unknown }): Element {
		return { tag: "Fragment", props, children: [] };
	}

	export namespace JSX {
		export type Element = React.Element;
		export interface ElementChildrenAttribute {
			children: {};
		}
		export interface IntrinsicAttributes {
			Key?: string | number;
		}
		export interface IntrinsicElements {
			[tag: string]: { [prop: string]: unknown };
		}
	}
}

export = React;
