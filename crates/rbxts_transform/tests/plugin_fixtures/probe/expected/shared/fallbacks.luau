-- Compiled with roblox-ts v3.0.0
local seen = 0
local v1 = "void claimed"
local function f()
	local _ = seen
	local _1 = nil
	print("probe: debugger")
	return seen
end
return {
	v1 = v1,
	f = f,
}
