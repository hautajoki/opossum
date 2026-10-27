-- Compiled with roblox-ts v3.0.0
local TS = require(game:GetService("ReplicatedStorage"):WaitForChild("include"):WaitForChild("RuntimeLib"))
-- Strings whose values hold a NUL or a carriage return. Luau rejects a raw NUL in every kind of string and a raw
-- carriage return between quotes ("Malformed string"), and reads a backslash as an escape. roblox-ts wrote decoded
-- values (a tagged template's cooked strings, enum values, JSX entities) into the source as they were.
local React = TS.import(script, game:GetService("ReplicatedStorage"), "shared", "react")
local function show(strings, ...)
	local values = { ... }
	local out = {}
	for _, s in strings do
		table.insert(out, s)
	end
	return out
end
-- a cooked NUL, also before a digit (`\x00` then `1`, not one escape)
local function taggedNul()
	return show({ "a\x00b", "\x001" }, 1)
end
-- written escapes stay escapes: the same spelling as roblox-ts's
local function literalNul()
	return { "a\0b", "\x001", `c\0{1}d` }
end
local Escaped = {
	Backslash = "a\\b",
	Nul = "c\x00d",
	Return = "e\rf",
}
local function enumValues()
	return { Escaped.Backslash, Escaped.Nul, Escaped.Return }
end
-- a const enum's members are inlined as their values
local function constEnumValues()
	return { "x\\y", "\x00" }
end
-- JSX entities decode to the characters they name, NUL and carriage return included
local function jsxEntities()
	return React.createElement("textlabel", {
		Text = "a\x00b",
		Other = "c\rd",
	}, "e\x00f")
end
return {
	taggedNul = taggedNul,
	literalNul = literalNul,
	enumValues = enumValues,
	constEnumValues = constEnumValues,
	jsxEntities = jsxEntities,
}
