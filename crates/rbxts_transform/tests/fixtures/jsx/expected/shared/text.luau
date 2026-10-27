-- Compiled with roblox-ts v3.0.0
local TS = require(game:GetService("ReplicatedStorage"):WaitForChild("include"):WaitForChild("RuntimeLib"))
-- Hand-written JSX fixture: JSX text, its whitespace rule and HTML entities.
local React = TS.import(script, game:GetService("ReplicatedStorage"), "shared", "react")
local single = React.createElement("textlabel", nil, "Hello world")
local padded = React.createElement("textlabel", nil, " spaced out ")
-- prettier-ignore
local multiLine = (React.createElement("textlabel", nil, "first line second   line third line"))
-- prettier-ignore
local blankLines = (React.createElement("textlabel", nil, "value"))
-- prettier-ignore
local leadingSameLine = React.createElement("textlabel", nil, "  start next line  ")
-- prettier-ignore
local mixed = (React.createElement("frame", nil, "Hello, ", name, "!", React.createElement("textlabel"), "   trailing text"))
local whitespaceOnly = React.createElement("frame", nil, " ")
-- prettier-ignore
local nonBreakingOnly = React.createElement("frame")
-- entities
local named = React.createElement("textlabel", nil, "& <b> \"q\" 'a'  |©…€")
local numeric = React.createElement("textlabel", nil, "AB😀😀")
local unknown = React.createElement("textlabel", nil, "&unknown; &amp &#x; &#X41; & ; &;")
local controls = React.createElement("textlabel", nil, "tab\x09nl\x0acr\x0dnul\x00del\x7fus\x1f")
local surrogates = React.createElement("textlabel", nil, "😀")
-- a lone surrogate is written as U+FFFD
local loneSurrogate = React.createElement("textlabel", nil, "�x")
local quotes = React.createElement("textlabel", nil, "both \" and ' quotes, a \\ backslash and \\n")
local unicode = React.createElement("textlabel", nil, "héllo — 😀 ünïcödé")
-- prettier-ignore
local entityLines = (React.createElement("textlabel", nil, "<first> &second"))
return {
	single = single,
	padded = padded,
	multiLine = multiLine,
	blankLines = blankLines,
	leadingSameLine = leadingSameLine,
	mixed = mixed,
	whitespaceOnly = whitespaceOnly,
	nonBreakingOnly = nonBreakingOnly,
	named = named,
	numeric = numeric,
	unknown = unknown,
	controls = controls,
	surrogates = surrogates,
	loneSurrogate = loneSurrogate,
	quotes = quotes,
	unicode = unicode,
	entityLines = entityLines,
}
