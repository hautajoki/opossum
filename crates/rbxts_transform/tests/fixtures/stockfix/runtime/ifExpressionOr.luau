return {
	calls = {
		{ call = "rangeStep", args = { true }, label = "(true)", expect = { "1", "2", "3" } },
		{ call = "rangeStep", args = { false }, label = "(false)", expect = { "1", "3" } },
	},
}
