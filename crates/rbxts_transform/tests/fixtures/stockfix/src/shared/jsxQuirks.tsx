// JSX that roblox-ts lowered differently from TypeScript's JSX semantics.
import React from "shared/react";

const _Inner = (props: {}) => <frame />;

// a tag whose name starts with `_` is the component of that name (TypeScript's `isIntrinsicJsxName`), not a
// host element named by a string
export function componentTag() {
	return <_Inner />;
}

// a dashed name is a host element, whatever its first letter (roblox-ts failed an assertion on `<Custom-frame />`,
// which would stop the whole build of stock roblox-ts here)
export function dashedTag() {
	return <custom-frame />;
}

// attribute strings have no escapes, decode HTML entities, and may span lines
export function attributeStrings() {
	return (
		<textlabel
			Text="a\nb &amp; c"
			Alt="it&apos;s"
			Path="C:\temp"
			Lines="one
two"
			Plain='single'
		/>
	);
}

let size = 1;
function bump(): undefined {
	size = 2;
	return undefined;
}
const holder: { bump(): undefined } | undefined = { bump: () => bump() };

// the attributes are evaluated before the children
export function attributesBeforeChildren() {
	size = 1;
	return <frame Size={size}>{holder?.bump()}</frame>;
}

const First = (props: { Value?: unknown }) => <frame />;
const Second = (props: { Value?: unknown }) => <frame />;
let Tag = First;

// the tag is evaluated before the attributes
export function tagBeforeAttributes() {
	Tag = First;
	const element = <Tag Value={holder?.bump() ?? ((Tag = Second), 1)} />;
	return [element.tag === First, element.tag === Second];
}
