-- Compiled with roblox-ts v3.0.0
-- an expression-kind hook compiles its own call (or `new`); the call plugins still claim it, inside
local a = exprWrap(wrap(probeExprSelf(sideEffect())))
local b = exprWrap(wrap(probeExprInOrder(1)))
local c = exprWrap(ProbeExprWidget.new("probed", 5))
return {
	a = a,
	b = b,
	c = c,
}
