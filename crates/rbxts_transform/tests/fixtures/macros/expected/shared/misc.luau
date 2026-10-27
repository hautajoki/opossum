-- Compiled with roblox-ts v3.0.0
local TS = require(game:GetService("ReplicatedStorage"):WaitForChild("include"):WaitForChild("RuntimeLib"))
-- call macros, string and math macros, Promise
local function callMacros(value, inst, n, s, maybe)
	assert(n ~= 0 and n == n and n)
	assert(s ~= "" and s, "message")
	assert(inst)
	assert(maybe)
	print(typeof(value), type(value) == "number", typeof(value) == "Vector3", inst.ClassName == "Part")
	print(n, s)
	if type(value) == "string" then
		print(string.upper(value))
	end
end
local function strings(s, n, getString)
	print(#s, { string.byte(s, 1) }, { string.find(s, "a") }, string.format(s, n), string.gmatch(s, "%d"), { string.gsub(s, "a", "b") }, string.lower(s))
	print({ string.match(s, "x") }, string.rep(s, 2), string.reverse(s), string.split(s, ","), string.sub(s, 1, 2), string.upper(s))
	print(#getString(), string.sub(getString(), n))
end
local function maths(a, b, cf, u, v2, n)
	print(a + b, a - b, a * 2, a / b, a // 2)
	print(cf * a, cf + a, cf - a, u + u, u - u, v2 * v2, v2 / 2, n // 3)
	print((a + b) * (a - b))
end
local function promises(p)
	local q = TS.Promise.resolve(1)
	p:andThen(function(v)
		return print(v)
	end)
	return q:andThen(function(v)
		return v + 1
	end)
end
local function trees()
	return { game:GetService("ReplicatedStorage"), { "shared", "arrays" } }
end
return {
	callMacros = callMacros,
	strings = strings,
	maths = maths,
	promises = promises,
	trees = trees,
}
