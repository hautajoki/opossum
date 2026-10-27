return {
	calls = {
		{ call = "forOf", expect = { "case 1", "after 1", "case 3", "after 3" } },
		{ call = "middleDefault", expect = { "one", "after 1", "three", "after 3" } },
		{ call = "nested", expect = { "odd 1", "after 1", "odd 3", "after 3", "even 4", "after 4" } },
		{ call = "loopInSwitch", expect = { "y 1", "y 3", "after 1" } },
		{ call = "forAndDo", expect = { "for 1", "for 3", "do 2", "do 3" } },
		{ call = "tryInSwitch", args = { { 1, 2, 3 } }, label = "({1, 2, 3})", expect = { "after 1", "finally 2", "after 3" } },
		{ call = "switchInTry", expect = { "in 1", "after 1", "in 3", "after 3" } },
		{
			call = "tryInSwitchInTry",
			expect = { "in 1", "outer fin 1", "after 1", "fin 2", "outer fin 2", "in 3", "outer fin 3", "after 3" },
		},
		{ call = "switchInTryInSwitch", expect = { "after 1", "fin 2", "in 3", "fin 3", "after 3" } },
		{ call = "deep", expect = { "fin 1", "after 1", "inner fin 2", "fin 2", "fin 3", "after 3" } },
	},
}
