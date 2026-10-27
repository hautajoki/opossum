return {
	calls = {
		{ call = "keyPrerequisites", args = {}, argCount = 1, label = "(nil)", expect = { -1, 0 } },
		{
			call = "keyPrerequisites",
			args = {
				{
					m = function()
						return 5
					end,
				},
			},
			label = "({ m })",
			expect = { 5, 1 },
		},
		{ call = "keyAfterReceiver", expect = "first" },
	},
}
