return {
	ambient = {
		maybe = function()
			return nil
		end,
	},
	calls = {
		{ call = "fallthroughCapture", args = { 1 }, label = "(1)", expect = { "user", "two" } },
		{ call = "fallthroughCapture", args = { 2 }, label = "(2)", expect = { "two" } },
		{ call = "fallbackCapture", expect = { 10 } },
		{
			call = "loopVariable",
			args = { { { inner = { value = 1 }, base = 10 }, { base = 20 } } },
			label = "(two rows)",
			expect = { 11, 20 },
		},
	},
}
