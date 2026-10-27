-- Compiled with roblox-ts v3.0.0
-- @probe elide
local function probeElide()
	return "elide"
end
-- @probe keep
local function probeKeep()
	return "keep"
end
local helper = 2
return {
	probeElide = probeElide,
	probeKeep = probeKeep,
	helper = helper,
}
