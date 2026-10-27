return {
	calls = {
		{ call = "taggedNul", expect = { "a\x00b", "\x001" } },
		{ call = "literalNul", expect = { "a\x00b", "\x001", "c\x001d" } },
		{ call = "enumValues", expect = { "a\\b", "c\x00d", "e\rf" } },
		{ call = "constEnumValues", expect = { "x\\y", "\x00" } },
		{
			call = "jsxEntities",
			expect = function(element)
				return element.props.Text == "a\x00b" and element.props.Other == "c\rd" and element.children[1] == "e\x00f"
			end,
		},
	},
}
