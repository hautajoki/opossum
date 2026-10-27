return {
	calls = {
		{ call = "optionalBooleans", args = { { true, false } }, label = "({ true, false })", expect = "true,false" },
		{ call = "optionalBooleans", args = {}, argCount = 1, label = "(nil)", expect = nil },
		{ call = "optionalStrings", args = { { "a", "b" } }, label = "({ a, b })", expect = "a+b" },
	},
}
