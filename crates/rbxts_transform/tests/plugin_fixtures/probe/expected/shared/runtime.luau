-- Compiled with roblox-ts v3.0.0
local function impl(b, a)
	return a + #b
end
local other = 1
return {
	impl = impl,
	other = other,
}
