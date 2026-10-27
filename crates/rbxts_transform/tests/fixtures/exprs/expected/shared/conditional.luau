-- Compiled with roblox-ts v3.0.0
-- Hand-written T2 exprs fixture: conditional expressions.
local function plain(flag, a, b)
	return if flag then a else b
end
-- truthiness checks on a number or string condition
local function truthy(n, s)
	local x = if n ~= 0 and n == n and n then "yes" else "no"
	local y = if s ~= "" and s then 1 else 2
	return x .. tostring(y)
end
-- a branch with prerequisites becomes an if statement and a temporary
local function withPrereqs(flag, n)
	local i = n
	local _result
	if flag then
		local _original = i
		i += 1
		_result = _original
	else
		local _original = i
		i -= 1
		_result = _original
	end
	local v = _result
	return v + i
end
-- used as a statement
local function asStatement(flag)
	if flag then
		sideEffect()
	else
		check()
	end
	local count = 0
	if flag then
		local _original = count
		count += 1
	else
		local _original = count
		count -= 1
	end
	return count
end
-- nested conditionals
local function nested(a, b)
	return if a then (if b then 1 else 2) elseif b then 3 else 4
end
-- condition with prerequisites
local function conditionPrereqs(n)
	local i = n
	local _original = i
	i += 1
	return if _original > 2 then i else -i
end
local function inCall(flag)
	print(if flag then "a" else "b", sideEffect())
end
return {
	plain = plain,
	truthy = truthy,
	withPrereqs = withPrereqs,
	asStatement = asStatement,
	nested = nested,
	conditionPrereqs = conditionPrereqs,
	inCall = inCall,
}
