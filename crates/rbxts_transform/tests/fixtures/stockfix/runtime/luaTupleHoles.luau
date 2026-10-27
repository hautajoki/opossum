return {
	ambient = {
		tup = function()
			return 1, 2, 3, 4
		end,
	},
	calls = {
		{ call = "tupleHoles", expect = { 2, 4 } },
		{ call = "literalHoles", expect = 5 },
	},
}
