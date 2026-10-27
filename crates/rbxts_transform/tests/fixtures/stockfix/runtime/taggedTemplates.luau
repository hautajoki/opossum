return {
	calls = {
		{ call = "backslash", expect = { "a\\b", "\\" } },
		{ call = "leadingNewline", expect = { "\nlead", "\r\n" } },
		{ call = "plain", expect = { "tab\tnl\nend", "q\"'", "]]==]" } },
		{ call = "invalidEscape", expect = { true, true, "fine" } },
	},
}
