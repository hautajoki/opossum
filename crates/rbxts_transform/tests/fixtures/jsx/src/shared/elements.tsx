// Hand-written JSX fixture: elements, attributes, spread props, fragments and children.
// `React` is used only by JSX here, so its import must stay (the checker marks the JSX factory as referenced).
import React from "shared/react";

declare function getProps(): { Size: number; Visible: boolean };
declare function getMaybeProps(): { Size: number } | undefined;
declare function getName(): string;
declare const props: { Size: number; Visible: boolean };
declare const maybeProps: { Size: number } | undefined;
declare const enabled: boolean;
declare const count: number;
declare const label: string;
declare const items: Array<string>;
declare const elements: Array<React.Element>;

interface ButtonProps {
	Text: string;
	Order?: number;
	children?: unknown;
}

const Components = {
	Panel: (panelProps: { Title: string; children?: unknown }) => <frame Name={panelProps.Title} />,
};

const _Inner = (innerProps: {}) => <frame />;

let counter = 0;

// function declarations first: TypeScript 5 lists them first among the exports, TypeScript 7 in source order
export function Button(buttonProps: ButtonProps) {
	return (
		<textbutton Text={buttonProps.Text} LayoutOrder={buttonProps.Order}>
			{buttonProps.children}
		</textbutton>
	);
}

export function List() {
	// children arrays from map, keyed
	return <frame>{items.map((item, i) => <textlabel Key={item} Text={item} LayoutOrder={i} />)}</frame>;
}

export function Conditional() {
	// conditional children
	return (
		<frame>
			{enabled && <textlabel Text="on" />}
			{enabled ? <textlabel Text="yes" /> : <textlabel Text="no" />}
			{count > 0 && label !== "" && <textlabel Text={label} />}
			{label !== "" && <textlabel Text={label} />}
			{maybeProps !== undefined ? <frame Size={maybeProps.Size} /> : undefined}
		</frame>
	);
}

// host and component tags
export const empty = <frame />;
export const host = <textlabel Text="hello" />;
export const component = <Button Text="press" />;
export const componentWithChildren = (
	<Button Text="outer">
		<textlabel Text="inner" />
	</Button>
);
export const dotted = <Components.Panel Title="panel" />;
export const nested = (
	<screengui>
		<frame Size={1}>
			<textlabel Text="deep" />
		</frame>
	</screengui>
);

// attributes
export const booleanAttribute = <textbutton AutoButtonColor Active={false} />;
export const stringAttributes = <textlabel Text='say "hi"' Name="it's" Font="a\b" />;
export const expressionAttributes = <frame Size={count * 2} Name={`n${count}`} Visible={enabled} />;
export const namespacedAttribute = <frame custom:value={1} />;
export const namespacedTag = <ns:frame />;
// roblox-ts reads a tag starting with `_` as a host component, though TypeScript checks it as a value
export const underscoreTag = <_Inner />;
export const elementAttribute = <frame Child={<textlabel />} />;

// attributes with prerequisites end the inline table
export const attributePrereqs = <frame LayoutOrder={counter++} Size={1} ZIndex={counter++} />;
export const attributeCall = <frame Name={getName()} Size={1} />;

// spread props
export const spreadFirst = <frame {...props} LayoutOrder={1} />;
export const spreadOnly = <frame {...props} />;
export const spreadAfter = <frame LayoutOrder={1} {...props} />;
export const spreadCall = <frame {...getProps()} />;
export const spreadMaybe = <frame {...maybeProps} LayoutOrder={2} />;
export const spreadMaybeCall = <frame LayoutOrder={2} {...getMaybeProps()} />;
export const spreadConditional = <frame ZIndex={2} {...(enabled && { LayoutOrder: 3 })} />;
export const spreadTwice = <frame {...props} {...maybeProps} Key="twice" />;
export const spreadObject = <frame {...{ Size: 1 }} />;

// children
export const keyedChildren = (
	<frame>
		<textlabel Key="First" Text="1" />
		<textlabel Key="Second" Text="2" />
	</frame>
);
export const arrayChild = <frame>{elements}</frame>;
export const spreadChild = <frame>{...elements}</frame>;
export const mixedChildren = (
	<frame>
		<textlabel Text="before" />
		{items.map((item) => (
			<textlabel Key={item} Text={item} />
		))}
		{...elements}
	</frame>
);
export const emptyExpressions = (
	<frame>
		{}
		{/* a comment */}
		<textlabel />
	</frame>
);
export const childPrereqs = (
	<frame Size={1}>
		<textlabel LayoutOrder={counter++} />
		{getName()}
		<textlabel LayoutOrder={counter++} />
	</frame>
);
export const childValue = <frame>{label}</frame>;

// fragments
export const emptyFragment = <></>;
export const fragment = (
	<>
		<frame />
		<textlabel Text="two" />
	</>
);
export const fragmentPrereqs = (
	<>
		<frame LayoutOrder={counter++} />
	</>
);
export const keyedFragment = (
	<React.Fragment Key="keyed">
		<frame />
	</React.Fragment>
);
