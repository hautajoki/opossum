return {
	calls = {
		{
			call = "componentTag",
			expect = function(element)
				return type(element.tag) == "function"
			end,
		},
		{
			call = "dashedTag",
			expect = function(element)
				return element.tag == "custom-frame"
			end,
		},
		{
			call = "attributeStrings",
			expect = function(element)
				local p = element.props
				return p.Text == "a\\nb & c"
					and p.Alt == "it's"
					and p.Path == "C:\\temp"
					and p.Lines == "one\ntwo"
					and p.Plain == "single"
			end,
		},
		{
			call = "attributesBeforeChildren",
			expect = function(element)
				return element.props.Size == 1
			end,
		},
		{ call = "tagBeforeAttributes", expect = { true, false } },
	},
}
