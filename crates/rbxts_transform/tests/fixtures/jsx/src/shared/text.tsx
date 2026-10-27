// Hand-written JSX fixture: JSX text, its whitespace rule and HTML entities.
import React from "shared/react";

declare const name: string;

export const single = <textlabel>Hello world</textlabel>;
export const padded = <textlabel> spaced out </textlabel>;
// prettier-ignore
export const multiLine = (
	<textlabel>
		first line
		second   line
		third line
	</textlabel>
);
// prettier-ignore
export const blankLines = (
	<textlabel>
		value

	</textlabel>
);
// prettier-ignore
export const leadingSameLine = <textlabel>  start
		next line  </textlabel>;
// prettier-ignore
export const mixed = (
	<frame>
		Hello, {name}!
		<textlabel />   trailing text
	</frame>
);
export const whitespaceOnly = <frame> </frame>;
// prettier-ignore
export const nonBreakingOnly = <frame> 
	</frame>;

// entities
export const named = <textlabel>&amp; &lt;b&gt; &quot;q&quot; &apos;a&apos; &nbsp;|&copy;&hellip;&euro;</textlabel>;
export const numeric = <textlabel>&#65;&#x42;&#x1F600;&#0128512;</textlabel>;
export const unknown = <textlabel>&unknown; &amp &#x; &#X41; & ; &;</textlabel>;
export const controls = <textlabel>tab&#9;nl&#10;cr&#13;nul&#0;del&#127;us&#x1f;</textlabel>;
export const surrogates = <textlabel>&#xD83D;&#xDE00;</textlabel>;
// a lone surrogate is written as U+FFFD
export const loneSurrogate = <textlabel>&#xD83D;x</textlabel>;
export const quotes = <textlabel>both " and ' quotes, a \ backslash and \n</textlabel>;
export const unicode = <textlabel>héllo — 😀 ünïcödé</textlabel>;
// prettier-ignore
export const entityLines = (
	<textlabel>
		&lt;first&gt;
		&amp;second
	</textlabel>
);
