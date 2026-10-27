-- Compiled with roblox-ts v3.0.0
-- A raw NUL character written in string and template literals (this file holds NUL bytes). roblox-ts copied it
-- into the Luau source, which Luau rejects ("Malformed string"); rbxts-rs writes `\x00`.
local function rawNul()
	local n = 1
	return { "a\x00b", 'c\x001', `d\x00{n}e\x00`, `\x00` }
end
return {
	rawNul = rawNul,
}
