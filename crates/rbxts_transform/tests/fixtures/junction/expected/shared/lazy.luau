-- Compiled with roblox-ts v3.0.0
local TS = require(game:GetService("ReplicatedStorage"):WaitForChild("include"):WaitForChild("RuntimeLib"))
-- A dynamic import of a linked package.
local load = TS.async(function()
	local greeter = TS.await(TS.Promise.new(function(resolve)
		resolve(TS.import(script, game:GetService("ReplicatedStorage"), "include", "node_modules", "@rbxts", "greeter"))
	end))
	return greeter.greet("lazy")
end)
return {
	load = load,
}
