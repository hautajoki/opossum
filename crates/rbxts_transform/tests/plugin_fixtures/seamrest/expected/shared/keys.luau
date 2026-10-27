-- Compiled with roblox-ts v3.0.0
-- Computed keys a plugin rewrote (probe's `property_key`): an argument that only reads is dropped with the key, one
-- with effects still runs once, after the receiver.
local DK = {
	A = "a",
}
local function enumKey()
	return dropKeyed.k_a
end
local function callKey()
	local _exp = getDropKeyed()
	keyName()
	return _exp.k_a
end
local function readKey()
	local _ = dropKeyed.k_s
	return dropKeyed.k_a
end
local function writeKey()
	getDropKeyed().k_a = 2
	return dropKeyed.k_a
end
return {
	enumKey = enumKey,
	callKey = callKey,
	readKey = readKey,
	writeKey = writeKey,
}
