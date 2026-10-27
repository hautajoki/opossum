-- Compiled with roblox-ts v3.0.0
local _hoisted2 = 1
-- a hoist is declared at the top of the file: a local of the same name in a nested function must not capture it
local function f()
	local _hoisted = 5
	return _hoisted2 + _hoisted
end
local _hoisted2_1 = 2
local function g()
	local _hoisted_1 = 6
	return _hoisted2_1 + _hoisted_1
end
return {
	f = f,
	g = g,
}
