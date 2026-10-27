-- Compiled with roblox-ts v3.0.0
-- the expression hook (probe-exprcall) sees calls inside chains: before anything of the chain is compiled, or,
-- after a `?.` link, inside the nil check on its compiled base
local c1 = exprWrap(probeExprChain(1)).value
local _c2 = maybeExprs
if _c2 ~= nil then
	_c2 = exprWrap(_c2:probeExprM(2)).value
end
local c2 = _c2
-- assignments the hook lowers itself: as a statement, or as a value
probeTarget.x = 1
probeTarget.x += 1
probeTarget.x = 2
local c3 = (exprWrap(probeTarget.x))
-- asked once: declined by the expression hook, then the call hook compiles the call itself
local c4 = wrap(probeAskOnce(4))
return {
	c1 = c1,
	c2 = c2,
	c3 = c3,
	c4 = c4,
}
