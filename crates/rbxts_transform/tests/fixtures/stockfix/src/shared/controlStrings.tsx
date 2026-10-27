// Strings whose values hold a NUL or a carriage return. Luau rejects a raw NUL in every kind of string and a raw
// carriage return between quotes ("Malformed string"), and reads a backslash as an escape. roblox-ts wrote decoded
// values (a tagged template's cooked strings, enum values, JSX entities) into the source as they were.
import React from "shared/react";

function show(strings: TemplateStringsArray, ...values: unknown[]) {
	const out: string[] = [];
	for (const s of strings) out.push(s);
	return out;
}

// a cooked NUL, also before a digit (`\x00` then `1`, not one escape)
export function taggedNul() {
	return show`a\0b${1}\x001`;
}

// written escapes stay escapes: the same spelling as roblox-ts's
export function literalNul() {
	return ["a\0b", "\x001", `c\0${1}d`];
}

enum Escaped {
	Backslash = "a\\b",
	Nul = "c\0d",
	Return = "e\rf",
}

export function enumValues() {
	return [Escaped.Backslash, Escaped.Nul, Escaped.Return];
}

const enum Inlined {
	Backslash = "x\\y",
	Nul = "\0",
}

// a const enum's members are inlined as their values
export function constEnumValues() {
	return [Inlined.Backslash, Inlined.Nul];
}

// JSX entities decode to the characters they name, NUL and carriage return included
export function jsxEntities() {
	return <textlabel Text="a&#0;b" Other="c&#13;d">e&#0;f</textlabel>;
}
