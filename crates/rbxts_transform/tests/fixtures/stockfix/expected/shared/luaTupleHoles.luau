-- Compiled with roblox-ts v3.0.0
-- Holes in an optimized LuaTuple (or array literal) assignment. roblox-ts never declared the holes' temporaries, so
-- `[, q, , p] = tup()` wrote the globals `_` and `_1`.
local function tupleHoles()
	local q = 0
	local p = 0
	local _, _1
	_, q, _1, p = tup()
	return { q, p }
end
local function literalHoles()
	local a = 0
	local _
	_, a = (tup()), 5
	return a
end
return {
	tupleHoles = tupleHoles,
	literalHoles = literalHoles,
}
