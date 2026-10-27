// The capture of `jsxQuirks`' `attributesBeforeChildren` is left out when the later prerequisites can't change what
// was read first: a Roblox datatype constructor (`new Vector2(…)`, `UDim2.fromScale(…)`) only allocates, and an Enum
// item or a datatype constant (`Enum.AutomaticSize.Y`, `Vector2.zero`) is a constant. A user's call or `Instance.new`
// in the same place still captures. (The fixture's tsconfig sets `preserveSymlinks`, so the linked `@rbxts/types`
// counts as `@rbxts/types` and its globals are recognised.)
import React from "shared/react";

const box = { size: 1 };
// `unset ?? x` checks `unset` against nil in a prerequisite (`false` is a value), so the children have prerequisites
const unset: boolean | undefined = undefined;

function resize(): Vector2 {
	box.size = 2;
	return new Vector2(0, 0);
}

// the children's `Vector2.new` can't change `box.size`: the table is built in place
export function pureChild() {
	box.size = 1;
	return <frame Size={box.size}>{unset ?? new Vector2(1, 2)}</frame>;
}

// a user's call can: the table is captured before it, with the size from before the call
export function userCallChild() {
	box.size = 1;
	const element = <frame Size={box.size}>{unset ?? resize()}</frame>;
	return [element.props, box.size];
}

// `Instance.new` is not on the list (a new instance can run code): captured as well
export function instanceChild() {
	box.size = 1;
	return <frame Size={box.size}>{unset ?? new Instance("Part")}</frame>;
}

// constructors in the attributes: nothing the children's call does can change them, so no capture
export function pureAttributes() {
	return (
		<frame Size={new UDim2(0, 1, 0, 2)} Position={UDim2.fromScale(0.5, 0.5)}>
			{unset ?? resize()}
		</frame>
	);
}

// a local named like a datatype is not the datatype: `UDim2.fromScale` here is the user's function
export function shadowed() {
	const UDim2 = {
		fromScale: (x: number, y: number) => {
			box.size = 3;
			return x + y;
		},
	};
	box.size = 1;
	const element = <frame Size={box.size}>{unset ?? UDim2.fromScale(1, 2)}</frame>;
	return [element.props, box.size];
}

// Enum items and datatype constants are constants: no capture before the children's call
export function constants() {
	return (
		<frame AutomaticSize={Enum.AutomaticSize.Y} AnchorPoint={Vector2.zero} CFrame={CFrame.identity}>
			{unset ?? resize()}
		</frame>
	);
}

// a local `Enum` is the user's table, which the children's call changes: captured
export function shadowedEnum() {
	const Enum = { AutomaticSize: { Y: 1 } };
	const grow = () => {
		Enum.AutomaticSize.Y = 2;
		return 0;
	};
	const element = <frame AutomaticSize={Enum.AutomaticSize.Y}>{unset ?? grow()}</frame>;
	return [element.props, Enum.AutomaticSize.Y];
}

// an Enum item reached through a variable is only that variable's member: captured
export function aliasedEnum() {
	const items = Enum.AutomaticSize;
	return <frame AutomaticSize={items.Y}>{unset ?? resize()}</frame>;
}

// a user's augmentation can give a datatype a constructor that reads a table: only signatures `@rbxts/types`
// declares count, so this one captures
type KeypointList = { readonly [index: number]: ColorSequenceKeypoint };
declare global {
	// an inherited constructor: `SharedTableConstructor`'s `new (t?: object)`
	interface ColorSequenceConstructor extends SharedTableConstructor {
		new (keypoints: KeypointList): ColorSequence;
	}
}
const keypoints = [
	new ColorSequenceKeypoint(0, Color3.fromRGB(0, 0, 0)),
	new ColorSequenceKeypoint(1, Color3.fromRGB(255, 255, 255)),
];
const holder = {
	bump: () => {
		keypoints.push(keypoints[0]);
		return 0;
	},
};
const maybeHolder: typeof holder | undefined = holder;

export function augmentedConstructor() {
	const list: KeypointList = keypoints;
	return <frame Seq={new ColorSequence(list)}>{maybeHolder?.bump()}</frame>;
}

// a constructor the type inherits from another `@rbxts/types` interface is not the datatype's own: captured
export function inheritedConstructor() {
	const list: object = keypoints;
	const element = <frame Seq={new ColorSequence(list)}>{maybeHolder?.bump()}</frame>;
	return [element.props, keypoints.size()];
}

const log: string[] = [];
const sideEffect = {
	run: () => {
		log.push("side effect");
		return 0;
	},
};
const maybeSide: typeof sideEffect | undefined = sideEffect;

// a constructor that rejects a value of the right type throws before the children run, as in JavaScript
export function throwingRange() {
	log.clear();
	try {
		<frame Range={new NumberRange(5, 1)}>{maybeSide?.run()}</frame>;
		log.push("built");
	} catch (e) {
		log.push("threw");
	}
	return [...log];
}

export function throwingHex() {
	log.clear();
	try {
		<frame Color={Color3.fromHex("zz")}>{maybeSide?.run()}</frame>;
		log.push("built");
	} catch (e) {
		log.push("threw");
	}
	return [...log];
}
