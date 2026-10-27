return {
	calls = {
		{ call = "reassigned", expect = { "A", "arg", "B" } },
		{ call = "plain", args = { "x" }, label = "(x)", expect = "x" },
	},
}
