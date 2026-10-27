// A module augmentation adds a member to `Settings` (merge.ts) from another file.

import type { Settings } from "./merge";

declare module "./merge" {
	interface Settings {
		scale: number;
	}
}

/** Redeclares a merged member. TS5: volume, extra, theme, scale. */
export interface CustomSettings extends Settings {
	volume: number;
	extra: boolean;
}
