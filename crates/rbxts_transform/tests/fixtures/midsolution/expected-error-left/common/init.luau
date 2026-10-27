-- Compiled with roblox-ts v3.0.0
local exports = {}
local function make(size)
	return {
		kind = "a",
		size = size,
	}
end
local function part()
	return Instance.new("Part")
end
exports.count = 0
local function bump()
	exports.count += 1
	return exports.count
end
exports.make = make
exports.part = part
exports.bump = bump
return exports
