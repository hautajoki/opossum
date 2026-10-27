-- Compiled with roblox-ts v3.0.0
-- Tagged templates pass the tag the cooked strings. roblox-ts wrote the cooked text into the Luau source as it is:
-- a backslash became an escape, a leading newline was dropped by a long bracket, and an invalid escape (whose
-- cooked string is undefined) gave invalid Luau.
local function show(strings, ...)
	local values = { ... }
	local out = {}
	for _, s in strings do
		table.insert(out, s)
	end
	return out
end
local function backslash()
	return show({ "a\\b", "\\" }, 1)
end
local function leadingNewline()
	return show({ "\nlead", "\r\n" }, 1)
end
-- the same spelling as roblox-ts's: no escape needed
local function plain()
	return show({ [[tab	nl
end]], [[q"']], "]]==]" }, 1, 2)
end
local function cooked(strings, ...)
	local values = { ... }
	return { strings[1] == nil, strings[2] == nil, strings[3] }
end
local function invalidEscape()
	return cooked({ nil, nil, "fine" }, 1, 2)
end
return {
	backslash = backslash,
	leadingNewline = leadingNewline,
	plain = plain,
	invalidEscape = invalidEscape,
}
