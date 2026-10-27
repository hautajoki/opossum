-- Compiled with roblox-ts v3.0.0
local TS = require(game:GetService("ReplicatedStorage"):WaitForChild("include"):WaitForChild("RuntimeLib"))
-- A header comment with a blank line after it: TypeScript's printer keeps it above the plugin imports.
local impl_1 = TS.import(script, game:GetService("ReplicatedStorage"), "shared", "runtime").impl
-- the first statement's own comment stays with it
local impl = 1
local constant_22 = 2
--[[
	@param jdOnlyDoc a JSDoc parameter name, and a link: {@link jdLinked}
	@jdTag
]]
local function described(n)
	return n + impl + constant_22
end
local literals = {
	litKey = 1,
	litQuoted = 2,
}
local e1 = literals.litIndex
local e2 = literals.litQuoted
-- `impl` and `constant_22` are taken: `impl_1`, `constant_22_1`
local swapped = impl_1("a", 1)
local constant_22_1 = { 1 }
local named = constant_22_1
local runtime = TS.Promise.is
local synth = { "^\u{00E9}.*\u{2713}$", "it's \"q\" \\ /", "a\nb\r\tc\v\f\b", "\0x\x007\u{001B}", "\u{1f600}\u{2028}\u{2029}\u{0085}" }
local idents = "jdLinked,jdOnlyDoc,jdTag,litKey,litQuoted,literals"
-- with plugins, a literal with separators is its value (as TypeScript's printer reprints it)
local big = 1000000
local hex = 65535
local plain = 0xff
local nums = { 1000.5, 10000000000, 170, 63, 123456789012345680000, 0.000001 }
local _ = nil
local props = "c,y,w,x"
return {
	described = described,
	e1 = e1,
	e2 = e2,
	swapped = swapped,
	named = named,
	runtime = runtime,
	synth = synth,
	idents = idents,
	big = big,
	hex = hex,
	plain = plain,
	nums = nums,
	props = props,
}
