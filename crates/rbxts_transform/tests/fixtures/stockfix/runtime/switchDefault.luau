return {
	calls = {
		{ call = "middle", args = { 1 }, label = "(1)", expect = { "one", "default", "two" } },
		{ call = "middle", args = { 2 }, label = "(2)", expect = { "two" } },
		{ call = "middle", args = { 3 }, label = "(3)", expect = { "three" } },
		{ call = "middle", args = { 4 }, label = "(4)", expect = { "default", "two" } },
		{ call = "first", args = { 1 }, label = "(1)", expect = { "one or two" } },
		{ call = "first", args = { 2 }, label = "(2)", expect = { "one or two" } },
		{ call = "first", args = { 3 }, label = "(3)", expect = { "default" } },
		{ call = "testedOnce", args = { 1 }, label = "(1)", expect = { "test 1", "one" } },
		{ call = "testedOnce", args = { 2 }, label = "(2)", expect = { "test 1", "test 2", "two" } },
		{ call = "testedOnce", args = { 3 }, label = "(3)", expect = { "test 1", "test 2", "prereq", "three" } },
		{ call = "testedOnce", args = { 4 }, label = "(4)", expect = { "test 1", "test 2", "prereq", "default" } },
	},
}
