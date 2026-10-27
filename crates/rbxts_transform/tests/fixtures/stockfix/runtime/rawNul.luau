return {
	calls = {
		{ call = "rawNul", expect = { "a\x00b", "c\x001", "d\x001e\x00", "\x00" } },
	},
}
