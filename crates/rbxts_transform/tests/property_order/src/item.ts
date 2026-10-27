// An interface extending a base from another file (display.ts). Offsets matter here: `Item` must start after
// `Card.body` ends and end after `Card.rank` ends, and `Card.icon` must start inside it (the test checks this).
import type { Card } from "./display";

/** TS5: a, b, c, title, body, icon, rank. TS7: icon, rank, a, b, c, title, body. */
export interface Item extends Card {
	a: string;
	b: string;
	c: number;
	// Filler, so that the declaration's range covers the offsets of `Card.icon` and `Card.rank` in display.ts.
	// Filler, so that the declaration's range covers the offsets of `Card.icon` and `Card.rank` in display.ts.
	// Filler, so that the declaration's range covers the offsets of `Card.icon` and `Card.rank` in display.ts.
}
