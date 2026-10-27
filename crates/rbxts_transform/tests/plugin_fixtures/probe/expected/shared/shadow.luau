-- Compiled with roblox-ts v3.0.0
local TS = require(game:GetService("ReplicatedStorage"):WaitForChild("include"):WaitForChild("RuntimeLib"))
local impl_1 = TS.import(script, game:GetService("ReplicatedStorage"), "shared", "runtime").impl
local _runtime = TS.import(script, game:GetService("ReplicatedStorage"), "shared", "runtime")
local impl = _runtime.impl
local other = _runtime.other
local a = impl("a", other)
-- `impl` would mean the parameter or the local here: the import is not reused
local function shadowed(impl)
	return impl_1(impl, 3)
end
local function withLocal()
	local impl = "x"
	return impl_1(impl, 4)
end
-- the new import's temporary must not be captured by a local named like it
local function withTemp(impl)
	local _impl = 3
	return impl_1("s", impl) + _impl
end
return {
	a = a,
	shadowed = shadowed,
	withLocal = withLocal,
	withTemp = withTemp,
}
