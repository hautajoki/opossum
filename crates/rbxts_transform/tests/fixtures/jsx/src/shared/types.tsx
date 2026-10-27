// Hand-written JSX fixture: a React import used only as a type is still elided in a .tsx file without JSX.
import React from "shared/react";

export type Element = React.Element;

export function describe(element: Element) {
	return element.tag;
}
