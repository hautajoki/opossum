-- Compiled with roblox-ts v3.0.0
local TS = require(game:GetService("ReplicatedStorage"):WaitForChild("include"):WaitForChild("RuntimeLib"))
-- Hand-written T2 exprs fixture: delete, await, tagged templates, import().
local function deletes(bag)
	bag.x = nil
	bag[bagKey()] = nil
	bag.y = nil
	local removed = true
	return removed
end
local awaits = TS.async(function(p)
	local a = TS.await(p)
	local b = TS.await(p)
	TS.await(p)
	return a + b
end)
local function tagged(a, b)
	local plain = tag({ "plain text" })
	local withValues = tag({ "a ", " b ", " end" }, a, b)
	local escapes = tag({ [[line
next A `tick`]] })
	local member = tags.upper({ "x", "" }, a)
	local called = getTag()({ "y", "z" }, b)
	local i = 0
	local _exp = i
	local _original = i
	i += 1
	local ordered = tag({ "", " ", " ", "" }, _exp, _original, i)
	return plain .. withValues .. escapes .. member .. called .. ordered
end
local dynamicImport = TS.async(function()
	local promise = TS.Promise.new(function(resolve)
		resolve(TS.import(script, game:GetService("ReplicatedStorage"), "shared", "conditional"))
	end)
	local mod = TS.await(TS.Promise.new(function(resolve)
		resolve(TS.import(script, game:GetService("ReplicatedStorage"), "shared", "generators"))
	end))
	print(promise, mod)
end)
return {
	deletes = deletes,
	awaits = awaits,
	tagged = tagged,
	dynamicImport = dynamicImport,
}
