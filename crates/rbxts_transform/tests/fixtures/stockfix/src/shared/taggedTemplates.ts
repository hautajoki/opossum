// Tagged templates pass the tag the cooked strings. roblox-ts wrote the cooked text into the Luau source as it is:
// a backslash became an escape, a leading newline was dropped by a long bracket, and an invalid escape (whose
// cooked string is undefined) gave invalid Luau.
function show(strings: TemplateStringsArray, ...values: unknown[]) {
	const out: string[] = [];
	for (const s of strings) out.push(s);
	return out;
}

export function backslash() {
	return show`a\\b${1}\\`;
}

export function leadingNewline() {
	return show`\nlead${1}\r\n`;
}

// the same spelling as roblox-ts's: no escape needed
export function plain() {
	return show`tab\tnl\nend${1}q"'${2}]]==]`;
}

function cooked(strings: TemplateStringsArray, ...values: unknown[]) {
	return [strings[0] === undefined, strings[1] === undefined, strings[2]];
}

export function invalidEscape() {
	return cooked`ok\unicode${1}\xZZ${2}fine`;
}
